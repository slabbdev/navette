// backend_wry — Windows + Linux backend via wry/tao.
// One implementation for both OSes: WebView2 (Windows) and WebKitGTK (Linux)
// behind the same surface. The macOS backend (backend_macos.rs) is separate —
// it has the delegate-fold optimization, screenshots and cookie state.
//
// Threading: the tao event loop owns the main thread; HTTP threads send
// Commands through the EventLoopProxy and wait on channels (same shape as the
// macOS backend's run_on_main hops).
//
// Evaluation results flow back through the IPC shim (window.ipc.postMessage):
// no blocking of the main thread anywhere — the navigate completion and the
// folded content extraction ride the same IPC path.

use crate::jsonv;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::mpsc::SyncSender;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoop, EventLoopProxy};
use tao::window::{Window, WindowBuilder};
use wry::WebView;

pub const WIDTH: f64 = 1280.0;
pub const HEIGHT: f64 = 800.0;

// ---------- Session

pub struct Session {
    pub name: String,
    pub window: Window,
    // The handler closures (ipc / page-load) receive this slot and read the
    // webview from it after build() — builder-time circularity avoided.
    pub webview_slot: Arc<Mutex<Option<WebView>>>,
    pub evals: Arc<Mutex<HashMap<u64, SyncSender<String>>>>,
    pub pending: Arc<Mutex<Option<PendingNav>>>,
    pub current_url: Arc<Mutex<String>>,
    pub current_title: Arc<Mutex<String>>,
}

pub type SessionRef = Arc<Session>;

pub struct PendingNav {
    pub tx: SyncSender<Result<String, String>>,
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
        *p = Some(PendingNav { tx: tx.clone() });
    }
    s.current_url.lock().unwrap().clear();
    proxy()
        .send_event(Command::Navigate(
            s.name.clone(),
            url.to_string(),
            after,
            tx,
        ))
        .map_err(|_| "event loop gone")?;
    rx.recv_timeout(Duration::from_secs(30))
        .unwrap_or_else(|_| Err("navigation timeout".into()))
}

pub fn eval_js(s: &SessionRef, js: &str) -> Result<String, String> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let id = next_rpc_id();
    s.evals.lock().unwrap().insert(id, tx);
    proxy()
        .send_event(Command::EvalJs(s.name.clone(), format!(
            "window.__navette_rpc({id}, Promise.resolve((function(){{ try {{ return (function(){{ {js} }})(); }} catch(e) {{ return {{__nav_error: String(e)}} }}; }})()).then(v => v === undefined ? null : v))",
            id = id, js = js
        ), tx.clone()))
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

// ---------- Event loop (main thread)

const RPC_SHIM: &str = r#"window.__navette_rpc = (id, value) => { window.ipc.postMessage(id + ':' + JSON.stringify(value === undefined ? null : value)); };"#;

enum Command {
    GetOrCreate(String, SyncSender<SessionRef>),
    Navigate(String, String, Option<String>, SyncSender<Result<String, String>>),
    EvalJs(String, String, SyncSender<Result<String, String>>),
    List(SyncSender<Value>),
    Close(String, SyncSender<Value>),
}

static MAIN_LOOP: OnceLock<EventLoop<Command>> = OnceLock::new();

