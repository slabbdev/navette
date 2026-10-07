// backend_wry — Windows + Linux backend via wry/tao (0.57/0.30).
// One implementation for both OSes: WebView2 (Windows) and WebKitGTK (Linux)
// behind the same surface. The macOS backend (backend_macos.rs) is separate —
// it has the delegate-fold optimization, screenshots and cookie state.
//
// Threading: the tao event loop owns the main thread; HTTP threads send
// Commands through the EventLoopProxy and wait on channels (same shape as the
// macOS backend's run_on_main hops).
//
// Results flow back through the IPC shim (window.ipc.postMessage):
// evaluate results are id-tagged, navigate completions carry the folded
// content. The main thread is never blocked, and the navigate completion +
// folded content extraction ride the same callback path.
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
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::{Window, WindowBuilder};
use wry::WebView;

pub const WIDTH: f64 = 1280.0;
pub const HEIGHT: f64 = 800.0;

// Injected at webview creation: eval results and fold completions both post
// through window.ipc (the with_callback path returns empty on WebKitGTK).
const RPC_SHIM: &str = r#"window.__navette_rpc = (id, value) => { window.ipc.postMessage(id + ':' + JSON.stringify(value === undefined ? null : value)); };
try { var __nav_dlg = function(o) { try { window.ipc.postMessage('__navette_dialog:' + JSON.stringify(o)); } catch(e) {} };
window.alert = function(m) { __nav_dlg({kind:'alert', message:String(m)}); };
window.confirm = function(m) { __nav_dlg({kind:'confirm', message:String(m)}); return true; };
window.prompt = function(m, d) { __nav_dlg({kind:'prompt', message:String(m)}); return d === undefined ? null : d; };
} catch(e) {}"#;

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
    pub evals: Arc<Mutex<HashMap<u64, SyncSender<String>>>>,
    pub pending: Arc<Mutex<Option<PendingNav>>>,
    // Once the URL-matched Finished takes the pending, the fold result still
    // has to come back through ipc — it needs the route's tx, which pending
    // no longer holds at that point.
    pub fold_tx: Arc<Mutex<Option<SyncSender<Result<String, String>>>>>,
    // Idle-watchdog bookkeeping: updated on every run_get_or_create.
    pub last_used: Mutex<std::time::Instant>,
    pub current_url: Arc<Mutex<String>>,
    pub current_title: Arc<Mutex<String>>,
}

pub type SessionRef = Arc<Session>;

pub struct PendingNav {
    pub tx: SyncSender<Result<String, String>>,
    pub after: Option<String>,
    pub url: String,  // completion matches the target URL — the initial
                      // about:blank event must not swallow it
    pub name: String, // the retry EvalJs needs the session name
}

static SESSIONS: OnceLock<Mutex<HashMap<String, SessionRef>>> = OnceLock::new();
// serve --proxy / --user-agent: applied to every session at webview creation.
static AGENT_OPTS: OnceLock<(Option<String>, Option<String>)> = OnceLock::new();
static PROXY: OnceLock<EventLoopProxy<Command>> = OnceLock::new();
static RPC_ID: AtomicU64 = AtomicU64::new(1);

fn sessions() -> &'static Mutex<HashMap<String, SessionRef>> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_rpc_id() -> u64 {
    RPC_ID.fetch_add(1, Ordering::SeqCst)
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
    let s: SessionRef = rx.recv().expect("event loop dropped session request");
    *s.last_used.lock().unwrap() = std::time::Instant::now();
    s
}

pub fn prewarm_default() {}

// ---------- Native (OS-level) key events ----------
//
// Synthetic JS KeyboardEvents do not set isTrusted and never drive IME or
// browser shortcuts. This posts REAL events to the ghost window: SendInput
// on Windows (to the focused WebView2 render widget), XTEST on Linux.

#[cfg(target_os = "windows")]
fn win_vk(key: &str) -> Option<u16> {
    let k = key.to_ascii_lowercase();
    Some(match k.as_str() {
        "enter" | "return" => 0x0D,
        "tab" => 0x09,
        "escape" | "esc" => 0x1B,
        "backspace" => 0x08,
        "delete" | "del" => 0x2E,
        "space" => 0x20,
        "up" | "arrowup" => 0x26,
        "down" | "arrowdown" => 0x28,
        "left" | "arrowleft" => 0x25,
        "right" | "arrowright" => 0x27,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        _ => {
            let c = k.chars().next()?;
            if c.is_ascii_alphabetic() {
                c.to_ascii_uppercase() as u16
            } else if c.is_ascii_digit() {
                c as u16
            } else {
                return None;
            }
        }
    })
}

