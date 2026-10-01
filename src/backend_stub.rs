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
