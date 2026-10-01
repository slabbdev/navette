// backend_wry — Windows + Linux backend via wry/tao (0.57/0.30).
// One implementation for both OSes: WebView2 (Windows) and WebKitGTK (Linux)
// behind the same surface. The macOS backend (backend_macos.rs) is separate —
// it has the delegate-fold optimization, screenshots and cookie state.
//
// Threading: the tao event loop owns the main thread; HTTP threads send
// Commands through the EventLoopProxy and wait on channels (same shape as the
// macOS backend's run_on_main hops). Results flow back through wry's
// evaluate_script_with_callback — the main thread is never blocked, and the
// navigate completion + folded content extraction ride the same callback path.
//
// tao/wry GTK+COM handles are !Send — they live in ghost wrappers that are
// only ever DEREFERENCED on the main thread (event loop + its scheduled
// blocks). The Send/Sync impls are the standard pattern for handles whose
// methods stay on the creating thread.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
use tao::window::{Window, WindowBuilder};
use wry::WebView;

pub const WIDTH: f64 = 1280.0;
pub const HEIGHT: f64 = 800.0;

unsafe impl Send for GhostWindow {}
unsafe impl Sync for GhostWindow {}
pub struct GhostWindow(pub Window);

unsafe impl Send for GhostWebView {}
unsafe impl Sync for GhostWebView {}
pub struct GhostWebView(pub WebView);

// ---------- Session

pub struct Session {
    pub name: String,
    pub window: GhostWindow,
    // The page-load handler closure receives this slot and reads the webview
    // from it after build() — builder-time circularity avoided.
    pub webview_slot: Arc<Mutex<Option<GhostWebView>>>,
    pub pending: Arc<Mutex<Option<PendingNav>>>,
    pub current_url: Arc<Mutex<String>>,
    pub current_title: Arc<Mutex<String>>,
}

pub type SessionRef = Arc<Session>;

pub struct PendingNav {
    pub tx: SyncSender<Result<String, String>>,
    pub after: Option<String>,
}

static SESSIONS: OnceLock<Mutex<HashMap<String, SessionRef>>> = OnceLock::new();
static PROXY: OnceLock<EventLoopProxy<Command>> = OnceLock::new();
static RPC_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    // The tao EventLoop is !Send and lives only on the main thread;
    // run_main_loop (called on main) takes it out and runs it.
    static MAIN_EVENT_LOOP: std::cell::RefCell<Option<EventLoop<Command>>> =
        std::cell::RefCell::new(None);
}

fn sessions() -> &'static Mutex<HashMap<String, SessionRef>> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn proxy() -> &'static EventLoopProxy<Command> {
    PROXY.get().expect("event loop proxy not initialized")
}

// ---------- Public surface (called from HTTP threads)

pub fn run_get_or_create(name: &str) -> SessionRef {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    proxy()
        .send_event(Command::GetOrCreate(name.to_string(), tx))
        .expect("event loop gone");
    rx.recv().expect("event loop dropped session request")
}

pub fn prewarm_default() {}

pub fn list_sessions() -> Value {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    proxy().send_event(Command::List(tx)).ok();
    rx.recv().unwrap_or_else(|_| json!({"sessions": []}))
}

pub fn close_session(name: &str) {
    proxy().send_event(Command::Close(name.to_string())).ok();
}

pub fn navigate(s: &SessionRef, url: &str, after: Option<String>) -> Result<String, String> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    // Register the pending completion BEFORE the load so the Finished event
    // (fired on main) finds it and folds the content extraction in.
    {
        let mut p = s.pending.lock().unwrap();
        *p = Some(PendingNav { tx: tx.clone(), after });
    }
    s.current_url.lock().unwrap().clear();
    proxy()
        .send_event(Command::Navigate(s.name.clone(), url.to_string()))
        .map_err(|_| "event loop gone")?;
    rx.recv_timeout(Duration::from_secs(30))
        .unwrap_or_else(|_| Err("navigation timeout".into()))
}

pub fn eval_js(s: &SessionRef, js: &str) -> Result<String, String> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    // The callback receives the JSON-serialized result of the wrapped JS.
    let wrapped = format!(
        "(function(){{ try {{ return JSON.stringify((function(){{ {js} }})()); }} catch(e) {{ return JSON.stringify({{__nav_error: String(e)}}); }} }})()",
        js = js
    );
    proxy()
        .send_event(Command::EvalJs(s.name.clone(), wrapped, tx))
        .map_err(|_| "event loop gone")?;
    match rx.recv_timeout(Duration::from_secs(20)) {
        Ok(v) => Ok(v),
        Err(_) => Err("evaluate timeout".into()),
    }
}