#[cfg(target_os = "windows")]
pub fn native_key(s: &SessionRef, key: &str) -> Result<(), String> {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, SetFocus, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumChildWindows, GetWindowRect, SetForegroundWindow,
    };
    let vk = win_vk(key).ok_or_else(|| format!("no native mapping for key '{key}'"))?;
    let hwnd = window_hwnd(&s.window.0)?;
    unsafe {
        let mut best: (isize, i64) = (0, 0);
        EnumChildWindows(hwnd as HWND, Some(enum_child), &mut best as *mut _ as isize);
        let target = if best.0 != 0 { best.0 } else { hwnd };
        SetForegroundWindow(hwnd as HWND);
        SetFocus(target as HWND);
        let mk = |up: bool| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: {
                let mut i: INPUT_0 = std::mem::zeroed();
                i.ki = KEYBDINPUT {
                    wVk: vk,
                    dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                    ..std::mem::zeroed()
                };
                i
            },
        };
        let down = mk(false);
        let up = mk(true);
        let sent = SendInput(2, [down, up].as_ptr(), std::mem::size_of::<INPUT>() as i32);
        if sent != 2 {
            return Err("SendInput failed".into());
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn x11_keysym(key: &str) -> Option<u32> {
    let k = key.to_ascii_lowercase();
    Some(match k.as_str() {
        "enter" | "return" => 0xff0d,
        "tab" => 0xff09,
        "escape" | "esc" => 0xff1b,
        "backspace" => 0xff08,
        "delete" | "del" => 0xffff,
        "space" => 0x0020,
        "up" | "arrowup" => 0xff52,
        "down" | "arrowdown" => 0xff54,
        "left" | "arrowleft" => 0xff51,
        "right" | "arrowright" => 0xff53,
        "home" => 0xff50,
        "end" => 0xff57,
        "pageup" => 0xff55,
        "pagedown" => 0xff56,
        _ => {
            let c = k.chars().next()?;
            if c.is_ascii() {
                c as u32
            } else {
                return None;
            }
        }
    })
}

#[cfg(target_os = "linux")]
pub fn native_key(s: &SessionRef, key: &str) -> Result<(), String> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt, EventMask, InputFocus};
    let keysym = x11_keysym(key).ok_or_else(|| format!("no native mapping for key '{key}'"))?;
    let (conn, screen) = x11rb::connect(None).map_err(|e| e.to_string())?;
    let root = conn.setup().roots[screen].root;
    let min = conn.setup().min_keycode;
    let max = conn.setup().max_keycode;
    let map = conn
        .get_keyboard_mapping(min, max - min + 1)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    let per = (map.keysyms_per_keycode as usize).max(1);
    let keycode = map
        .keysyms
        .chunks(per)
        .position(|chunk| chunk.contains(&keysym))
        .map(|idx| min + idx as u8)
        .ok_or_else(|| format!("keycode not found for '{key}' on this keymap"))?;
    // Bring the ghost on-screen first: X11 refuses focus to unviewable
    // windows. NOTE: no gtk::main_iteration() here — this runs on an HTTP
    // thread and GTK is main-thread-only (an iteration pump from a worker
    // deadlocks the loop; that was CI exit 52).
    {
        s.window.0.set_visible(true);
        s.window.0.set_outer_position(tao::dpi::PhysicalPosition::new(0, 0));
    }
    if let RawWindowHandle::Xlib(h) = s.window.0.window_handle().map_err(|e| e.to_string())?.as_raw() {
        let wid: u32 = h.window as u32;
        let _ = conn.set_input_focus(InputFocus::PARENT, wid, 0u32); // 0 = CurrentTime
    }
    let _ = conn.change_window_attributes(root, &ChangeWindowAttributesAux::new().event_mask(EventMask::KEY_PRESS | EventMask::KEY_RELEASE));
    x11rb::protocol::xtest::fake_input(&conn, 2, keycode, 0, root, 0, 0, 0)
        .map_err(|e| e.to_string())?; // 2 = KeyPress
    x11rb::protocol::xtest::fake_input(&conn, 3, keycode, 0, root, 0, 0, 0)
        .map_err(|e| e.to_string())?; // 3 = KeyRelease
    conn.flush().map_err(|e| e.to_string())?;
    Ok(())
}

pub fn set_agent_options(proxy: Option<String>, user_agent: Option<String>) {
    let _ = AGENT_OPTS.set((proxy, user_agent));
}