fn handle_command(cmd: Command) {
    match cmd {
        Command::GetOrCreate(name, tx) => {
            if let Some(s) = sessions().lock().unwrap().get(&name) {
                let _ = tx.send(s.clone());
                return;
            }
            match create_session(&name) {
                Ok(s) => {
                    sessions().lock().unwrap().insert(name.clone(), s.clone());
                    let _ = tx.send(s);
                }
                Err(e) => eprintln!("[navette] session '{}' creation failed: {e}", name),
            }
        }
        Command::Navigate(name, url, after, _tx) => {
            let map = sessions().lock().unwrap();
            if let Some(s) = map.get(&name) {
                // The pending completion was registered by navigate() before
                // this command; the page-load handler completes it.
                if let Err(e) = s.webview.load_url(&url) {
                    if let Some(p) = s.pending.lock().unwrap().take() {
                        let _ = p.tx.send(Err(e.to_string()));
                    }
                }
            } else if let Some(p) = _tx.take() {
                let _ = p.send(Err("no such session".into()));
            }
        }
        Command::EvalJs(name, script, _tx) => {
            let map = sessions().lock().unwrap();
            if let Some(s) = map.get(&name) {
                if let Some(wv) = s.webview_slot.lock().unwrap().as_ref() {
                    if let Err(e) = wv.evaluate_script(&script) {
                        // complete the pending eval with the error
                        if let Some((_id, tx)) = s.evals.lock().unwrap().iter().next().map(|(k, v)| (*k, v.clone())) {
                            let _ = tx.send(format!("{{\"__nav_error\":{}}}", serde_json::to_string(&e.to_string()).unwrap_or_default()));
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
        Command::Close(name, tx) => {
            let removed = sessions().lock().unwrap().remove(&name);
            if let Some(s) = removed {
                let _ = s.window.set_visible(false);
            }
            let _ = tx;
        }
    }
}

fn create_session(name: &str) -> Result<SessionRef, String> {
    let el = MAIN_LOOP
        .get()
        .ok_or("event loop not initialized")?
        .clone();
    let window = WindowBuilder::new()
        .with_title(format!("navette — {name}"))
        .with_inner_size(tao::dpi::LogicalSize::new(WIDTH, HEIGHT))
        .with_position(LogicalPosition::new(-WIDTH - 120.0, 60.0))
        .build(&el)
        .map_err(|e| e.to_string())?;

    let evals: Arc<Mutex<HashMap<u64, SyncSender<String>>>> = Arc::new(Mutex::new(HashMap::new()));
    let pending: Arc<Mutex<Option<PendingNav>>> = Arc::new(Mutex::new(None));
    let current_url = Arc::new(Mutex::new(String::new()));
    let current_title = Arc::new(Mutex::new(String::new()));
    let webview_slot: Arc<Mutex<Option<WebView>>> = Arc::new(Mutex::new(None));

    // IPC: eval results (__navette_rpc id:...) and folded navigate results
    // (__navette_fold:...) both arrive here, on the main thread.
    let ipc_evals = evals.clone();
    let ipc_pending = pending.clone();
    let ipc_url = current_url.clone();
    let ipc_title = current_title.clone();
    let ipc_webview_slot = webview_slot.clone();
    let ipc_name = name.to_string();

    let webview = wry::WebViewBuilder::new(&window)
        .with_url("about:blank")
        .map_err(|e| e.to_string())?
        .with_initialization_script(RPC_SHIM)
        .with_ipc_handler(Arc::new(move |req| {
            let body = req.body().to_string();
            if let Some(folded) = body.strip_prefix("__navette_fold:") {
                // Folded navigate completion: value = {"t":…,"c":…}
                *ipc_url.lock().unwrap() = {
                    let v: Value = serde_json::from_str(folded).unwrap_or(json!({}));
                    let t = v.get("t").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    *ipc_title.lock().unwrap() = t;
                    String::new() // url was captured at navigate time
                };
                if let Some(p) = ipc_pending.lock().unwrap().take() {
                    let _ = p.tx.send(Ok(folded.to_string()));
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
            let _ = ipc_webview_slot;
            let _ = ipc_name;
        }))
        .with_on_page_load_handler(Arc::new(move |event: wry::PageLoadEvent, url: String| {
            if matches!(event, wry::PageLoadEvent::Finished) {
                *ipc_url.lock().unwrap() = url;
                // Folded extraction: fire the metadata/content JS; its result
                // returns through the ipc handler (__navette_fold:).
                let wv_ok = ipc_webview_slot
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some();
                if wv_ok {
                    if let Some(p) = ipc_pending.lock().unwrap().as_ref() {
                        let js = p
                            .after
                            .clone()
                            .unwrap_or_else(|| "JSON.stringify({t:document.title,c:''})".to_string());
                        let id = next_rpc_id();
                        ipc_evals.lock().unwrap().insert(
                            id,
                            // the eval result is routed back as a fold message
                            {
                                let pending = ipc_pending.clone();
                                let tx = p.tx.clone();
                                std::sync::mpsc::sync_channel(1).1 // placeholder; unused
                            },
                        );
                        let wv_guard = ipc_webview_slot.lock().unwrap();
                        if let Some(wv) = wv_guard.as_ref() {
                            let _ = wv.evaluate_script(&format!(
                                "window.__navette_fold_post = (v) => window.ipc.postMessage('__navette_fold:' + JSON.stringify(v)); Promise.resolve((function(){{ try {{ return (function(){{ {js} }})(); }} catch(e) {{ return {{__nav_error: String(e)}} }}; }})()).then(v => window.__navette_fold_post(v === undefined ? {{t:'',c:''}} : v))",
                                js = js
                            ));
                        }
                        // NOTE: the eval result completes the pending nav via
                        // the fold path above; this eval has no id-based reply.
                    } else {
                        // plain navigation (no with_content): grab the title
                        let wv_guard = ipc_webview_slot.lock().unwrap();
                        if let Some(wv) = wv_guard.as_ref() {
                            let _ = wv.evaluate_script(
                                "window.ipc ? window.ipc.postMessage('__navette_fold:' + JSON.stringify({t:document.title,c:''})) : 0;",
                            );
                        }
                    }
                }
            }
        }))
        .build()
        .map_err(|e| e.to_string())?;

    *webview_slot.lock().unwrap() = Some(webview);

    Ok(Arc::new(Session {
        name: name.to_string(),
        window,
        webview_slot,
        evals,
        pending,
        current_url,
        current_title,
    }))
}

pub fn run_main_loop() {
    let event_loop = EventLoop::<Command>::with_user_event()
        .build()
        .expect("tao event loop");
    let _ = PROXY.set(event_loop.create_proxy());
    let _ = MAIN_LOOP.set(event_loop.clone());
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::UserEvent(cmd) = event {
            handle_command(cmd);
        }
    });
}
