// backend_macos — WKWebView via raw objc2 message sends.
// Lessons carried from the Swift reference prototype (navette-swift/):
// - AppKit init first, listener second, webview pre-warm third (measured order);
// - navigation never touches the WindowServer — the ghost window is attached
//   lazily, only when a screenshot needs it;
// - evaluateJavaScript must be CALLED on the main thread but never BLOCK it —
//   the HTTP threads hop via the main run loop and wait on a channel.

use objc2::define_class;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{class, msg_send, ClassType};
use objc2_foundation::{
    NSMutableDictionary, NSHTTPCookie, NSHTTPCookieDomain, NSHTTPCookieExpires, NSHTTPCookieName,
    NSHTTPCookiePath, NSHTTPCookiePropertyKey, NSHTTPCookieSecure, NSHTTPCookieValue, NSPoint,
    NSString, NSRect, NSSize,
};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

pub const WIDTH: f64 = 1280.0;
pub const HEIGHT: f64 = 800.0;

// Force-load the frameworks: everything below is dynamic msg_send, so without
// these the Obj-C runtime has no NSApplication/WKWebView classes to find.
#[link(name = "AppKit", kind = "framework")]
extern "C" {}
#[link(name = "WebKit", kind = "framework")]
extern "C" {}

// ---------- ObjC plumbing

#[derive(Clone)]
pub struct SendObj(pub Retained<AnyObject>);
unsafe impl Send for SendObj {}
unsafe impl Sync for SendObj {}

// Deref through the wrapper so closures capture the WHOLE SendObj (edition-2021
// disjoint capture would otherwise grab the raw Retained and break Send).
impl std::ops::Deref for SendObj {
    type Target = AnyObject;
    fn deref(&self) -> &AnyObject {
        &self.0
    }
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGImageGetWidth(image: *mut AnyObject) -> usize;
    fn CGImageGetHeight(image: *mut AnyObject) -> usize;
    fn CGImageGetBytesPerRow(image: *mut AnyObject) -> usize;
    fn CGImageGetBitsPerPixel(image: *mut AnyObject) -> usize;
    fn CGImageGetAlphaInfo(image: *mut AnyObject) -> u32;
    fn CGImageGetDataProvider(image: *mut AnyObject) -> *mut AnyObject;
    fn CGDataProviderCopyData(provider: *mut AnyObject) -> *mut AnyObject;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFDataGetLength(data: *mut AnyObject) -> isize;
    fn CFDataGetBytePtr(data: *mut AnyObject) -> *const u8;
    fn CFRelease(x: *mut AnyObject);
}

unsafe fn any_to_string(o: *mut AnyObject) -> Option<String> {
    if o.is_null() {
        return None;
    }
    let desc: *mut AnyObject = msg_send![o, description];
    if desc.is_null() {
        return None;
    }
    let ns: &NSString = &*(desc as *const NSString);
    Some(ns.to_string())
}

pub fn run_on_main<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    use std::sync::mpsc::sync_channel;
    let (tx, rx) = sync_channel(1);
    // block2 blocks must be Fn; protect the FnOnce with RefCell + Option,
    // exactly as the block2 docs prescribe.
    let f = RefCell::new(Some(f));
    let block = block2::RcBlock::new(move || {
        if let Some(g) = f.borrow_mut().take() {
            let _ = tx.send(g());
        }
    });
    unsafe {
        let rl: *mut AnyObject = msg_send![class!(NSRunLoop), mainRunLoop];
        let _: () = msg_send![rl, performBlock: &*block];
    }
    rx.recv().unwrap_or_else(|_| panic!("main run loop gone"))
}

unsafe fn make_webview() -> SendObj {
    let config: *mut AnyObject = msg_send![class!(WKWebViewConfiguration), new];
    // Ephemeral store: full isolation between sessions, nothing on disk.
    // (A per-identifier persistent store exists on macOS 13+ but its
    // initWithIdentifier selector proved unstable on this WebKit build.)
    let store: *mut AnyObject = msg_send![class!(WKWebsiteDataStore), nonPersistentDataStore];
    let _: () = msg_send![config, setWebsiteDataStore: store];
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT));
    let wv: *mut AnyObject = msg_send![class!(WKWebView), alloc];
    let wv: *mut AnyObject = msg_send![wv, initWithFrame: frame configuration: &*config];
    if let Some(ua) = MAC_UA.lock().unwrap().clone() {
        let ns: Retained<NSString> = NSString::from_str(&ua);
        let _: () = msg_send![wv, setCustomUserAgent: &*ns];
    }
    SendObj(Retained::from_raw(wv).expect("WKWebView init failed"))
}