// "http://host:port" / "socks5://host:port" / "host:port" -> ProxyEndpoint.
// Proxy AUTH is not carried by the engine layer — warn loudly.
fn parse_proxy(url: &str) -> Result<wry::ProxyConfig, String> {
    let rest = url
        .split_once("://")
        .map(|(scheme, r)| {
            if scheme.starts_with("socks") {
                (true, r)
            } else {
                (false, r)
            }
        })
        .unwrap_or((false, url));
    let (socks, authority) = rest;
    let hostport = if let Some((userinfo, hp)) = authority.rsplit_once('@') {
        eprintln!(
            "[navette] WARNING: proxy credentials in the URL are NOT passed through by the engine layer — use an unauthenticated proxy or IP allowlisting"
        );
        let _ = userinfo;
        hp
    } else {
        authority
    };
    let (host, port) = hostport
        .rsplit_once(':')
        .ok_or("proxy URL must include a port (host:port)")?;
    let ep = wry::ProxyEndpoint {
        host: host.to_string(),
        port: port.to_string(),
    };
    Ok(if socks {
        wry::ProxyConfig::Socks5(ep)
    } else {
        wry::ProxyConfig::Http(ep)
    })
}

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
        *p = Some(PendingNav {
            tx: tx.clone(),
            after,
            url: url.trim_end_matches('/').to_string(),
            name: s.name.clone(),
        });
    }
    s.current_url.lock().unwrap().clear();
    proxy()
        .send_event(Command::Navigate(s.name.clone(), url.to_string()))
        .map_err(|_| "event loop gone")?;
    rx.recv_timeout(Duration::from_secs(45))
        .unwrap_or_else(|_| Err("navigation timeout".into()))
}

pub fn eval_js(s: &SessionRef, js: &str) -> Result<String, String> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let id = next_rpc_id();
    // The script POSTS its result through the ipc shim (the with_callback
    // path returns empty on WebKitGTK — the ipc path is the reliable one).
    // `js` is substituted as an EXPRESSION — wrapping it in a function body
    // would silently discard its value (the fold_js_for lesson, again).
    let script = format!(
        "(function(){{ try {{ var v = ({js}); var r = (v === undefined) ? 'null' : JSON.stringify(v); window.ipc.postMessage('{id}:' + r); }} catch(e) {{ window.ipc.postMessage('{id}:' + JSON.stringify({{__nav_error: String(e)}})); }} }})()",
        id = id,
        js = js
    );
    s.evals.lock().unwrap().insert(id, tx);
    proxy()
        .send_event(Command::EvalJs(s.name.clone(), script))
        .map_err(|_| "event loop gone")?;
    match rx.recv_timeout(Duration::from_secs(20)) {
        Ok(v) => Ok(v),
        Err(_) => {
            s.evals.lock().unwrap().remove(&id);
            Err("evaluate timeout".into())
        }
    }
}

pub fn screenshot(s: &SessionRef) -> Result<Vec<u8>, String> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    proxy()
        .send_event(Command::Screenshot(s.name.clone(), tx))
        .map_err(|_| "event loop gone")?;
    rx.recv_timeout(Duration::from_secs(30))
        .unwrap_or_else(|_| Err("screenshot timeout".into()))
}

pub fn export_cookies(s: &SessionRef) -> Result<Value, String> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    proxy()
        .send_event(Command::ExportCookies(s.name.clone(), tx))
        .map_err(|_| "event loop gone")?;
    rx.recv_timeout(Duration::from_secs(30))
        .unwrap_or_else(|_| Err("cookie export timeout".into()))
}

pub fn import_cookies(s: &SessionRef, cookies: &Value) -> Result<usize, String> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    proxy()
        .send_event(Command::ImportCookies(s.name.clone(), cookies.clone(), tx))
        .map_err(|_| "event loop gone")?;
    rx.recv_timeout(Duration::from_secs(30))
        .unwrap_or_else(|_| Err("cookie import timeout".into()))
}

pub fn reap_idle(max_idle_secs: u64) -> usize {
    let Some(p) = PROXY.get() else { return 0 }; // event loop not up yet — nothing to reap
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    if p.send_event(Command::Reap(max_idle_secs, tx)).is_err() {
        return 0;
    }
    rx.recv_timeout(Duration::from_secs(10)).unwrap_or(0)
}

pub fn set_viewport(s: &SessionRef, width: u32, height: u32) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    proxy()
        .send_event(Command::Viewport(s.name.clone(), width, height, tx))
        .map_err(|_| "event loop gone")?;
    rx.recv_timeout(Duration::from_secs(10))
        .unwrap_or_else(|_| Err("viewport timeout".into()))
}

pub fn show_window(s: &SessionRef) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    proxy()
        .send_event(Command::Show(s.name.clone(), tx))
        .map_err(|_| "event loop gone")?;
    rx.recv_timeout(Duration::from_secs(10))
        .unwrap_or_else(|_| Err("show timeout".into()))
}