pub fn screenshot(_s: &SessionRef) -> Result<Vec<u8>, String> {
    Err("screenshots on Windows/Linux land in the next release — macOS ships them today".into())
}

pub fn export_cookies(s: &SessionRef) -> Result<Value, String> {
    let guard = s.webview_slot.lock().unwrap();
    let wv = guard.as_ref().ok_or("no webview")?;
    let cookies = wv.0.cookies().map_err(|e| e.to_string())?;
    let out: Vec<Value> = cookies
        .iter()
        .map(|c| {
            let expires = match c.expires() {
                Some(wry::cookie::Expiration::DateTime(dt)) => json!(dt.unix_timestamp()),
                _ => Value::Null,
            };
            json!({
                "name": c.name(),
                "value": c.value(),
                "domain": c.domain().unwrap_or(""),
                "path": c.path().unwrap_or("/"),
                "expires": expires,
                "secure": c.secure(),
                "httpOnly": c.http_only(),
            })
        })
        .collect();
    Ok(json!({ "cookies": out }))
}

pub fn import_cookies(s: &SessionRef, cookies: &Value) -> Result<usize, String> {
    let guard = s.webview_slot.lock().unwrap();
    let wv = guard.as_ref().ok_or("no webview")?;
    let mut count = 0usize;
    if let Some(arr) = cookies.get("cookies").and_then(|v| v.as_array()) {
        for c in arr {
            let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if name.is_empty() {
                continue;
            }
            let value = c.get("value").and_then(|v| v.as_str()).unwrap_or("");
            let mut builder = wry::cookie::Cookie::build((name, value));
            if let Some(d) = c.get("domain").and_then(|v| v.as_str()) {
                builder = builder.domain(d);
            }
            if let Some(p) = c.get("path").and_then(|v| v.as_str()) {
                builder = builder.path(p);
            }
            if c.get("secure").and_then(|v| v.as_bool()).unwrap_or(false) {
                builder = builder.secure(true);
            }
            if c.get("httpOnly").and_then(|v| v.as_bool()).unwrap_or(false) {
                builder = builder.http_only(true);
            }
            if let Some(ts) = c.get("expires").and_then(|v| v.as_f64()) {
                if let Ok(dt) = wry::cookie::time::OffsetDateTime::from_unix_timestamp(ts as i64) {
                    builder = builder.expires(wry::cookie::Expiration::from(dt));
                }
            }
            wv.0.set_cookie(&builder.finish()).map_err(|e| e.to_string())?;
            count += 1;
        }
    }
    Ok(count)
}