unsafe fn attach_window(webview: &AnyObject) -> SendObj {
    let frame = NSRect::new(
        NSPoint::new(-WIDTH - 80.0, 60.0),
        NSSize::new(WIDTH, HEIGHT),
    );
    let style: u64 = 0; // borderless
    let backing: i64 = 2; // NSBackingStoreType::Buffered
    let win: *mut AnyObject = msg_send![class!(NSWindow), alloc];
    let win: *mut AnyObject = msg_send![win,
        initWithContentRect: frame
        styleMask: style
        backing: backing
        defer: false
    ];
    let _: () = msg_send![win, setContentView: &*webview];
    let _: () = msg_send![win, setReleasedWhenClosed: false];
    let _: () = msg_send![win, setCollectionBehavior: 257u64]; // canJoinAllSpaces | fullScreenAuxiliary
    let _: () = msg_send![win, orderFrontRegardless];
    SendObj(Retained::from_raw(win).expect("NSWindow init failed"))
}

// ---------- Sessions

pub struct Session {
    pub webview: SendObj,
    pub window: Mutex<Option<SendObj>>,
    // Idle-watchdog bookkeeping: updated on every run_get_or_create.
    pub last_used: Mutex<Instant>,
}

static SESSIONS: Mutex<Option<HashMap<String, Arc<Session>>>> = Mutex::new(None);
// serve --user-agent override (applied to every webview at creation).
// --proxy: on macOS the WKWebView follows the SYSTEM proxy settings; a
// per-serve override would need Network.framework plumbing — documented.
static MAC_UA: Mutex<Option<String>> = Mutex::new(None);
static LOG_FIRST_FINISH: AtomicBool = AtomicBool::new(false);

pub fn get_or_create(name: &str) -> Arc<Session> {
    // Call on the main thread.
    let mut map = SESSIONS.lock().unwrap();
    let map = map.get_or_insert_with(HashMap::new);
    if let Some(s) = map.get(name) {
        return s.clone();
    }
    let s = Arc::new(Session {
        webview: unsafe { make_webview() },
        window: Mutex::new(None),
        last_used: Mutex::new(Instant::now()),
    });
    map.insert(name.to_string(), s.clone());
    s
}

pub fn init_main_loop() {}

pub fn set_agent_options(proxy: Option<String>, user_agent: Option<String>) {
    if proxy.is_some() {
        eprintln!("[navette] --proxy on macOS: WKWebView follows the system proxy settings; per-serve override not applied");
    }
    *MAC_UA.lock().unwrap() = user_agent;
}

pub fn prewarm_default() {
    // Called directly on main before app.run().
    // NOTE (measured, twice): a fire-and-forget about:blank load here
    // SERIALIZES behind the first real navigation and slows cold start.
    // Creating the webview alone is the right warm-up.
    let _ = get_or_create("default");
}