pub fn hide_window(s: &SessionRef) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    proxy()
        .send_event(Command::Hide(s.name.clone(), tx))
        .map_err(|_| "event loop gone")?;
    rx.recv_timeout(Duration::from_secs(10))
        .unwrap_or_else(|_| Err("hide timeout".into()))
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

// ---------- Cookie state (same JSON schema as the macOS backend)

fn cookies_json(wv: &GhostWebView) -> Result<Value, String> {
    let list = wv.0.cookies().map_err(|e| e.to_string())?;
    let out: Vec<Value> = list
        .iter()
        .map(|c| {
            json!({
                "name": c.name(),
                "value": c.value(),
                "domain": c.domain().unwrap_or(""),
                "path": c.path().unwrap_or(""),
                "expires": match c.expires() {
                    Some(wry::cookie::Expiration::DateTime(dt)) => json!(dt.unix_timestamp() as f64),
                    _ => Value::Null,
                },
                "secure": c.secure().unwrap_or(false),
                "httpOnly": c.http_only().unwrap_or(false),
            })
        })
        .collect();
    Ok(json!({ "cookies": out }))
}

fn cookies_from_json(wv: &GhostWebView, v: &Value) -> Result<usize, String> {
    let list = v
        .get("cookies")
        .and_then(|c| c.as_array())
        .ok_or("expected {\"cookies\": [...]}")?;
    let mut n = 0;
    for c in list {
        let name = c.get("name").and_then(|x| x.as_str()).unwrap_or("");
        let value = c.get("value").and_then(|x| x.as_str()).unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let mut b = wry::cookie::Cookie::build((name.to_string(), value.to_string()));
        if let Some(d) = c.get("domain").and_then(|x| x.as_str()) {
            if !d.is_empty() {
                b = b.domain(d.to_string());
            }
        }
        if let Some(p) = c.get("path").and_then(|x| x.as_str()) {
            if !p.is_empty() {
                b = b.path(p.to_string());
            }
        }
        if c.get("secure").and_then(|x| x.as_bool()).unwrap_or(false) {
            b = b.secure(true);
        }
        if c.get("httpOnly").and_then(|x| x.as_bool()).unwrap_or(false) {
            b = b.http_only(true);
        }
        if let Some(e) = c.get("expires").and_then(|x| x.as_f64()) {
            if let Ok(dt) = wry::cookie::time::OffsetDateTime::from_unix_timestamp(e as i64) {
                b = b.expires(wry::cookie::Expiration::DateTime(dt));
            }
        }
        wv.0.set_cookie(&b.build()).map_err(|e| e.to_string())?;
        n += 1;
    }
    Ok(n)
}

// ---------- Screenshot (native capture of the ghost window)

fn encode_png(rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

fn screenshot_on_main(s: &SessionRef) -> Result<Vec<u8>, String> {
    let size = s.window.0.inner_size();
    if size.width == 0 || size.height == 0 {
        return Err("window has zero size".into());
    }
    s.window.0.set_visible(true);
    let old = s.window.0.outer_position().ok();
    // Bring the ghost to the origin: off-screen windows are never composited
    // on X11 (no compositor under Xvfb), and GDI/X capture reads the
    // framebuffer. At (0,0) the capture rect stays inside the root window —
    // GetImage raises BadMatch for any rect crossing the screen edge.
    s.window
        .0
        .set_outer_position(tao::dpi::PhysicalPosition::new(0i32, 0));
    #[cfg(target_os = "linux")]
    {
        // pump the GTK loop so the move + expose actually repaint
        for _ in 0..12 {
            gtk::main_iteration();
        }
        std::thread::sleep(Duration::from_millis(120));
        for _ in 0..6 {
            gtk::main_iteration();
        }
        let shot = capture_x11(size.width, size.height);
        if let Some(p) = old {
            s.window
                .0
                .set_outer_position(tao::dpi::PhysicalPosition::new(p.x, p.y));
        }
        for _ in 0..4 {
            gtk::main_iteration();
        }
        shot
    }
    #[cfg(target_os = "windows")]
    {
        let shot = capture_gdi(&s.window.0, size.width, size.height);
        if let Some(p) = old {
            s.window
                .0
                .set_outer_position(tao::dpi::PhysicalPosition::new(p.x, p.y));
        }
        shot
    }
}

#[cfg(target_os = "linux")]
fn capture_x11(w: u32, h: u32) -> Result<Vec<u8>, String> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConnectionExt, ImageFormat};
    let (conn, screen) = x11rb::connect(None).map_err(|e| e.to_string())?;
    let root = conn.setup().roots[screen].root;
    // clamp the rect to the screen — GetImage BadMatches past the edge
    let sw = u32::from(conn.setup().roots[screen].width_in_pixels);
    let sh = u32::from(conn.setup().roots[screen].height_in_pixels);
    let w = w.min(sw);
    let h = h.min(sh);
    let img = conn
        .get_image(ImageFormat::Z_PIXMAP, root, 0, 0, w as u16, h as u16, !0u32)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    let data = img.data;
    let px = (w as usize) * (h as usize);
    if data.len() < px * 4 {
        return Err("short X image".into());
    }
    // ZPixmap on 24/32-bpp little-endian X servers is BGRX per pixel
    let mut rgba = vec![0u8; px * 4];
    for i in 0..px {
        rgba[i * 4] = data[i * 4 + 2];
        rgba[i * 4 + 1] = data[i * 4 + 1];
        rgba[i * 4 + 2] = data[i * 4];
        rgba[i * 4 + 3] = 255;
    }
    encode_png(&rgba, w, h)
}

