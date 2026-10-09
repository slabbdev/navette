// keystore — session states in the OS keychain (issue #8, stage 1).
//
// A stored state IS a login: the cookie jar re-authenticates without a
// human, so it must not sit in a plaintext sidecar file. States live in the
// OS credential store — macOS Keychain, Windows Credential Manager, Linux
// Secret Service — under one service name, and the jar itself only ever
// moves inside the navette process: the HTTP surface passes a store NAME,
// never the secret. No plaintext fallback: a machine with no keychain gets
// a loud error naming the missing service.

use serde_json::{json, Value};

pub const SERVICE: &str = "navette";
const INDEX_KEY: &str = "_index";

/// True when a real OS credential store is compiled in. keyring silently
/// falls back to its in-memory mock when no platform feature is named —
/// a store that forgets on restart would break every promise this module
/// makes, so unsupported targets fail loudly instead.
const REAL_STORE_COMPILED: bool =
    cfg!(target_os = "macos") || cfg!(target_os = "windows") || cfg!(target_os = "linux");

fn entry(name: &str) -> Result<keyring::Entry, String> {
    if !REAL_STORE_COMPILED {
        return Err(
            "no OS keychain backend for this platform — session states cannot be stored safely \
             (build for macOS/Windows/Linux, which compile keyring's native store)"
                .into(),
        );
    }
    keyring::Entry::new(SERVICE, name).map_err(|e| format!("no usable OS keychain: {e}"))
}

// ---------- pure helpers (unit-tested; no keychain involved)

/// Store names become keychain account fields — keep them boring and
/// user-visible: 1–64 chars of [a-zA-Z0-9._-].
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || "-._".contains(c))
}

/// The secret stored per name: the jar verbatim plus the metadata stage 2
/// needs for origin-binding. `saved_at` is unix seconds.
pub fn wrap(origin: &str, jar: &Value) -> Value {
    let saved_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    json!({"v": 1, "origin": origin, "saved_at": saved_at, "jar": jar})
}

pub fn unwrap_secret(secret: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(secret)
        .map_err(|_| "stored state is not valid JSON (wrong service, or a corrupted entry)")?;
    if v.get("jar").and_then(|j| j.get("cookies")).is_none() {
        return Err("stored state carries no cookie jar".into());
    }
    Ok(v)
}

/// The index is its own keychain entry — names, origins, timestamps only,
/// never jars — because the keyring APIs expose no enumeration.
pub fn index_add(index: &mut Value, name: &str, origin: &str) {
    if !index.is_object() {
        *index = json!({"states": []});
    }
    if index.get("states").and_then(|s| s.as_array()).is_none() {
        index["states"] = json!([]);
    }
    let saved_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Some(arr) = index.get_mut("states").and_then(|s| s.as_array_mut()) {
        arr.retain(|e| e.get("name").and_then(|n| n.as_str()) != Some(name));
        arr.push(json!({"name": name, "origin": origin, "saved_at": saved_at}));
    }
}

pub fn index_remove(index: &mut Value, name: &str) {
    if let Some(arr) = index.get_mut("states").and_then(|s| s.as_array_mut()) {
        arr.retain(|e| e.get("name").and_then(|n| n.as_str()) != Some(name));
    }
}

// ---------- keychain operations

fn read_index() -> Result<Value, String> {
    match entry(INDEX_KEY)?.get_password() {
        Ok(s) => Ok(serde_json::from_str(&s).unwrap_or_else(|_| json!({"states": []}))),
        Err(keyring::Error::NoEntry) => Ok(json!({"states": []})),
        Err(e) => Err(format!("keychain read failed: {e}")),
    }
}

fn write_index(index: &Value) -> Result<(), String> {
    entry(INDEX_KEY)?
        .set_password(&index.to_string())
        .map_err(|e| format!("keychain write failed: {e}"))
}