// After a click that may have triggered a form-POST navigation: wait for the
// new page to reach `complete` (opt-in via wait_navigation on /click).
pub fn wait_settle(s: &SessionRef) {
    let start = std::time::Instant::now();
    let mut saw_loading = false;
    while start.elapsed() < Duration::from_secs(10) {
        let rs = eval_js(s, "document.readyState").unwrap_or_default();
        if rs == "loading" {
            saw_loading = true;
        } else if rs == "complete" && saw_loading {
            return;
        } else if !saw_loading && start.elapsed() > Duration::from_millis(300) {
            // No navigation in flight — the click stayed on the page.
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// ---------- Event loop (main thread)

fn eval_wrapper(js: &str) -> String {
    format!(
        "(function(){{ try {{ return JSON.stringify((function(){{ {js} }})()); }} catch(e) {{ return JSON.stringify({{__nav_error: String(e)}}); }} }})()",
        js = js
    )
}

pub enum Command {
    GetOrCreate(String, SyncSender<SessionRef>),
    Navigate(String, String),
    EvalJs(String, String, SyncSender<String>),
    List(SyncSender<Value>),
    Close(String),
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Command")
    }
}

fn handle_command(cmd: Command, target: &tao::event_loop::EventLoopWindowTarget<Command>) {
    match cmd {
        Command::GetOrCreate(name, tx) => {
            if let Some(s) = sessions().lock().unwrap().get(&name) {
                let _ = tx.send(s.clone());
                return;
            }
            match create_session(&name, target) {
                Ok(s) => {
                    sessions().lock().unwrap().insert(name.clone(), s.clone());
                    let _ = tx.send(s);
                }
                Err(e) => eprintln!("[navette] session '{}' creation failed: {e}", name),
            }
        }
        Command::Navigate(name, url) => {
            let map = sessions().lock().unwrap();
            if let Some(s) = map.get(&name) {
                // The pending completion was registered by navigate() before
                // this command; the page-load handler completes it.
                if let Some(wv) = s.webview_slot.lock().unwrap().as_ref() {
                    if let Err(e) = wv.0.load_url(&url) {
                        if let Some(p) = s.pending.lock().unwrap().take() {
                            let _ = p.tx.send(Err(e.to_string()));
                        }
                    }
                }
            }
        }
        Command::EvalJs(name, script, tx) => {
            let map = sessions().lock().unwrap();
            if let Some(s) = map.get(&name) {
                if let Some(wv) = s.webview_slot.lock().unwrap().as_ref() {
                    let cb = move |result: String| {
                        let _ = tx.send(result);
                    };
                    let _ = wv.0.evaluate_script_with_callback(&script, cb);
                } else {
                    let _ = tx.send("__nav_error: webview gone".to_string());
                }
            }
        }
        Command::List(tx) => {
            let map = sessions().lock().unwrap();
            let out: Vec<Value> = map
                .values()
                .map(|s| {
                    json!({
                        "name": s.name,
                        "url": s.current_url.lock().unwrap().clone(),
                        "title": s.current_title.lock().unwrap().clone(),
                    })
                })
                .collect();
            let _ = tx.send(json!({"sessions": out}));
        }
        Command::Close(name) => {
            let removed = sessions().lock().unwrap().remove(&name);
            if let Some(s) = removed {
                let _ = s.window.0.set_visible(false);
            }
        }
    }
}

fn create_session(
    name: &str,
    target: &tao::event_loop::EventLoopWindowTarget<Command>,
) -> Result<SessionRef, String> {
    let window = WindowBuilder::new()
        .with_title(format!("navette — {name}"))
        .with_inner_size(tao::dpi::LogicalSize::new(WIDTH, HEIGHT))
        .with_position(tao::dpi::Position::Logical(tao::dpi::LogicalPosition::new(
            -WIDTH - 120.0,
            60.0,
        )))
        .build(target)
        .expect("ghost window");
    let window = GhostWindow(window);

    let pending: Arc<Mutex<Option<PendingNav>>> = Arc::new(Mutex::new(None));
    let current_url = Arc::new(Mutex::new(String::new()));
    let current_title = Arc::new(Mutex::new(String::new()));
    let webview_slot: Arc<Mutex<Option<GhostWebView>>> = Arc::new(Mutex::new(None));

    // Page-load handler: on Finished, fire the folded extraction through
    // evaluate_script_with_callback; its callback completes the pending
    // navigate. Never blocks the main thread.
    let pl_pending = pending.clone();
    let pl_url = current_url.clone();
    let pl_slot = webview_slot.clone();

    let webview = wry::WebViewBuilder::new()
        .with_url("about:blank")
        .with_on_page_load_handler(
            move |event: wry::PageLoadEvent, url: String| {
                if !matches!(event, wry::PageLoadEvent::Finished) {
                    return;
                }
                *pl_url.lock().unwrap() = url;
                let Some(p) = pl_pending.lock().unwrap().take() else { return };
                let guard = pl_slot.lock().unwrap();
                let Some(wv) = guard.as_ref() else {
                    let _ = p.tx.send(Err("webview gone".into()));
                    return;
                };
                let js = p
                    .after
                    .clone()
                    .unwrap_or_else(|| "JSON.stringify({t:document.title,c:''})".to_string());
                let fold_js = format!(
                    "(function(){{ try {{ return JSON.stringify((function(){{ {js} }})()); }} catch(e) {{ return JSON.stringify({{__nav_error: String(e)}}); }} }})()",
                    js = js
                );
                let cb = move |result: String| {
                    let _ = p.tx.send(Ok(result));
                };
                let _ = wv.0.evaluate_script_with_callback(&fold_js, cb);
            },
        )
        .build(&window.0)
        .map_err(|e| e.to_string())?;

    *webview_slot.lock().unwrap() = Some(GhostWebView(webview));

    Ok(Arc::new(Session {
        name: name.to_string(),
        window,
        webview_slot,
        pending,
        current_url,
        current_title,
    }))
}

// Called on the main thread BEFORE the listener starts: the proxy is ready
// immediately, while the loop itself starts processing at run_main_loop().
pub fn init_main_loop() {
    let event_loop = EventLoopBuilder::<Command>::with_user_event().build();
    let _ = PROXY.set(event_loop.create_proxy());
    MAIN_EVENT_LOOP.with(|c| *c.borrow_mut() = Some(event_loop));
}

pub fn run_main_loop() {
    MAIN_EVENT_LOOP.with(|cell| {
        if let Some(event_loop) = cell.borrow_mut().take() {
            event_loop.run(move |event, target, control_flow| {
                *control_flow = ControlFlow::Wait;
                if let Event::UserEvent(cmd) = event {
                    handle_command(cmd, target);
                }
            });
        }
    });
}