#[cfg(target_os = "windows")]
fn window_hwnd(window: &tao::window::Window) -> Result<isize, String> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let raw = window.window_handle().map_err(|e| e.to_string())?.as_raw();
    match raw {
        RawWindowHandle::Win32(h) => Ok(h.hwnd.get() as isize),
        _ => Err("not a win32 window".into()),
    }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn enum_child(hwnd: windows_sys::Win32::Foundation::HWND, lparam: isize) -> i32 {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;
    let p = lparam as *mut (isize, i64);
    let mut rc: RECT = std::mem::zeroed();
    unsafe { GetWindowRect(hwnd, &mut rc) };
    let area = ((rc.right - rc.left) as i64) * ((rc.bottom - rc.top) as i64);
    if area > (*p).1 {
        (*p).0 = hwnd as isize;
        (*p).1 = area;
    }
    1
}

#[cfg(target_os = "windows")]
fn capture_gdi(window: &tao::window::Window, w: u32, h: u32) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GdiFlush, ReleaseDC,
        SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use windows_sys::Win32::Storage::Xps::PrintWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumChildWindows, GetWindowRect, PW_RENDERFULLCONTENT,
    };

    unsafe {
        let parent = window_hwnd(window)?;
        // PW_RENDERFULLCONTENT on the parent may not reach the DirectComposition
        // surface of the WebView2 render widget — prefer the largest child.
        let mut best: (isize, i64) = (0, 0);
        EnumChildWindows(parent as HWND, Some(enum_child), &mut best as *mut _ as isize);
        let target = if best.0 != 0 { best.0 } else { parent };

        let hdc = GetDC(parent as HWND);
        if hdc.is_null() {
            return Err("GetDC failed".into());
        }
        let mem = CreateCompatibleDC(hdc);
        let mut bih: BITMAPINFOHEADER = std::mem::zeroed();
        bih.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bih.biWidth = w as i32;
        bih.biHeight = -(h as i32); // top-down
        bih.biPlanes = 1;
        bih.biBitCount = 32;
        bih.biCompression = BI_RGB;
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader = bih;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let hbmp = CreateDIBSection(mem, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if hbmp.is_null() {
            DeleteDC(mem);
            ReleaseDC(parent as HWND, hdc);
            return Err("CreateDIBSection failed".into());
        }
        let oldbmp = SelectObject(mem, hbmp);
        let ok = PrintWindow(target as HWND, mem, PW_RENDERFULLCONTENT);
        GdiFlush();
        if ok == 0 {
            SelectObject(mem, oldbmp);
            DeleteObject(hbmp);
            DeleteDC(mem);
            ReleaseDC(parent as HWND, hdc);
            return Err("PrintWindow failed (PW_RENDERFULLCONTENT)".into());
        }
        let out = finish_gdi(bits, w, h);
        SelectObject(mem, oldbmp);
        DeleteObject(hbmp);
        DeleteDC(mem);
        ReleaseDC(parent as HWND, hdc);
        out
    }
}

#[cfg(target_os = "windows")]
unsafe fn finish_gdi(bits: *mut core::ffi::c_void, w: u32, h: u32) -> Result<Vec<u8>, String> {
    if bits.is_null() {
        return Err("no DIB bits".into());
    }
    let px = (w as usize) * (h as usize);
    let data = std::slice::from_raw_parts(bits as *const u8, px * 4);
    // 32-bpp BI_RGB DIB memory order is BGRX
    let mut rgba = vec![0u8; px * 4];
    for i in 0..px {
        rgba[i * 4] = data[i * 4 + 2];
        rgba[i * 4 + 1] = data[i * 4 + 1];
        rgba[i * 4 + 2] = data[i * 4];
        rgba[i * 4 + 3] = 255; // DIB alpha is often 0 for opaque windows
    }
    encode_png(&rgba, w, h)
}

