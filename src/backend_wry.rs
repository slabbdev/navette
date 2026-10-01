// backend_wry — Windows + Linux backend via wry/tao.
// One implementation for both OSes: WebView2 (Windows) and WebKitGTK (Linux)
// behind the same surface. The macOS backend (backend_macos.rs) is separate —
// it has the delegate-fold optimization, screenshots and cookie state.
//
// Threading: the tao event loop owns the main thread; HTTP threads send
// Commands through the EventLoopProxy and wait on channels (same shape as the
// macOS backend's run_on_main hops).
//
// Results flow back through wry's evaluate_script_with_callback: no blocking
// of the main thread anywhere — the navigate completion and the folded content
// extraction ride the same callback path.
//
// tao/wry GTK objects are !Send — they are wrapped in ghost handles that are
// only ever DEREFERENCED on the main thread (event loop + its scheduled
// blocks). The Send impls are the standard tao/wrio pattern for handles whose
// methods stay on the creating thread.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::mpsc::SyncSender;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
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
    // The handler closures (ipc / page-load) receive this slot and read the
    // webview from it after build() — builder-time circularity avoided.
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

fn sessions() -> &'static Mutex<HashMap<String, SessionRef>> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn proxy() -> &'static EventLoopProxy<Command> {
    PROXY.get().expect("event loop proxy not initialized")
}

fn next_rpc_id() -> u64 {
    RPC_ID.fetch_add(1, Ordering::SeqCst)
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
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    proxy().send_event(Command::Close(name.to_string(), tx)).ok();
    let _ = rx.recv();
}

pub fn navigate(s: &SessionRef, url: &str, after: Option<String>) -> Result<String, String> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    // Register the pending completion BEFORE the load so the Finished event
    // (fired on main) finds it.
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
    let id = next_rpc_id();
    s.evals.lock().unwrap().insert(id, tx);
    let script = eval_wrapper(id, js);
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

pub fn screenshot(_s: &SessionRef) -> Result<Vec<u8>, String> {
    Err("screenshots on Windows/Linux land in the next release — macOS ships them today".into())
}

pub fn export_cookies(_s: &SessionRef) -> Result<Value, String> {
    Err("cookie state on Windows/Linux lands in the next release".into())
}

pub fn import_cookies(_s: &SessionRef, _cookies: &Value) -> Result<usize, String> {
    Err("cookie state on Windows/Linux lands in the next release".into())
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

fn eval_wrapper(id: u64, js: &str) -> String {
    format!(
        "window.__navette_rpc({id}, Promise.resolve((function(){{ try {{ return (function(){{ {js} }})(); }} catch(e) {{ return {{__nav_error: String(e)}} }}; }})()).then(v => v === undefined ? null : v))",
        id = id,
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
        Command::EvalJs(name, script) => {
            let map = sessions().lock().unwrap();
            if let Some(s) = map.get(&name) {
                if let Some(wv) = s.webview_slot.lock().unwrap().as_ref() {
                    if let Err(e) = wv.0.evaluate_script(&script) {
                        // Complete the failing eval with an error marker.
                        let marker = format!("{{\"__nav_error\":{}}}", serde_json::to_string(&e.to_string()).unwrap_or_default());
                        if let Some((_id, tx)) = s.evals.lock().unwrap().iter().next().map(|(k, v)| (*k, v.clone())) {
                            let _ = tx.send(marker);
                            s.evals.lock().unwrap().remove(&_id);
                        }
                    }
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

    // IPC: eval results (__navette_rpc id:...) and folded navigate results
    // (__navette_fold:...) both arrive here, on the main thread.
    let ipc_evals = evals.clone();
    let ipc_pending = pending.clone();
    let ipc_url = current_url.clone();
    let ipc_title = current_title.clone();
    let ipc_webview_slot = webview_slot.clone();

    let builder = wry::WebViewBuilder::new()
        .with_url("about:blank")
        .with_initialization_script(RPC_SHIM)
        .with_on_page_load_handler(
            move |event: wry::PageLoadEvent, url: String| {
                if !matches!(event, wry::PageLoadEvent::Finished) {
                    return;
                }
                *ipc_url.lock().unwrap() = url;
                // Folded extraction: fire the metadata/content JS through the
                // ipc shim; the result completes the pending navigate without
                // ever blocking this handler (or the main thread).
                let wv = ipc_webview_slot.lock().unwrap().as_ref().map(|g| &g.0);
                let Some(wv) = wv else { return };
                let js = ipc_pending
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|p| p.after.clone().unwrap_or_else(|| {
                        "JSON.stringify({t:document.title,c:''})".to_string()
                    }))
                    .unwrap_or_else(|| "JSON.stringify({t:document.title,c:''})".to_string());
                let _ = wv.0.evaluate_script(&format!(
                    "window.__navette_rpc('__navette_fold', Promise.resolve((function(){{ try {{ return (function(){{ {js} }})(); }} catch(e) {{ return {{__nav_error: String(e)}} }}; }})()).then(v => v === undefined ? null : v))",
                    js = js
                ));
            },
        )
        .build()
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

pub fn run_main_loop() {
    let event_loop = EventLoopBuilder::<Command>::with_user_event()
        .build()
        .expect("tao event loop");
    let proxy = event_loop.create_proxy();
    let _ = PROXY.set(proxy);
    event_loop.run(move |event, target, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::UserEvent(cmd) = event {
            handle_command(cmd, target);
        }
    });
}