/// Store `jar` (the full /sessions/state payload) under `name`.
pub fn save(name: &str, origin: &str, jar: &Value) -> Result<Value, String> {
    if !valid_name(name) {
        return Err(format!(
            "invalid store name {name:?} — use 1-64 chars of [a-zA-Z0-9._-]"
        ));
    }
    entry(name)?
        .set_password(&wrap(origin, jar).to_string())
        .map_err(|e| format!("keychain write failed: {e}"))?;
    let mut idx = read_index()?;
    index_add(&mut idx, name, origin);
    write_index(&idx)?;
    Ok(json!({"stored": name, "origin": origin}))
}

/// Fetch the wrapper stored under `name`.
pub fn load(name: &str) -> Result<Value, String> {
    let secret = entry(name)?.get_password().map_err(|e| match e {
        keyring::Error::NoEntry => format!("no stored state named {name:?} (see: navette state list)"),
        other => format!("keychain read failed: {other}"),
    })?;
    unwrap_secret(&secret)
}

/// `{"ok":true,"states":[{name, origin, saved_at}]}` — metadata only.
pub fn list() -> Result<Value, String> {
    let mut idx = read_index()?;
    if !idx.is_object() {
        idx = json!({"states": []});
    }
    idx["ok"] = json!(true);
    Ok(idx)
}

pub fn delete(name: &str) -> Result<Value, String> {
    match entry(name)?.delete_credential() {
        Ok(()) => {}
        Err(keyring::Error::NoEntry) => {}
        Err(e) => return Err(format!("keychain delete failed: {e}")),
    }
    let mut idx = read_index()?;
    index_remove(&mut idx, name);
    write_index(&idx)?;
    Ok(json!({"deleted": name}))
}

// ---------- stage 2: origin-bound credentials (#8) — set by a human, used
// by the agent, never readable back by anyone

const CRED_INDEX_KEY: &str = "_cred_index";
const CRED_PREFIX: &str = "cred:";

fn cred_entry(site: &str) -> Result<keyring::Entry, String> {
    if !valid_name(site) {
        return Err(format!(
            "invalid site name {site:?} — use 1-64 chars of [a-zA-Z0-9._-]"
        ));
    }
    entry(&format!("{CRED_PREFIX}{site}"))
}

pub fn wrap_cred(origin: &str, username: &str, password: &str) -> Value {
    let saved_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    json!({"v": 1, "origin": origin, "username": username, "password": password, "saved_at": saved_at})
}

pub fn unwrap_cred(secret: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(secret)
        .map_err(|_| "stored credential is not valid JSON".to_string())?;
    for k in ["origin", "username", "password"] {
        if v.get(k).and_then(|x| x.as_str()).is_none() {
            return Err(format!("stored credential carries no {k}"));
        }
    }
    Ok(v)
}

fn read_cred_index() -> Result<Value, String> {
    match entry(CRED_INDEX_KEY)?.get_password() {
        Ok(s) => Ok(serde_json::from_str(&s).unwrap_or_else(|_| json!({"creds": []}))),
        Err(keyring::Error::NoEntry) => Ok(json!({"creds": []})),
        Err(e) => Err(format!("keychain read failed: {e}")),
    }
}

fn write_cred_index(index: &Value) -> Result<(), String> {
    entry(CRED_INDEX_KEY)?
        .set_password(&index.to_string())
        .map_err(|e| format!("keychain write failed: {e}"))
}

/// Store the credential for `site`, bound to `origin`. The password only
/// ever travels in: hidden stdin → CLI → loopback HTTP → keychain. No route
/// returns it; there is no read-back API at all.
pub fn set_cred(site: &str, origin: &str, username: &str, password: &str) -> Result<Value, String> {
    cred_entry(site)?
        .set_password(&wrap_cred(origin, username, password).to_string())
        .map_err(|e| format!("keychain write failed: {e}"))?;
    let mut idx = read_cred_index()?;
    if !idx.is_object() {
        idx = json!({"creds": []});
    }
    if idx.get("creds").and_then(|c| c.as_array()).is_none() {
        idx["creds"] = json!([]);
    }
    let saved_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Some(arr) = idx.get_mut("creds").and_then(|c| c.as_array_mut()) {
        arr.retain(|e| e.get("site").and_then(|n| n.as_str()) != Some(site));
        arr.push(json!({"site": site, "origin": origin, "username": username, "saved_at": saved_at}));
    }
    write_cred_index(&idx)?;
    Ok(json!({"site": site, "origin": origin}))
}