// ---------- Event loop (main thread)

// Folded extraction with an IN-PAGE retry: the JS re-runs itself up to 10
// times (300 ms apart) until the page reports content, then posts through
// the ipc shim. Works identically on WebView2 and WebKitGTK — no callback
// involvement, no main-thread blocking.
pub fn fold_js_for(after: &str) -> String {
    // `after` is a complete JS EXPRESSION (an IIFE, or a plain
    // JSON.stringify(...)). Substitute it directly — wrapping it in another
    // function would discard its return value and JSON.parse(undefined).
    // A non-string result is stringified so the parse below always gets text.
    format!(
        "(function(){{ var attempt = 0; var run = () => {{ try {{ var r = ({js}); if (typeof r !== 'string') {{ r = JSON.stringify(r); }} var o = JSON.parse(r); if (o && (o.c || o.t)) {{ window.ipc.postMessage('__navette_fold:' + r); return; }} throw 'empty'; }} catch(e) {{ if (attempt < 10) {{ attempt++; setTimeout(run, 300); }} else {{ window.ipc.postMessage('__navette_fold:' + JSON.stringify({{t:document.title,c:'',__nav_error:String(e)}})); }} }} }}; run(); }})()",
        js = after
    )
}

pub enum Command {
    GetOrCreate(String, SyncSender<SessionRef>),
    Navigate(String, String),
    EvalJs(String, String),
    List(SyncSender<Value>),
    Close(String),
    Viewport(String, u32, u32, SyncSender<Result<(), String>>),
    ExportCookies(String, SyncSender<Result<Value, String>>),
    ImportCookies(String, Value, SyncSender<Result<usize, String>>),
    Screenshot(String, SyncSender<Result<Vec<u8>, String>>),
    Show(String, SyncSender<Result<(), String>>),
    Hide(String, SyncSender<Result<(), String>>),
    Reap(u64, SyncSender<usize>),
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Command")
    }
}

fn handle_command(cmd: Command, target: &tao::event_loop::EventLoopWindowTarget<Command>) {
    eprintln!("[navette][dbg] command received");
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
        Command::EvalJs(name, script) => {
            let map = sessions().lock().unwrap();
            if let Some(s) = map.get(&name) {
                if let Some(wv) = s.webview_slot.lock().unwrap().as_ref() {
                    let _ = wv.0.evaluate_script(&script);
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
        Command::Viewport(name, w, h, tx) => {
            let r = (|| -> Result<(), String> {
                let s = sessions().lock().unwrap().get(&name).cloned().ok_or("no such session")?;
                // Take the webview out of the slot for the duration: wry's
                // set_bounds runs on the UI thread and some paths pump nested
                // events — nested handlers must see None and skip, not deadlock.
                let wv = s.webview_slot.lock().unwrap().take();
                let Some(wv) = wv else { return Err("session not ready".into()) };
                let r = wv
                    .0
                    .set_bounds(wry::Rect {
                        position: wry::dpi::LogicalPosition::new(0.0, 0.0).into(),
                        size: wry::dpi::LogicalSize::new(f64::from(w), f64::from(h)).into(),
                    })
                    .map_err(|e| e.to_string());
                if r.is_ok() {
                    s.window
                        .0
                        .set_inner_size(tao::dpi::LogicalSize::new(f64::from(w), f64::from(h)));
                }
                *s.webview_slot.lock().unwrap() = Some(wv);
                r
            })();
            let _ = tx.send(r);
        }
        Command::ExportCookies(name, tx) => {
            let r = (|| -> Result<Value, String> {
                let s = sessions().lock().unwrap().get(&name).cloned().ok_or("no such session")?;
                let wv = s.webview_slot.lock().unwrap().take().ok_or("session not ready")?;
                let r = cookies_json(&wv);
                *s.webview_slot.lock().unwrap() = Some(wv);
                r
            })();
            let _ = tx.send(r);
        }
        Command::ImportCookies(name, cookies, tx) => {
            let r = (|| -> Result<usize, String> {
                let s = sessions().lock().unwrap().get(&name).cloned().ok_or("no such session")?;
                let wv = s.webview_slot.lock().unwrap().take().ok_or("session not ready")?;
                let r = cookies_from_json(&wv, &cookies);
                *s.webview_slot.lock().unwrap() = Some(wv);
                r
            })();
            let _ = tx.send(r);
        }
        Command::Screenshot(name, tx) => {
            let r = (|| -> Result<Vec<u8>, String> {
                let s = sessions().lock().unwrap().get(&name).cloned().ok_or("no such session")?;
                screenshot_on_main(&s)
            })();
            let _ = tx.send(r);
        }
        // Human-visible window: decorations on, centered, focused — for
        // one-time logins a human completes inside the session.
        Command::Show(name, tx) => {
            let r = (|| -> Result<(), String> {
                let s = sessions().lock().unwrap().get(&name).cloned().ok_or("no such session")?;
                s.window.0.set_decorations(true);
                // Center on the current monitor (physical pixels on wry).
                // tao's current_monitor() returns Option<MonitorHandle>.
                if let Some(mon) = s.window.0.current_monitor() {
                    let m = mon.size();
                    let w = s.window.0.inner_size();
                    let x = ((m.width as i32 - w.width as i32) / 2).max(0);
                    let y = ((m.height as i32 - w.height as i32) / 2).max(0);
                    let _ = s.window
                        .0
                        .set_outer_position(tao::dpi::PhysicalPosition::new(x, y));
                }
                s.window.0.set_visible(true);
                s.window.0.set_focus();
                Ok(())
            })();
            let _ = tx.send(r);
        }
        Command::Hide(name, tx) => {
            let r = (|| -> Result<(), String> {
                let s = sessions().lock().unwrap().get(&name).cloned().ok_or("no such session")?;
                s.window.0.set_visible(false);
                Ok(())
            })();
            let _ = tx.send(r);
        }
        Command::Reap(max_idle_secs, tx) => {
            // Idle watchdog: drop whole sessions (window + WebKit processes
            // follow) that no HTTP request has touched for max_idle_secs.
            // The next request re-creates and re-pre-warms on demand.
            let now = std::time::Instant::now();
            let idle: Vec<String> = sessions()
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, s)| now.duration_since(*s.last_used.lock().unwrap()).as_secs() >= max_idle_secs)
                .map(|(n, _)| n.clone())
                .collect();
            for name in &idle {
                let removed = sessions().lock().unwrap().remove(name);
                if let Some(s) = removed {
                    let _ = s.window.0.set_visible(false);
                }
            }
            if !idle.is_empty() {
                eprintln!("[navette] idle-release: dropped {} session(s) (idle >= {}s)", idle.len(), max_idle_secs);
            }
            let _ = tx.send(idle.len());
        }
    }
}

