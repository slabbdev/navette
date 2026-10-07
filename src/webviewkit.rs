// webviewkit — the engine-agnostic contract every backend must satisfy.
//
// navette is a thin product layer (HTTP + MCP) over a kit of webview
// primitives. Each platform backend — backend_macos (WKWebView via objc2),
// backend_wry (WebView2 / WebKitGTK via wry+tao), and any future hand-rolled
// backend (e.g. a direct WebKitGTK or WebView2 FFI layer replacing wry) —
// implements this trait so the COMPILER enforces surface parity: a backend
// that misses a primitive, or drifts on a signature, does not build.
//
// The kit is session-typed: a backend owns its Session struct; the product
// layer only ever holds `Self::SessionRef`. JS execution, cookie state,
// screenshots and native input are all the backend's business — the HTTP
// layer above never touches an engine handle directly.
//
// Deliberately outside the trait: boot lifecycle. It genuinely differs per
// platform (macOS: app_init then app_run on the main thread; wry backends:
// run_main_loop owns the tao event loop and never returns) and stays a free
// function per backend module.
//
// Adding a primitive? Extend this trait first — every backend fails to build
// until it catches up. That asymmetry is the point.

use serde_json::Value;

pub trait WebviewKit: 'static {
    /// The backend's session handle (in practice `Arc<Session>`), safe to
    /// hold and call from any HTTP thread.
    type SessionRef: Send + Sync + 'static;

    // ---------- Global

    /// Create-or-fetch the named session; updates idle bookkeeping.
    fn run_get_or_create(&self, name: &str) -> Self::SessionRef;

    /// Pre-warm the default session before the run loop starts.
    fn prewarm_default(&self);

    /// serve-time operator options: proxy URL + user-agent override.
    fn set_agent_options(&self, proxy: Option<String>, user_agent: Option<String>);

    /// Session inventory for GET /sessions (name, url, title).
    fn list_sessions(&self) -> Value;

    /// Drop one session (and its engine resources) by name.
    fn close_session(&self, name: &str);

    /// Idle watchdog: drop sessions idle for >= max_idle_secs, return how
    /// many. The daemon itself stays resident.
    fn reap_idle(&self, max_idle_secs: u64) -> usize;

    // ---------- Primitives

    /// Load `url`; when `after` is given, run that JS on completion and
    /// return its result (the navigate+read fold). Returns the final title.
    fn navigate(
        &self,
        s: &Self::SessionRef,
        url: &str,
        after: Option<String>,
    ) -> Result<String, String>;

    /// Evaluate a JS expression; returns its JSON-serialized value.
    fn eval_js(&self, s: &Self::SessionRef, js: &str) -> Result<String, String>;

    /// Full-viewport screenshot as PNG bytes.
    fn screenshot(&self, s: &Self::SessionRef) -> Result<Vec<u8>, String>;

    /// Logged-in state (cookies) as JSON — the storageState equivalent.
    fn export_cookies(&self, s: &Self::SessionRef) -> Result<Value, String>;

    /// Restore a cookie state previously exported; returns imported count.
    fn import_cookies(&self, s: &Self::SessionRef, cookies: &Value) -> Result<usize, String>;

    /// Set the viewport (default 1280x800).
    fn set_viewport(&self, s: &Self::SessionRef, width: u32, height: u32) -> Result<(), String>;

    /// Put the session's window ON SCREEN (one-time human logins).
    fn show_window(&self, s: &Self::SessionRef) -> Result<(), String>;

    /// Order the window back out; session and state untouched.
    fn hide_window(&self, s: &Self::SessionRef) -> Result<(), String>;

    /// Real OS-level key input where available; Err = caller falls back to
    /// the synthetic dispatch (the route reports which mode ran).
    fn native_key(&self, s: &Self::SessionRef, key: &str) -> Result<(), String>;

    /// After a click that may navigate: wait for the page to settle.
    fn wait_settle(&self, s: &Self::SessionRef);
}
