// backend_stub — non-macOS placeholder. The 8 primitives are engine-agnostic;
// the WebView2 (windows-rs) and WebKitGTK/WPE backends plug in here behind the
// exact same surface. See ONEPAGER.md "Engine strategy".

use serde_json::{json, Value};
use std::sync::Arc;

pub struct Session;

pub type SessionRef = Arc<Session>;

pub fn get_or_create(_name: &str) -> SessionRef {
    Arc::new(Session)
}

pub fn run_get_or_create(_name: &str) -> SessionRef {
    Arc::new(Session)
}

pub fn prewarm_default() {}

pub fn list_sessions() -> Value {
    json!({ "sessions": [] })
}

pub fn close_session(_name: &str) {}

pub fn load_url(_s: &SessionRef, _url: &str) -> Result<(), String> {
    Err("platform not supported yet — macOS backend only for now".into())
}

pub fn eval_js(_s: &SessionRef, _js: &str) -> Result<String, String> {
    Err("platform not supported yet".into())
}

pub fn screenshot(_s: &SessionRef) -> Result<Vec<u8>, String> {
    Err("platform not supported yet".into())
}

pub fn export_cookies(_s: &SessionRef) -> Result<Value, String> {
    Err("platform not supported yet".into())
}

pub fn import_cookies(_s: &SessionRef, _cookies: &Value) -> Result<usize, String> {
    Err("platform not supported yet".into())
}

pub fn set_agent_options(_proxy: Option<String>, _user_agent: Option<String>) {}

pub fn native_key(_s: &SessionRef, _key: &str) -> Result<(), String> {
    Err("no native input on this platform".into())
}

pub fn reap_idle(_max_idle_secs: u64) -> usize {
    0
}

pub fn set_viewport(_s: &SessionRef, _width: u32, _height: u32) -> Result<(), String> {
    Err("platform not supported yet".into())
}

pub fn show_window(_s: &SessionRef) -> Result<(), String> {
    Err("platform not supported yet".into())
}

pub fn hide_window(_s: &SessionRef) -> Result<(), String> {
    Err("platform not supported yet".into())
}

pub fn wait_settle(_s: &SessionRef) {}