fn create_session(
    name: &str,
    target: &tao::event_loop::EventLoopWindowTarget<Command>,
) -> Result<SessionRef, String> {
    eprintln!("[navette][dbg] create_session: building window");
    let window = WindowBuilder::new()
        .with_title(format!("navette — {name}"))
        .with_decorations(false)
        .with_inner_size(tao::dpi::LogicalSize::new(WIDTH, HEIGHT))
        .with_position(tao::dpi::Position::Logical(tao::dpi::LogicalPosition::new(
            -WIDTH - 120.0,
            60.0,
        )))
        .build(target)
        .expect("ghost window");
    let window = GhostWindow(window);
    eprintln!("[navette][dbg] create_session: window built");

    let evals: Arc<Mutex<HashMap<u64, SyncSender<String>>>> = Arc::new(Mutex::new(HashMap::new()));
    let pending: Arc<Mutex<Option<PendingNav>>> = Arc::new(Mutex::new(None));
    let fold_tx: Arc<Mutex<Option<SyncSender<Result<String, String>>>>> =
        Arc::new(Mutex::new(None));
    let current_url = Arc::new(Mutex::new(String::new()));
    let current_title = Arc::new(Mutex::new(String::new()));
    let webview_slot: Arc<Mutex<Option<GhostWebView>>> = Arc::new(Mutex::new(None));

    // IPC: eval results (__navette_rpc id:...) and folded navigate results
    // (__navette_fold:...) both arrive here, on the main thread.
    let ipc_evals = evals.clone();
    let ipc_fold = fold_tx.clone();
    let ipc_title = current_title.clone();
    let pl_pending = pending.clone();
    let pl_fold = fold_tx.clone();
    let pl_url = current_url.clone();
    let pl_slot = webview_slot.clone();

    let mut builder = wry::WebViewBuilder::new();
    if let Some((proxy, ua)) = AGENT_OPTS.get() {
        if let Some(p) = proxy {
            match parse_proxy(p) {
                Ok(cfg) => builder = builder.with_proxy_config(cfg),
                Err(e) => eprintln!("[navette] proxy ignored: {e}"),
            }
        }
        if let Some(ua) = ua {
            builder = builder.with_user_agent(ua.clone());
        }
    }
    let webview = builder
        .with_url("about:blank")
        .with_initialization_script(RPC_SHIM)
        .with_ipc_handler(move |req: wry::http::Request<String>| {
            let body = req.body().to_string();
            if let Some(rest) = body.strip_prefix("__navette_dialog:") {
                // Dialogs are auto-handled in-page by RPC_SHIM (alert no-ops,
                // confirm accepts, prompt returns its default) — log and move on.
                eprintln!("[navette][dialog] {rest}");
                return;
            }
            if let Some(folded) = body.strip_prefix("__navette_fold:") {
                // Folded navigate completion: value = {"t":…,"c":…}. The route's
                // tx lives in fold_tx — the pending slot was already consumed by
                // the URL-matched Finished that started this fold.
                let v: Value = serde_json::from_str(folded).unwrap_or(json!({}));
                *ipc_title.lock().unwrap() = v
                    .get("t")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                if let Some(tx) = ipc_fold.lock().unwrap().take() {
                    let _ = tx.send(Ok(folded.to_string()));
                }
                return;
            }
            if let Some((id_str, value)) = body.split_once(':') {
                if let Ok(id) = id_str.parse::<u64>() {
                    let value = if value == "null" { String::new() } else { value.to_string() };
                    if let Some(tx) = ipc_evals.lock().unwrap().remove(&id) {
                        let _ = tx.send(value);
                    }
                }
            }
        })
        .with_on_page_load_handler(
            move |event: wry::PageLoadEvent, url: String| {
                if !matches!(event, wry::PageLoadEvent::Finished) {
                    return;
                }
                *pl_url.lock().unwrap() = url.clone();
                // URL-matched completion: the initial about:blank Finished (or
                // any other in-flight load) must not swallow a pending navigate
                // aimed at another URL — the CI cold start hits exactly that race.
                let matched = {
                    let mut g = pl_pending.lock().unwrap();
                    match g.as_ref() {
                        Some(p) if p.url == url.trim_end_matches('/') => g.take(),
                        _ => None,
                    }
                };
                let Some(p) = matched else {
                    eprintln!("[navette][dbg] page-load Finished (no matching pending): {url}");
                    return;
                };
                eprintln!("[navette][dbg] page-load Finished matched: {url} — folding");
                let js = p
                    .after
                    .clone()
                    .unwrap_or_else(|| "JSON.stringify({t:document.title,c:''})".to_string());
                *pl_fold.lock().unwrap() = Some(p.tx);
                let guard = pl_slot.lock().unwrap();
                let Some(wv) = guard.as_ref() else { return };
                let _ = wv.0.evaluate_script(&fold_js_for(&js));
            },
        )
        .build_as_child(&window.0)
        .map_err(|e| e.to_string())?;
    eprintln!("[navette][dbg] create_session: webview built");

    *webview_slot.lock().unwrap() = Some(GhostWebView(webview));
    SLOTS.get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap()
        .push(webview_slot.clone());
    eprintln!("[navette][dbg] create_session: session ready");

    Ok(Arc::new(Session {
        name: name.to_string(),
        window,
        webview_slot,
        evals,
        pending,
        fold_tx,
        last_used: Mutex::new(std::time::Instant::now()),
        current_url,
        current_title,
    }))
}

