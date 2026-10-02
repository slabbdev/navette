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
const RPC_SHIM: &str = r#"window.__navette_rpc = (id, value) => { window.ipc.postMessage(id + ':' + JSON.stringify(value === undefined ? null : value)); };"#;

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
    rx.recv_timeout(Duration::from_secs(30))
        .unwrap_or_else(|_| Err("navigation timeout".into()))
}

pub fn eval_js(s: &SessionRef, js: &str) -> Result<String, String> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let id = next_rpc_id();
    // The script POSTS its result through the ipc shim (the with_callback
    // path returns empty on WebKitGTK — the ipc path is the reliable one).
    let script = format!(
        "(function(){{ try {{ var r = JSON.stringify((function(){{ {js} }})()); window.ipc.postMessage('{id}:' + r); }} catch(e) {{ window.ipc.postMessage('{id}:' + JSON.stringify({{__nav_error: String(e)}})); }} }})()",
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

// Folded extraction with an IN-PAGE retry: the JS re-runs itself up to 10
// times (300 ms apart) until the page reports content, then posts through
// the ipc shim. Works identically on WebView2 and WebKitGTK — no callback
// involvement, no main-thread blocking.
pub fn fold_js_for(after: &str) -> String {
    format!(
        "(function(){{ var attempt = 0; var run = () => {{ try {{ var r = (function(){{ {js} }})(); var o = JSON.parse(r); if (o && (o.c || o.t)) {{ window.ipc.postMessage('__navette_fold:' + r); return; }} throw 'empty'; }} catch(e) {{ if (attempt < 10) {{ attempt++; setTimeout(run, 300); }} else {{ window.ipc.postMessage('__navette_fold:' + JSON.stringify({{t:document.title,c:'',__nav_error:String(e)}})); }} }} }}; run(); }})()",
        js = after
    )
}

pub enum Command {
    GetOrCreate(String, SyncSender<SessionRef>),
    Navigate(String, String),
    EvalJs(String, String),
    List(SyncSender<Value>),
    Close(String),
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
    }
}

fn create_session(
    name: &str,
    target: &tao::event_loop::EventLoopWindowTarget<Command>,
) -> Result<SessionRef, String> {
    eprintln!("[navette][dbg] create_session: building window");
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
    eprintln!("[navette][dbg] create_session: window built");

    let evals: Arc<Mutex<HashMap<u64, SyncSender<String>>>> = Arc::new(Mutex::new(HashMap::new()));
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
    let ipc_slot = webview_slot.clone();
    let pl_pending = pending.clone();
    let pl_url = current_url.clone();
    let pl_slot = webview_slot.clone();

    let webview = wry::WebViewBuilder::new()
        .with_url("about:blank")
        .with_initialization_script(RPC_SHIM)
        .with_ipc_handler(move |req: wry::http::Request<String>| {
            let body = req.body().to_string();
            if let Some(folded) = body.strip_prefix("__navette_fold:") {
                // Folded navigate completion: value = {"t":…,"c":…}
                let v: Value = serde_json::from_str(folded).unwrap_or(json!({}));
                *ipc_title.lock().unwrap() = v
                    .get("t")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
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
        })
        .with_on_page_load_handler(
            move |event: wry::PageLoadEvent, url: String| {
                if !matches!(event, wry::PageLoadEvent::Finished) {
                    return;
                }
                *pl_url.lock().unwrap() = url;
                let Some(p) = pl_pending.lock().unwrap().take() else { return };
                let guard = pl_slot.lock().unwrap();
                let Some(wv) = guard.as_ref() else { return };
                let js = p
                    .after
                    .clone()
                    .unwrap_or_else(|| "JSON.stringify({t:document.title,c:''})".to_string());
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
    eprintln!("[navette][dbg] event loop running");
    event_loop.run(move |event, target, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::UserEvent(cmd) = event {
            handle_command(cmd, target);
        }
    });
}