pub fn list_sessions() -> Value {
    run_on_main(|| {
        let map = SESSIONS.lock().unwrap();
        let out: Vec<Value> = map
            .as_ref()
            .map(|m| {
                m.iter()
                    .map(|(name, s)| unsafe {
                        let url: *mut AnyObject = msg_send![&*s.webview, URL];
                        let abs: *mut AnyObject = if url.is_null() {
                            std::ptr::null_mut()
                        } else {
                            msg_send![url, absoluteString]
                        };
                        let title: *mut AnyObject = msg_send![&*s.webview, title];
                        json!({
                            "name": name,
                            "url": any_to_string(abs).unwrap_or_default(),
                            "title": any_to_string(title).unwrap_or_default(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        json!({ "sessions": out })
    })
}

pub fn close_session(name: &str) {
    let name = name.to_string();
        run_on_main(move || {
            let mut map = SESSIONS.lock().unwrap();
            if let Some(m) = map.as_mut() {
                if let Some(s) = m.remove(&name) {
                    let _: () = unsafe { msg_send![&*s.webview, stopLoading] };
                    if let Some(w) = s.window.lock().unwrap().take() {
                        let _: () = unsafe { msg_send![&*w.0, orderOut: std::ptr::null::<AnyObject>()] };
                    }
                }
            }
        });
}

// ---------- Primitives

// ---------- Navigation delegate (event-driven completion, zero polling)

struct PendingNav {
    nav: SendObj,       // the WKNavigation this completion belongs to (+1 kept)
    after: Option<String>, // optional content JS, executed the moment didFinish fires
    tx: std::sync::mpsc::SyncSender<Result<String, String>>,
}

// One entry per webview, written and drained on the main thread only.
static PENDING: Mutex<Option<HashMap<usize, PendingNav>>> = Mutex::new(None);

define_class!(
    // SAFETY: plain NSObject subclass, no subclassing requirements, no ivars.
    #[unsafe(super(NSObject))]
    struct NavDelegate;

    impl NavDelegate {
        #[unsafe(method(webView:didFinishNavigation:))]
        fn did_finish(&self, webview: *mut AnyObject, nav: *mut AnyObject) {
            unsafe { on_nav_event(webview, nav, None) }
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn did_fail(&self, webview: *mut AnyObject, nav: *mut AnyObject, err: *mut AnyObject) {
            unsafe { on_nav_event(webview, nav, Some(err)) }
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn did_fail_provisional(&self, webview: *mut AnyObject, nav: *mut AnyObject, err: *mut AnyObject) {
            unsafe { on_nav_event(webview, nav, Some(err)) }
        }
    }

    unsafe impl NSObjectProtocol for NavDelegate {}
);

fn delegate_instance() -> SendObj {
    static DELEGATE: OnceLock<SendObj> = OnceLock::new();
    DELEGATE
        .get_or_init(|| unsafe {
            let _cls = NavDelegate::class(); // triggers registration
            let obj: Retained<AnyObject> = msg_send![NavDelegate::class(), new];
            SendObj(obj)
        })
        .clone()
}

// Called from the delegate methods (main thread).
fn on_nav_event(webview: *mut AnyObject, nav: *mut AnyObject, err: Option<*mut AnyObject>) {
    let key = webview as usize;
    let pending = PENDING.lock().unwrap().as_mut().and_then(|m| m.remove(&key));
    if let Some(p) = pending {
        let same = Retained::as_ptr(&p.nav.0) as usize == nav as usize;
        if !same {
            // Not our main-frame navigation (noise) — restore and wait on.
            PENDING.lock().unwrap().as_mut().unwrap().insert(key, p);
            return;
        }
        match err {
            Some(e) => unsafe {
                let desc: *mut AnyObject = msg_send![e, localizedDescription];
                let msg = any_to_string(desc).unwrap_or_else(|| "navigation failed".into());
                let _ = p.tx.send(Err(msg));
            },
            None => {
                // didFinish — run the folded content extraction RIGHT HERE,
                // on main, with zero extra main-loop hops.
                match p.after.clone() {
                    Some(js) => unsafe {
                        let wv_retained = p.nav.clone(); // keep anything alive; js below targets webview
                        let _ = wv_retained;
                        let tx = p.tx.clone();
                        let js_ns = NSString::from_str(&js);
                        let wv_ptr = key as *mut AnyObject;
                        let block = block2::RcBlock::new(
                            move |res: *mut AnyObject, _err: *mut AnyObject| {
                                let out = unsafe { any_to_string(res) }.unwrap_or_default();
                                let _ = tx.send(Ok(out));
                            },
                        );
                        let _: () = msg_send![wv_ptr, evaluateJavaScript: &*js_ns completionHandler: &*block];
                    },
                    None => {
                        let _ = p.tx.send(Ok(String::new()));
                    }
                }
            }
        }
    }
}

// Event-driven navigation: load, wait for didFinish, and (optionally) run the
// folded content extraction in the same delegate callback.
pub fn navigate(s: &Arc<Session>, url: &str, after: Option<String>) -> Result<String, String> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let so = s.webview.clone();
    let url = url.to_string();
    let delegate = delegate_instance();
    run_on_main(move || unsafe {
        let wv = &*so;
        let _: () = msg_send![wv, setNavigationDelegate: &*delegate];
        let ns = NSString::from_str(&url);
        let nsurl: *mut AnyObject = msg_send![class!(NSURL), URLWithString: &*ns];
        if nsurl.is_null() {
            let _ = tx.send(Err("bad url".into()));
            return;
        }
        let req: *mut AnyObject = msg_send![class!(NSURLRequest), requestWithURL: nsurl];
        let nav: *mut AnyObject = msg_send![wv, loadRequest: req];
        if nav.is_null() {
            let _ = tx.send(Err("loadRequest returned nothing".into()));
            return;
        }
        // loadRequest returns an AUTORELEASED navigation (+0) — retain it before
        // from_raw consumes our own +1, or it will be over-released later.
        let owned: *mut AnyObject = msg_send![nav, retain];
        let nav_owned = SendObj(Retained::from_raw(owned).expect("navigation retained"));
        PENDING
            .lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .insert(wv as *const AnyObject as usize, PendingNav { nav: nav_owned, after, tx });
    });
    match rx.recv_timeout(Duration::from_secs(30)) {
        Ok(r) => r,
        Err(_) => Err("navigation timeout".into()),
    }
}

pub fn eval_js(s: &Arc<Session>, js: &str) -> Result<String, String> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let so = s.webview.clone();
    let js = js.to_string();
    run_on_main(move || unsafe {
        let wv = &*so;
        let js_ns = NSString::from_str(&js);
        let block = block2::RcBlock::new(move |res: *mut AnyObject, _err: *mut AnyObject| {
            let out = unsafe { any_to_string(res) }.unwrap_or_default();
            let _ = tx.send(out);
        });
        let _: () = msg_send![wv, evaluateJavaScript: &*js_ns completionHandler: &*block];
    });
    match rx.recv_timeout(Duration::from_secs(crate::webviewkit::eval_timeout_secs())) {
        Ok(v) => Ok(v),
        Err(_) => Err("evaluate timeout".into()),
    }
}

pub fn screenshot(s: &Arc<Session>) -> Result<Vec<u8>, String> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let s2 = s.clone();
    let first_attach = {
        let just = s.window.lock().unwrap().is_none();
        let so = s.webview.clone();
        run_on_main(move || unsafe {
            let wv = &*so;
            let mut w = s2.window.lock().unwrap();
            if w.is_none() {
                *w = Some(attach_window(wv));
            }
            just
        })
    };
    if first_attach {
        // Give the freshly attached ghost window one run-loop spin to lay out.
        thread::sleep(Duration::from_millis(120));
    }
    let so = s.webview.clone();
    run_on_main(move || unsafe {
        let wv = &*so;
        let block = block2::RcBlock::new(move |img: *mut AnyObject, _err: *mut AnyObject| {
            if img.is_null() {
                let _ = tx.send(Err("snapshot failed".into()));
                return;
            }
            // NSImage -> CGImage, then rasterize (Swift: img.cgImage(forProposedRect:...))
            let cg: *mut AnyObject = msg_send![img,
                CGImageForProposedRect: std::ptr::null::<AnyObject>()
                context: std::ptr::null::<AnyObject>()
                hints: std::ptr::null::<AnyObject>()
            ];
            if cg.is_null() {
                let _ = tx.send(Err("no CGImage in snapshot".into()));
                return;
            }
            let _ = tx.send(cgimage_to_png(cg));
        });
        let _: () = msg_send![wv,
            takeSnapshotWithConfiguration: std::ptr::null::<AnyObject>()
            completionHandler: &*block
        ];
    });
    match rx.recv_timeout(Duration::from_secs(30)) {
        Ok(v) => v,
        Err(_) => Err("snapshot timeout".into()),
    }
}

// ---------- Cookies (session state)

unsafe fn cookie_store(webview: &AnyObject) -> *mut AnyObject {
    let cfg: *mut AnyObject = msg_send![webview, configuration];
    let store: *mut AnyObject = msg_send![cfg, websiteDataStore];
    msg_send![store, httpCookieStore]
}

pub fn export_cookies(s: &Arc<Session>) -> Result<Value, String> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let so = s.webview.clone();
    run_on_main(move || unsafe {
        let wv = &*so;
        let cstore = cookie_store(wv);
        let block = block2::RcBlock::new(move |cookies: *mut AnyObject| {
            let mut out: Vec<Value> = Vec::new();
            if !cookies.is_null() {
                let count: usize = msg_send![cookies, count];
                for i in 0..count {
                    let c: *mut AnyObject = msg_send![cookies, objectAtIndex: i];
                    let name: *mut AnyObject = msg_send![c, name];
                    let value: *mut AnyObject = msg_send![c, value];
                    let domain: *mut AnyObject = msg_send![c, domain];
                    let path: *mut AnyObject = msg_send![c, path];
                    let secure: bool = msg_send![c, isSecure];
                    let http_only: bool = msg_send![c, isHTTPOnly];
                    let exp: *mut AnyObject = msg_send![c, expiresDate];
                    let expires = if exp.is_null() {
                        Value::Null
                    } else {
                        let ts: f64 = msg_send![exp, timeIntervalSince1970];
                        json!(ts)
                    };
                    out.push(json!({
                        "name": any_to_string(name).unwrap_or_default(),
                        "value": any_to_string(value).unwrap_or_default(),
                        "domain": any_to_string(domain).unwrap_or_default(),
                        "path": any_to_string(path).unwrap_or_default(),
                        "expires": expires,
                        "secure": secure,
                        "httpOnly": http_only,
                    }));
                }
            }
            let _ = tx.send(Ok(json!({ "cookies": out })));
        });
        let _: () = msg_send![cstore, getAllCookies: &*block];
    });
    match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(r) => r,
        Err(_) => Err("cookie export timeout".into()),
    }
}

pub fn import_cookies(s: &Arc<Session>, cookies: &Value) -> Result<usize, String> {
    eprintln!("[navette][dbg] import_cookies: entered");
    let so = s.webview.clone();
    let list = cookies
        .get("cookies")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let n = list.len();
    run_on_main(move || unsafe {
        eprintln!("[navette][dbg] import_cookies: block on main, {} cookies", n);
        let wv = &*so;
        let cstore = cookie_store(wv);
        // Typed Foundation API — the raw dictionaryWithObjects:forKeys:count:
        // msg_send deadlocked this WebKit build, do not bring it back.
        let done = block2::RcBlock::new(|| {});
        for c in &list {
            let sval = |k: &str| -> Retained<NSString> {
                NSString::from_str(c.get(k).and_then(|v| v.as_str()).unwrap_or(""))
            };
            let dict: Retained<NSMutableDictionary<NSHTTPCookiePropertyKey, AnyObject>> =
                NSMutableDictionary::new();
            let put = |d: &NSMutableDictionary<NSHTTPCookiePropertyKey, AnyObject>,
                       key: &'static NSHTTPCookiePropertyKey,
                       val: Retained<AnyObject>| {
                unsafe { d.setObject_forKey(&val, ProtocolObject::from_ref(key)) };
            };
            put(&dict, NSHTTPCookieName, Retained::cast(sval("name")));
            put(&dict, NSHTTPCookieValue, Retained::cast(sval("value")));
            put(&dict, NSHTTPCookieDomain, Retained::cast(sval("domain")));
            put(&dict, NSHTTPCookiePath, Retained::cast(sval("path")));
            if let Some(ts) = c.get("expires").and_then(|v| v.as_f64()) {
                let d: Retained<AnyObject> =
                    msg_send![class!(NSDate), dateWithTimeIntervalSince1970: ts];
                put(&dict, NSHTTPCookieExpires, d);
            }
            if let Some(b) = c.get("secure").and_then(|v| v.as_bool()) {
                let n: Retained<AnyObject> = msg_send![class!(NSNumber), numberWithBool: b];
                put(&dict, NSHTTPCookieSecure, n);
            }
            let Some(ck) = NSHTTPCookie::cookieWithProperties(&dict) else {
                continue;
            };
            let _: () = msg_send![cstore, setCookie: &*ck completionHandler: &*done];
            eprintln!(
                "[navette][dbg] import_cookies: setCookie sent for {}",
                c.get("name").and_then(|v| v.as_str()).unwrap_or("?")
            );
        }
    });
    // Settle: give the store a beat to apply and sync the cookies.
    thread::sleep(Duration::from_millis(300));
    Ok(n)
}

// After a click that may trigger a navigation: settle briefly. If the click
// started a navigation, wait for it to finish; if nothing happened within
// 300 ms, return immediately (opt-in via wait_navigation on /click).
pub fn set_viewport(s: &Arc<Session>, width: u32, height: u32) -> Result<(), String> {
    // Attach the ghost if this session never had a window — /sessions/viewport
    // must not require a screenshot first (the n8n navigate journey sets the
    // viewport before anything else).
    {
        let s2 = s.clone();
        let so = s.webview.clone();
        run_on_main(move || unsafe {
            let wv = &*so;
            let mut w = s2.window.lock().unwrap();
            if w.is_none() {
                *w = Some(attach_window(wv));
            }
        });
    }
    let win = {
        let guard = s.window.lock().unwrap();
        match guard.as_ref() {
            Some(w) => w.clone(),
            None => return Err("session has no window yet — take a screenshot first".into()),
        }
    };
    run_on_main(move || unsafe {
        let win = &*win;
        let frame: NSRect = msg_send![win, frame];
        let new = NSRect::new(frame.origin, NSSize::new(width as f64, height as f64));
        let _: () = msg_send![win, setFrame: new display: true];
    });
    Ok(())
}

// ---------- Human-visible windows (one-time logins & friends) ----------
//
// The ghost window is borderless and parked off-screen — a rendering surface,
// not a surface a human can interact with: a borderless NSWindow can never
// become the key window, so a login form would swallow every keystroke.
// show_window() restyles it titled (keyable), sizes the CONTENT rect back to
// the viewport (the title bar eats into it otherwise), centers it on the main
// screen and fronts the app. hide_window() orders it back out; the ghost
// screenshot path is unaffected — takeSnapshot rasterizes the layer tree.

pub fn show_window(s: &Arc<Session>) -> Result<(), String> {
    {
        // Attach the ghost if this session never had a window.
        let s2 = s.clone();
        let so = s.webview.clone();
        run_on_main(move || unsafe {
            let wv = &*so;
            let mut w = s2.window.lock().unwrap();
            if w.is_none() {
                *w = Some(attach_window(wv));
            }
        });
    }
    let win = {
        let guard = s.window.lock().unwrap();
        guard.as_ref().ok_or("session has no window")?.clone()
    };
    run_on_main(move || unsafe {
        let win = &*win;
        // Titled windows can become key; the ghost's borderless mask cannot.
        let style: u64 = 1 | 2 | 4 | 8; // titled | closable | miniaturizable | resizable
        let _: () = msg_send![win, setStyleMask: style];
        let title: Retained<NSString> = NSString::from_str("navette");
        let _: () = msg_send![win, setTitle: &*title];
        let _: () = msg_send![win, setContentSize: NSSize::new(WIDTH, HEIGHT)];
        // Center on the main screen's visible frame (menu bar / Dock excluded).
        let screen: *mut AnyObject = msg_send![class!(NSScreen), mainScreen];
        let vf: NSRect = msg_send![screen, visibleFrame];
        let frame: NSRect = msg_send![win, frame];
        let origin = NSPoint::new(
            vf.origin.x + (vf.size.width - frame.size.width) / 2.0,
            vf.origin.y + (vf.size.height - frame.size.height) / 2.0,
        );
        let _: () = msg_send![win, setFrameOrigin: origin];
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, activateIgnoringOtherApps: true];
        let _: () = msg_send![win, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];
    });
    Ok(())
}

pub fn hide_window(s: &Arc<Session>) -> Result<(), String> {
    let win = {
        let guard = s.window.lock().unwrap();
        guard.as_ref().ok_or("session has no window")?.clone()
    };
    run_on_main(move || unsafe {
        let win = &*win;
        let _: () = msg_send![win, orderOut: std::ptr::null::<AnyObject>()];
    });
    Ok(())
}

// ---------- Native (OS-level) key events (CGEvent) ----------
//
// Synthetic JS KeyboardEvents never set isTrusted; CGEvents posted at the
// HID tap are real events the OS routes to the frontmost app. We attach the
// ghost window if needed, make it key, then post the press+release.

#[allow(dead_code)]
fn mac_vk(key: &str) -> Option<u16> {
    let k = key.to_ascii_lowercase();
    Some(match k.as_str() {
        "enter" | "return" => 0x24,
        "tab" => 0x30,
        "escape" | "esc" => 0x35,
        "backspace" => 0x33,
        "delete" | "del" => 0x75,
        "space" => 0x31,
        "up" | "arrowup" => 0x7E,
        "down" | "arrowdown" => 0x7D,
        "left" | "arrowleft" => 0x7B,
        "right" | "arrowright" => 0x7C,
        "home" => 0x73,
        "end" => 0x77,
        "pageup" => 0x74,
        "pagedown" => 0x79,
        _ => {
            let c = k.chars().next()?;
            match c {
                'a' => 0x00, 's' => 0x01, 'd' => 0x02, 'f' => 0x03, 'h' => 0x04,
                'g' => 0x05, 'z' => 0x06, 'x' => 0x07, 'c' => 0x08, 'v' => 0x09,
                'b' => 0x0B, 'q' => 0x0C, 'w' => 0x0D, 'e' => 0x0E, 'r' => 0x0F,
                'y' => 0x10, 't' => 0x11, '1' => 0x12, '2' => 0x13, '3' => 0x14,
                '4' => 0x15, '5' => 0x17, '6' => 0x16, '7' => 0x1A, '8' => 0x1C,
                '9' => 0x19, '0' => 0x1D, 'm' => 0x2E, 'o' => 0x1F, 'p' => 0x23,
                'l' => 0x25, 'j' => 0x26, 'k' => 0x28, 'i' => 0x22, 'u' => 0x20,
                'n' => 0x2D,
                _ => return None,
            }
        }
    })
}

// Not exposed by core-graphics 0.24 — attach a unicode string to a key
// event so the delivered character is layout-independent (physical keycode
// 0x00 is 'a' on QWERTY and 'q' on AZERTY; the unicode string wins), plus
// the raw post/release pair to route the event after into_raw().
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventKeyboardSetUnicodeString(
        event: core_graphics::sys::CGEventRef,
        length: libc::c_ulong,
        string: *const u16,
    );
    fn CGEventPost(tap: libc::c_int, event: core_graphics::sys::CGEventRef);
}

/// kCGHIDEventTap — post into the hardware-input stream (lands on the
/// focused application, which the focus gate guarantees is ours).
const kCGHIDEventTap: libc::c_int = 0;

/// macOS virtual keycodes (HIToolbox/Events.h) for the keys agents send.
fn mac_keycode(key: &str) -> Option<u16> {
    let lower = key.to_ascii_lowercase();
    Some(match lower.as_str() {
        "a" => 0x00, "s" => 0x01, "d" => 0x02, "f" => 0x03, "h" => 0x04,
        "g" => 0x05, "z" => 0x06, "x" => 0x07, "c" => 0x08, "v" => 0x09,
        "b" => 0x0B, "q" => 0x0C, "w" => 0x0D, "e" => 0x0E, "r" => 0x0F,
        "y" => 0x10, "t" => 0x11, "1" => 0x12, "2" => 0x13, "3" => 0x14,
        "4" => 0x15, "6" => 0x16, "5" => 0x17, "=" => 0x18, "9" => 0x19,
        "7" => 0x1A, "-" => 0x1B, "8" => 0x1C, "0" => 0x1D, "]" => 0x1E,
        "o" => 0x1F, "u" => 0x20, "[" => 0x21, "i" => 0x22, "p" => 0x23,
        "return" | "enter" => 0x24, "l" => 0x25, "j" => 0x26, "'" => 0x27,
        "k" => 0x28, ";" => 0x29, "\\" => 0x2A, "," => 0x2B, "/" => 0x2C,
        "n" => 0x2D, "m" => 0x2E, "." => 0x2F,
        "tab" => 0x30, " " | "space" => 0x31, "`" => 0x32,
        "backspace" | "delete" => 0x33, "escape" | "esc" => 0x35,
        "shift" => 0x38, "option" | "alt" => 0x3A, "control" | "ctrl" => 0x3B,
        "arrowleft" | "left" => 0x7B, "arrowright" | "right" => 0x7C,
        "arrowdown" | "down" => 0x7D, "arrowup" | "up" => 0x7E,
        _ => return None,
    })
}

/// Native key input, the honest attempt-first way (#3): bring the window
/// on-screen and key, activate the app, and only post CGEvents once the
/// Window Server actually made OUR window key — posting into the HID tap
/// without focus would type into someone else's app. Delivery is verified
/// in-page (isTrusted) before Ok is reported; anything less is an Err and
/// the /key route falls back to the synthetic dispatch, mode reported.
#[allow(dead_code)]
pub fn native_key(s: &Arc<Session>, key: &str) -> Result<(), String> {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use foreign_types::ForeignType;

    let code = mac_keycode(key).ok_or_else(|| format!("no native mapping for key '{key}'"))?;

    // Focus gate — show_window is the whole recipe: it materializes the
    // ghost's window, converts it to a TITLED window (borderless windows
    // can never become key), centers it, activates the app, and makes it
    // key. Only then may CGEvents be posted: posting into the HID tap
    // without focus would type into someone else's app.
    show_window(s)?;
    std::thread::sleep(Duration::from_millis(150));
    let is_key: bool = {
        let win = { s.window.lock().unwrap().as_ref().cloned() }
            .ok_or("session has no window")?;
        run_on_main(move || unsafe { msg_send![&*win, isKeyWindow] })
    };
    if !is_key {
        return Err(
            "Window Server did not grant key focus (navette is not an app bundle) — not posting".into(),
        );
    }

    // Delivery probe: a document-level once-listener records the next keydown.
    let probe = "__navette_native_probe";
    let _ = eval_js(
        s,
        &format!(
            "(function(){{window.{p}=null;document.addEventListener('keydown',\
             function(e){{window.{p}=JSON.stringify({{trusted:e.isTrusted,key:e.key}})}},\
             {{once:true}});return 'armed'}})()",
            p = probe
        ),
    );

    let src = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|e| format!("CGEventSource failed: {e:?}"))?;
    // Single characters ride as unicode strings — the delivered e.key is
    // then the character the agent asked for, whatever the active layout.
    let utf16: Vec<u16> = if key.chars().count() == 1 {
        key.encode_utf16().collect()
    } else {
        Vec::new()
    };
    for down in [true, false] {
        let ev = CGEvent::new_keyboard_event(src.clone(), code, down)
            .map_err(|e| format!("CGEvent create failed: {e:?}"))?;
        let raw = ev.as_ptr();
        if !utf16.is_empty() {
            unsafe {
                CGEventKeyboardSetUnicodeString(
                    raw,
                    utf16.len() as libc::c_ulong,
                    utf16.as_ptr(),
                );
            }
        }
        unsafe { CGEventPost(kCGHIDEventTap, raw) };
        // ev's Drop releases the event ref on scope exit.
    }

    // Delivery verification: the page must have seen a TRUSTED keydown with
    // this key — otherwise report failure so the caller falls back.
    std::thread::sleep(Duration::from_millis(250));
    let seen = eval_js(s, &format!("window.{p} === null ? 'none' : window.{p}", p = probe))
        .unwrap_or_else(|_| "eval-failed".into());
    let _ = eval_js(s, &format!("delete window.{p}", p = probe));
    let seen_l = seen.to_ascii_lowercase();
    if seen_l.contains("\"trusted\":true") {
        return Ok(());
    }
    Err(format!("CGEvent posted but not delivered as trusted (probe: {seen})"))
}

pub fn wait_settle(s: &Arc<Session>) {
    let start = Instant::now();
    let mut saw_loading = false;
    while start.elapsed() < Duration::from_secs(10) {
        let loading = {
            let so = s.webview.clone();
            run_on_main(move || unsafe {
                let wv = &*so;
                let l: bool = msg_send![wv, isLoading];
                l
            })
        };
        if loading {
            saw_loading = true;
        } else if saw_loading || start.elapsed() > Duration::from_millis(300) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

unsafe fn cgimage_to_png(img: *mut AnyObject) -> Result<Vec<u8>, String> {
    let w = CGImageGetWidth(img);
    let h = CGImageGetHeight(img);
    let bpr = CGImageGetBytesPerRow(img);
    if CGImageGetBitsPerPixel(img) != 32 || w == 0 || h == 0 {
        return Err(format!("unsupported bitmap {}x{}bpp", w, CGImageGetBitsPerPixel(img)));
    }
    let data = CGDataProviderCopyData(CGImageGetDataProvider(img));
    if data.is_null() {
        return Err("no pixel data".into());
    }
    let len = CFDataGetLength(data) as usize;
    let ptr = CFDataGetBytePtr(data);
    // kCGImageAlphaPremultipliedFirst (2) / kCGImageAlphaFirst (4): WebKit hands
    // us BGRA bytes on little-endian — swap B<->R for the RGBA PNG encoder.
    let alpha_first = matches!(CGImageGetAlphaInfo(img), 2 | 4);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h {
        let start = (row * bpr) as usize;
        let end = start + (w * 4) as usize;
        if end > len {
            CFRelease(data);
            return Err("pixel buffer short".into());
        }
        let row_bytes = std::slice::from_raw_parts(ptr.add(start), end - start);
        if alpha_first {
            for px in row_bytes.chunks_exact(4) {
                rgba.push(px[2]);
                rgba.push(px[1]);
                rgba.push(px[0]);
                rgba.push(px[3]);
            }
        } else {
            rgba.extend_from_slice(row_bytes);
        }
    }
    CFRelease(data);
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().map_err(|e| e.to_string())?;
    wr.write_image_data(&rgba).map_err(|e| e.to_string())?;
    wr.finish().map_err(|e| e.to_string())?;
    Ok(out)
}

// ---------- App lifecycle (called from main, on the main thread)

pub fn app_init() {
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, setActivationPolicy: 1i64]; // .accessory
    }
}

pub fn app_run() {
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, run];
    }
}