// The page-load handler needs the freshly built webview; the slot registry is
// the bridge (one session in practice — the default).
static SLOTS: OnceLock<Mutex<Vec<Arc<Mutex<Option<GhostWebView>>>>>> = OnceLock::new();

fn ipc_webview_slot() -> Option<Arc<Mutex<Option<GhostWebView>>>> {
    let map = SLOTS.get_or_init(|| Mutex::new(Vec::new()));
    map.lock().unwrap().last().cloned()
}

pub fn run_main_loop() {
    // The WORKING pattern (verified in CI to +10 s of successful navigate):
    // build the loop, expose the proxy, start the listener, then run — all on
    // the main thread. Splitting init/run across main-thread calls deadlocks
    // the GTK/Win32 loop start.
    let event_loop = EventLoopBuilder::<Command>::with_user_event().build();
    let _ = PROXY.set(event_loop.create_proxy());
    crate::start_listener(crate::PORT.get().copied().unwrap_or(8765));
    // Pre-warm the default session like the macOS backend does: WebView2's
    // first Environment+Controller creation can take ~30 s on a GPU-less CI
    // VM — it must not be paid inside the first navigate's 30 s timeout.
    {
        let (tx, _rx) = std::sync::mpsc::sync_channel(1);
        let _ = event_loop
            .create_proxy()
            .send_event(Command::GetOrCreate("default".to_string(), tx));
        eprintln!("[navette] pre-warm requested (default session)");
    }
    eprintln!("[navette][dbg] event loop running");
    event_loop.run(move |event, target, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::UserEvent(cmd) = event {
            handle_command(cmd, target);
        }
    });
}