pub fn load_cred(site: &str) -> Result<Value, String> {
    let secret = cred_entry(site)?.get_password().map_err(|e| match e {
        keyring::Error::NoEntry => format!(
            "no stored credential for {site:?} — a human saves one with: navette creds set {site}"
        ),
        other => format!("keychain read failed: {other}"),
    })?;
    unwrap_cred(&secret)
}

/// `{"ok":true,"creds":[{site, origin, username, saved_at}]}` — no passwords.
pub fn list_creds() -> Result<Value, String> {
    let mut idx = read_cred_index()?;
    if !idx.is_object() {
        idx = json!({"creds": []});
    }
    idx["ok"] = json!(true);
    Ok(idx)
}

pub fn delete_cred(site: &str) -> Result<Value, String> {
    match cred_entry(site)?.delete_credential() {
        Ok(()) => {}
        Err(keyring::Error::NoEntry) => {}
        Err(e) => return Err(format!("keychain delete failed: {e}")),
    }
    let mut idx = read_cred_index()?;
    if let Some(arr) = idx.get_mut("creds").and_then(|c| c.as_array_mut()) {
        arr.retain(|e| e.get("site").and_then(|n| n.as_str()) != Some(site));
    }
    write_cred_index(&idx)?;
    Ok(json!({"deleted": site}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn names_are_boring() {
        assert!(valid_name("dailydev"));
        assert!(valid_name("leboncoin-pro"));
        assert!(valid_name("a.b_c-1"));
        assert!(!valid_name(""));
        assert!(!valid_name("has space"));
        assert!(!valid_name("slash/in"));
        assert!(!valid_name("quotes\"here"));
        assert!(!valid_name(&"x".repeat(65)));
    }

    #[test]
    fn wrap_unwrap_roundtrip() {
        let jar = json!({"cookies": [{"name": "sid", "value": "s3cret"}]});
        let w = wrap("https://app.example", &jar);
        assert_eq!(w["v"], 1);
        assert_eq!(w["origin"], "https://app.example");
        let back = unwrap_secret(&w.to_string()).unwrap();
        assert_eq!(back["jar"]["cookies"][0]["name"], "sid");
    }

    #[test]
    fn unwrap_rejects_garbage() {
        assert!(unwrap_secret("not json").is_err());
        assert!(unwrap_secret("{\"nope\": 1}").is_err());
    }

    #[test]
    fn index_add_replaces_by_name() {
        let mut idx = json!({"states": []});
        index_add(&mut idx, "a", "https://a.example");
        index_add(&mut idx, "b", "https://b.example");
        index_add(&mut idx, "a", "https://a2.example");
        let states = idx["states"].as_array().unwrap();
        assert_eq!(states.len(), 2);
        // replace = remove + push: the re-added entry lands last
        assert_eq!(states[0]["name"], "b");
        assert_eq!(states[1]["origin"], "https://a2.example");
        index_remove(&mut idx, "a");
        assert_eq!(idx["states"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn index_survives_garbage_input() {
        let mut idx = json!("garbage");
        index_add(&mut idx, "a", "https://a.example");
        assert_eq!(idx["states"][0]["name"], "a");
    }

    #[test]
    fn cred_roundtrip_and_rejection() {
        let c = wrap_cred("https://login.example", "sam", "hunter2");
        let back = unwrap_cred(&c.to_string()).unwrap();
        assert_eq!(back["username"], "sam");
        assert_eq!(back["password"], "hunter2");
        assert_eq!(back["origin"], "https://login.example");
        assert!(unwrap_cred("{\"origin\":\"https://x\",\"username\":\"u\"}").is_err());
        assert!(unwrap_cred("garbage").is_err());
    }
}