// Route-facing wrapper: hop to main, create-or-fetch. Safe to call from any
// HTTP thread once app.run() is active.
pub fn run_get_or_create(name: &str) -> Arc<Session> {
    let n = name.to_string();
    let s = run_on_main(move || get_or_create(&n));
    *s.last_used.lock().unwrap() = Instant::now();
    s
}

// Idle watchdog: drop whole idle sessions on the main thread so the
// WKWebView deallocates where AppKit expects it. The daemon stays alive;
// the next request re-creates and re-pre-warms on demand.
pub fn reap_idle(max_idle_secs: u64) -> usize {
    let now = Instant::now();
    let idle: Vec<String> = {
        let map = SESSIONS.lock().unwrap();
        match map.as_ref() {
            Some(m) => m
                .iter()
                .filter(|(_, s)| now.duration_since(*s.last_used.lock().unwrap()).as_secs() >= max_idle_secs)
                .map(|(n, _)| n.clone())
                .collect(),
            None => Vec::new(),
        }
    };
    for n in &idle {
        let n2 = n.clone();
        run_on_main(move || {
            let mut map = SESSIONS.lock().unwrap();
            if let Some(m) = map.as_mut() {
                if let Some(s) = m.remove(&n2) {
                    let _: () = unsafe { msg_send![&*s.webview, stopLoading] };
                    // s drops here, on the main thread
                }
            }
        });
    }
    if !idle.is_empty() {
        eprintln!("[navette] idle-release: dropped {} session(s) (idle >= {}s)", idle.len(), max_idle_secs);
    }
    idle.len()
}

