// backend_stub — placeholder for platforms without a backend yet. The kit
// contract (webviewkit.rs) forces this module to expose the exact same
// surface as the real backends, so a new engine plugs in without touching
// the product layer. See ONEPAGER.md "Engine strategy".

use crate::webviewkit::WebviewKit;
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

pub fn navigate(_s: &SessionRef, _url: &str, _after: Option<String>) -> Result<String, String> {
    Err("platform not supported yet".into())
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

// ---------- Kit contract (compile-checked parity — see webviewkit.rs)

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