// ---------- Kit contract (compile-checked parity — see webviewkit.rs)

use crate::webviewkit::WebviewKit;

#[allow(dead_code)]
pub struct Kit;

impl WebviewKit for Kit {
    type SessionRef = Arc<Session>;

    fn run_get_or_create(&self, name: &str) -> Self::SessionRef {
        run_get_or_create(name)
    }
    fn prewarm_default(&self) {
        prewarm_default()
    }
    fn set_agent_options(&self, proxy: Option<String>, user_agent: Option<String>) {
        set_agent_options(proxy, user_agent)
    }
    fn list_sessions(&self) -> Value {
        list_sessions()
    }
    fn close_session(&self, name: &str) {
        close_session(name)
    }
    fn reap_idle(&self, max_idle_secs: u64) -> usize {
        reap_idle(max_idle_secs)
    }
    fn navigate(&self, s: &Self::SessionRef, url: &str, after: Option<String>) -> Result<String, String> {
        navigate(s, url, after)
    }
    fn eval_js(&self, s: &Self::SessionRef, js: &str) -> Result<String, String> {
        eval_js(s, js)
    }
    fn screenshot(&self, s: &Self::SessionRef) -> Result<Vec<u8>, String> {
        screenshot(s)
    }
    fn export_cookies(&self, s: &Self::SessionRef) -> Result<Value, String> {
        export_cookies(s)
    }
    fn import_cookies(&self, s: &Self::SessionRef, cookies: &Value) -> Result<usize, String> {
        import_cookies(s, cookies)
    }
    fn set_viewport(&self, s: &Self::SessionRef, width: u32, height: u32) -> Result<(), String> {
        set_viewport(s, width, height)
    }
    fn show_window(&self, s: &Self::SessionRef) -> Result<(), String> {
        show_window(s)
    }
    fn hide_window(&self, s: &Self::SessionRef) -> Result<(), String> {
        hide_window(s)
    }
    fn native_key(&self, s: &Self::SessionRef, key: &str) -> Result<(), String> {
        native_key(s, key)
    }
    fn wait_settle(&self, s: &Self::SessionRef) {
        wait_settle(s)
    }
}
