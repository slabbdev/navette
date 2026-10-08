// navette — the browser for agents.
// One binary, two modes: `navette serve` (HTTP on loopback) and `navette mcp`
// (MCP stdio for agent hosts). The engine is the OS's native WebView;
// navette is only the glue. No Chromium, no download.

#[cfg(target_os = "macos")]
#[path = "backend_macos.rs"]
mod backend;
#[cfg(any(target_os = "windows", target_os = "linux"))]
#[path = "backend_wry.rs"]
mod backend;
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
#[path = "backend_stub.rs"]
mod backend;

mod mcp;
mod webviewkit;

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

// The active backend's kit handle — every engine call in the product layer
// goes through the WebviewKit trait (surface compile-enforced per backend).
// Only the boot lifecycle stays on backend::* free functions: it genuinely
// differs per platform (app_init/app_run vs run_main_loop).
static KIT: backend::Kit = backend::Kit;
use webviewkit::WebviewKit;

static T0: OnceLock<Instant> = OnceLock::new();
pub static PORT: OnceLock<u16> = OnceLock::new();
static TOKEN: OnceLock<String> = OnceLock::new();

#[cfg(unix)]
extern "C" fn graceful_term(_sig: i32) {
    std::process::exit(0);
}

#[cfg(unix)]
fn install_graceful_signals() {
    unsafe {
        libc::signal(libc::SIGTERM, graceful_term as usize);
        libc::signal(libc::SIGINT, graceful_term as usize);
    }
}
static LOGGED_FIRST_NAV: AtomicBool = AtomicBool::new(false);

// MARK: - JS snippets (shared across backends; keep in sync with the reference)

fn click_js(sel: &str) -> String {
    format!(
        r#"(function(sel){{
  const el = document.querySelector(sel);
  if (!el) return 'MISSING';
  const r = el.getBoundingClientRect();
  const base = {{bubbles:true, cancelable:true, view:window, clientX:r.x + r.width/2, clientY:r.y + r.height/2}};
  const po = Object.assign({{pointerId:1, pointerType:'mouse', isPrimary:true, width:1, height:1, pressure:0}}, base);
  // Pointer events first: Radix/HeadlessUI menus listen for pointerdown and
  // never see a bare mousedown (found the hard way on daily.dev's kebab menu).
  el.dispatchEvent(new PointerEvent('pointerover', po));
  el.dispatchEvent(new PointerEvent('pointerdown', po));
  el.dispatchEvent(new MouseEvent('mousedown', base));
  el.dispatchEvent(new PointerEvent('pointerup', po));
  el.dispatchEvent(new MouseEvent('mouseup', base));
  if (el instanceof HTMLElement) {{ el.click(); }} else {{ el.dispatchEvent(new MouseEvent('click', base)); }}
  return 'OK';
}})({sel})"#
    )
}

fn type_js(sel: &str, val: &str) -> String {
    format!(
        r#"(function(sel, val){{
  const el = document.querySelector(sel);
  if (!el) return 'MISSING';
  el.focus();
  const proto = (el instanceof HTMLTextAreaElement) ? HTMLTextAreaElement.prototype
              : (el instanceof HTMLInputElement)  ? HTMLInputElement.prototype : null;
  if (proto) {{
    const d = Object.getOwnPropertyDescriptor(proto, 'value');
    if (d && d.set) {{ d.set.call(el, val); }} else {{ el.value = val; }}
  }} else {{ el.textContent = val; }}
  el.dispatchEvent(new Event('input', {{bubbles:true}}));
  el.dispatchEvent(new Event('change', {{bubbles:true}}));
  return 'OK';
}})({sel}, {val})"#,
        sel = sel,
        val = val
    )
}

pub const MARKDOWN_JS: &str = r#"(function(){
  document.querySelectorAll('script,style,noscript,svg,iframe,template').forEach(e => e.remove());
  const txt = e => (e.innerText || '').replace(/\s+/g, ' ').trim();
  const out = [];
  const walk = el => {
    for (const n of el.children) {
      const t = n.tagName.toLowerCase();
      if (t === 'h1') out.push('\n# ' + txt(n));
      else if (t === 'h2') out.push('\n## ' + txt(n));
      else if (t === 'h3') out.push('\n### ' + txt(n));
      else if (t === 'h4' || t === 'h5' || t === 'h6') out.push('\n#### ' + txt(n));
      else if (t === 'p') out.push('\n' + txt(n));
      else if (t === 'li') out.push('- ' + txt(n));
      else if (t === 'pre') out.push('\n```\n' + n.innerText + '\n```');
      else if (t === 'blockquote') out.push('> ' + txt(n));
      else if (t === 'a' && n.getAttribute('href')) out.push('[' + txt(n) + '](' + n.href + ')');
      else if (n.children.length) walk(n);
      else if (txt(n)) out.push(txt(n));
    }
  };
  walk(document.body);
  return out.join('\n').replace(/\n{3,}/g, '\n\n').trim();
})()"#;

fn combined_content_js(format: &str) -> String {
    match format {
        "text" => "(function(){return JSON.stringify({t:document.title,c:document.body.innerText});})()".into(),
        _ => format!(
            "(function(){{return JSON.stringify({{t:document.title,c:({MD})}});}})()",
            MD = MARKDOWN_JS
        ),
    }
}

fn jstr(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Constant-time string equality for token checks — no early exit on the
/// first differing byte. (Length still leaks via iteration count, the
/// standard accepted residual.)
fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let n = a.len().max(b.len());
    let mut acc = 0u8;
    for i in 0..n {
        acc |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0);
    }
    acc == 0
}

/// Proxy URLs may carry credentials (http://user:pass@host) — never log them.
fn redact_proxy(u: &str) -> String {
    match u.split_once("://") {
        Some((scheme, rest)) => match rest.rsplit_once('@') {
            Some((_, tail)) => format!("{scheme}://***@{tail}"),
            None => u.to_string(),
        },
        None => u.to_string(),
    }
}

// MARK: - HTTP plumbing

struct Req {
    method: String,
    path: String,
    body: Vec<u8>,
    headers: Vec<(String, String)>,
}

fn parse_request(data: &[u8]) -> Option<Req> {
    let idx = data.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&data[..idx]).to_string();
    let mut lines = head.split("\r\n");
    let first = lines.next()?.to_string();
    let mut cl = 0usize;
    let mut headers: Vec<(String, String)> = Vec::new();
    for l in lines {
        let mut kv = l.splitn(2, ':');
        if let (Some(k), Some(v)) = (kv.next(), kv.next()) {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
            if k.trim().eq_ignore_ascii_case("content-length") {
                cl = v.trim().parse().unwrap_or(0);
            }
        }
    }
    let body_start = idx + 4;
    if data.len() < body_start + cl {
        return None;
    }
    let mut parts = first.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    Some(Req {
        method,
        path,
        body: data[body_start..body_start + cl].to_vec(),
        headers,
    })
}

fn respond(fd: &mut TcpStream, status: u16, reason: &str, content_type: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ct}\r\nContent-Length: {n}\r\nConnection: close\r\n\r\n",
        status = status,
        reason = reason,
        ct = content_type,
        n = body.len()
    );
    let _ = fd.write_all(head.as_bytes());
    let _ = fd.write_all(body);
    let _ = fd.flush();
    let _ = fd.shutdown(std::net::Shutdown::Both);
}

fn json_bytes(v: &Value) -> Vec<u8> {
    serde_json::to_vec_pretty(v).unwrap_or_else(|_| b"{}".to_vec())
}

fn err_data(msg: &str) -> Value {
    json!({ "ok": false, "error": msg })
}

fn serve_fd(mut stream: TcpStream) {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 65536];
    loop {
        if let Some(req) = parse_request(&buf) {
            route(&mut stream, req);
            return;
        }
        if buf.len() > 10_000_000 {
            return;
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
}

// MARK: - Routes

fn route(fd: &mut TcpStream, req: Req) {
    // Optional auth (--token): every route except /health requires
    // `Authorization: Bearer <secret>` or `X-Navette-Token: <secret>`.
    // DNS-rebinding hardening: this API is loopback-only; a public site
    // that rebinds its DNS to 127.0.0.1 must not reach it. Only requests
    // addressed to a local Host are served (browsers' Private Network
    // Access blocks most of this already — this is the belt to that).
    let host_local = req
        .headers
        .iter()
        .find(|(k, _)| k == "host")
        .map(|(_, v)| {
            let h = v.split(':').next().unwrap_or("").trim();
            h == "127.0.0.1" || h == "localhost" || h == "[::1]"
        })
        .unwrap_or(false);
    let path_ok = req.path.split('?').next() == Some("/health");
    if let Some(tok) = TOKEN.get() {
        let presented = req
            .headers
            .iter()
            .find(|(k, _)| k == "authorization" || k == "x-navette-token")
            .map(|(_, v)| {
                let v = v.trim();
                v.strip_prefix("Bearer ").unwrap_or(v)
            });
        if !path_ok && !presented.is_some_and(|p| ct_eq(p, tok.as_str())) {
            respond(
                fd,
                401,
                "Unauthorized",
                "application/json",
                &json_bytes(&err_data("missing or invalid token — pass the --token value via Authorization: Bearer")),
            );
            return;
        }
    }
    if !path_ok && !host_local {
        respond(
            fd,
            403,
            "Forbidden",
            "application/json",
            &json_bytes(&err_data("non-local Host header — this API is loopback-only (DNS-rebinding guard)")),
        );
        return;
    }

    let path = req.path.split('?').next().unwrap_or("/").to_string();
    let j: Value = serde_json::from_slice(&req.body).unwrap_or(json!({}));
    let name = j.get("session").and_then(|v| v.as_str()).unwrap_or("default").to_string();

    match (req.method.as_str(), path.as_str()) {
        ("GET", "/health") => {
            respond(fd, 200, "OK", "application/json",
                    &json_bytes(&json!({"ok": true, "name": "navette", "engine": "system WebView"})));
        }

        ("GET", "/sessions") => {
            respond(fd, 200, "OK", "application/json", &json_bytes(&KIT.list_sessions()));
        }

        ("POST", "/sessions/close") => {
            KIT.close_session(&name);
            respond(fd, 200, "OK", "application/json", &json_bytes(&json!({"ok": true})));
        }

        ("POST", "/navigate") => {
            let url = j.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if url.is_empty() {
                respond(fd, 400, "Bad Request", "application/json", &json_bytes(&err_data("bad url")));
                return;
            }
            let with_content = j.get("with_content").and_then(|v| v.as_bool()).unwrap_or(false);
            let format = j.get("format").and_then(|v| v.as_str()).unwrap_or("markdown").to_string();

            let s = { let n = name.clone(); KIT.run_get_or_create(&n) };
            // with_content: the extraction is FOLDED into the didFinish
            // callback — one main-loop hop total, zero after the load.
            let after = if with_content {
                Some(combined_content_js(&format))
            } else {
                None
            };
            match KIT.navigate(&s, &url, after) {
                Err(e) => respond(fd, 502, "Bad Gateway", "application/json", &json_bytes(&err_data(&e))),
                Ok(out) => {
                    let mut payload = json!({"ok": true, "session": name, "url": url, "title": ""});
                    if with_content {
                        match serde_json::from_str::<Value>(&out) {
                            Ok(v) if v.is_string() => {
                                // wry serializes the fold result, which may
                                // itself be a JSON string — decode twice.
                                let inner = v.as_str().unwrap();
                                match serde_json::from_str::<Value>(inner) {
                                    Ok(v2) => {
                                        payload["title"] = v2.get("t").cloned().unwrap_or(json!(""));
                                        payload["content"] = v2.get("c").cloned().unwrap_or(json!(""));
                                        payload["format"] = json!(format);
                                    }
                                    Err(_) => payload["title"] = json!(out),
                                }
                            }
                            Ok(v) => {
                                payload["title"] = v.get("t").cloned().unwrap_or(json!(""));
                                payload["content"] = v.get("c").cloned().unwrap_or(json!(""));
                                payload["format"] = json!(format);
                                payload["raw_fold"] = json!(out);
                                if let Some(err) = v.get("__nav_error") {
                                    payload["fold_error"] = err.clone();
                                }
                            }
                            Err(_) => payload["title"] = json!(out),
                        }
                        // A fold can come back empty when it landed in a stale
                        // page context (cold first navigate under headless CI,
                        // WebKitGTK web-process swap) or on a Chromium error
                        // page. Re-extract on the now-current page via the eval
                        // shim before giving up.
                        let t_empty = payload["title"].as_str().map(|s| s.is_empty()).unwrap_or(true);
                        let c_empty = payload["content"].as_str().map(|s| s.is_empty()).unwrap_or(true);
                        if t_empty && c_empty {
                            let deadline = std::time::Instant::now() + Duration::from_secs(15);
                            let snippet = combined_content_js(&format);
                            while std::time::Instant::now() < deadline {
                                std::thread::sleep(Duration::from_millis(250));
                                let Ok(r) = KIT.eval_js(&s, &snippet) else { continue };
                                let decoded = serde_json::from_str::<Value>(&r).ok().and_then(|v| {
                                    if v.is_string() {
                                        serde_json::from_str::<Value>(v.as_str().unwrap()).ok()
                                    } else {
                                        Some(v)
                                    }
                                });
                                if let Some(v2) = decoded {
                                    let t = v2.get("t").and_then(|x| x.as_str()).unwrap_or("");
                                    let c = v2.get("c").and_then(|x| x.as_str()).unwrap_or("");
                                    if !t.is_empty() || !c.is_empty() {
                                        payload["title"] = json!(t);
                                        payload["content"] = json!(c);
                                        payload["format"] = json!(format);
                                        payload["retried"] = json!(true);
                                        break;
                                    }
                                }
                            }
                        }
                    } else if let Ok(t) = KIT.eval_js(&s, "document.title") {
                        payload["title"] = json!(t);
                    }
                    respond(fd, 200, "OK", "application/json", &json_bytes(&payload));
                }
            }
            if !LOGGED_FIRST_NAV.swap(true, Ordering::Relaxed) {
                if let Some(t0) = T0.get() {
                    eprintln!("[navette] +{} ms — first navigate complete", t0.elapsed().as_millis());
                }
            }
        }

        ("POST", "/read") => {
            let format = j.get("format").and_then(|v| v.as_str()).unwrap_or("markdown").to_string();
            let s = { let n = name.clone(); KIT.run_get_or_create(&n) };
            let js: String = match format.as_str() {
                "text" => "document.body.innerText".into(),
                "html" => "document.documentElement.outerHTML".into(),
                _ => MARKDOWN_JS.into(),
            };
            match KIT.eval_js(&s, &js) {
                Ok(text) => {
                    // eval_js returns the value JSON-encoded — unwrap strings
                    let content = serde_json::from_str::<Value>(&text)
                        .ok()
                        .and_then(|v| v.as_str().map(|s| s.to_string()))
                        .unwrap_or(text);
                    let body = if content.len() > 3_000_000 {
                        json!({"ok": true, "content": content.chars().take(1_000_000).collect::<String>(), "truncated": true})
                    } else {
                        json!({"ok": true, "format": format, "content": content})
                    };
                    respond(fd, 200, "OK", "application/json", &json_bytes(&body));
                }
                Err(e) => respond(fd, 500, "Internal Server Error", "application/json", &json_bytes(&err_data(&e))),
            }
        }

        ("POST", "/screenshot") => {
            let s = { let n = name.clone(); KIT.run_get_or_create(&n) };
            match KIT.screenshot(&s) {
                Ok(png) => respond(fd, 200, "OK", "image/png", &png),
                Err(e) => respond(fd, 500, "Internal Server Error", "application/json", &json_bytes(&err_data(&e))),
            }
        }

        ("POST", "/click") => {
            let sel = j.get("selector").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let wait_nav = j.get("wait_navigation").and_then(|v| v.as_bool()).unwrap_or(false);
            let s = { let n = name.clone(); KIT.run_get_or_create(&n) };
            let clicked = KIT.eval_js(&s, &click_js(&jstr(&sel)));
            if wait_nav {
                // The click may have started a form-POST navigation: settle.
                if clicked.is_ok() {
                    KIT.wait_settle(&s);
                }
            }
            match clicked {
                // eval_js returns the value JSON-encoded ("OK" arrives quoted)
                Ok(v) => {
                    let dec: Value = serde_json::from_str(&v).unwrap_or(json!(v));
                    let hit = dec.as_str() == Some("OK");
                    respond(fd, 200, "OK", "application/json",
                             &json_bytes(&json!({"ok": hit, "result": v})))
                }
                Err(e) => respond(fd, 500, "Internal Server Error", "application/json", &json_bytes(&err_data(&e))),
            }
        }

        ("POST", "/type") => {
            let sel = j.get("selector").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let val = j.get("value").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let s = { let n = name.clone(); KIT.run_get_or_create(&n) };
            match KIT.eval_js(&s, &type_js(&jstr(&sel), &jstr(&val))) {
                Ok(v) => {
                    let dec: Value = serde_json::from_str(&v).unwrap_or(json!(v));
                    let hit = dec.as_str() == Some("OK");
                    respond(fd, 200, "OK", "application/json",
                             &json_bytes(&json!({"ok": hit, "result": v})))
                }
                Err(e) => respond(fd, 500, "Internal Server Error", "application/json", &json_bytes(&err_data(&e))),
            }
        }

        ("POST", "/evaluate") => {
            let js = j.get("js").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let s = { let n = name.clone(); KIT.run_get_or_create(&n) };
            match KIT.eval_js(&s, &js) {
                Ok(v) => respond(fd, 200, "OK", "application/json", &json_bytes(&json!({"ok": true, "result": v}))),
                Err(e) => respond(fd, 500, "Internal Server Error", "application/json", &json_bytes(&err_data(&e))),
            }
        }

        ("POST", "/wait") => {
            let sel = j.get("selector").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let ms = j.get("ms").and_then(|v| v.as_u64()).unwrap_or(10_000);
            let s = { let n = name.clone(); KIT.run_get_or_create(&n) };
            // String() coerces the bool: the macOS bridge stringifies an
            // NSNumber bool as "1"/"0" (description), the wry IPC path as
            // "true"/"false" — accept both shapes.
            let probe = format!("String(!!document.querySelector({}))", jstr(&sel));
            let deadline = Instant::now() + Duration::from_millis(ms);
            let mut found = false;
            while Instant::now() < deadline {
                if let Ok(v) = KIT.eval_js(&s, &probe) {
                    if v == "true" || v == "1" {
                        found = true;
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(150));
            }
            respond(fd, if found { 200 } else { 408 }, if found { "OK" } else { "Request Timeout" },
                    "application/json",
                    &json_bytes(&json!({"ok": found, "selector": sel})));
        }

        ("POST", "/sessions/state") => {
            let s = { let n = name.clone(); KIT.run_get_or_create(&n) };
            match KIT.export_cookies(&s) {
                Ok(v) => respond(fd, 200, "OK", "application/json", &json_bytes(&v)),
                Err(e) => respond(fd, 500, "Internal Server Error", "application/json", &json_bytes(&err_data(&e))),
            }
        }

        ("POST", "/sessions/load") => {
            let s = { let n = name.clone(); KIT.run_get_or_create(&n) };
            match KIT.import_cookies(&s, &j) {
                Ok(n_cookies) => respond(fd, 200, "OK", "application/json",
                                         &json_bytes(&json!({"ok": true, "imported": n_cookies}))),
                Err(e) => respond(fd, 500, "Internal Server Error", "application/json", &json_bytes(&err_data(&e))),
            }
        }

        ("POST", "/sessions/viewport") => {
            let s = { let n = name.clone(); KIT.run_get_or_create(&n) };
            let w = j.get("width").and_then(|v| v.as_u64()).unwrap_or(1280) as u32;
            let h = j.get("height").and_then(|v| v.as_u64()).unwrap_or(800) as u32;
            match KIT.set_viewport(&s, w, h) {
                Ok(()) => respond(fd, 200, "OK", "application/json",
                                  &json_bytes(&json!({"ok": true, "width": w, "height": h}))),
                Err(e) => respond(fd, 500, "Internal Server Error", "application/json", &json_bytes(&err_data(&e))),
            }
        }

        ("POST", "/sessions/show") => {
            let s = { let n = name.clone(); KIT.run_get_or_create(&n) };
            match KIT.show_window(&s) {
                Ok(()) => respond(fd, 200, "OK", "application/json",
                                  &json_bytes(&json!({"ok": true, "visible": true}))),
                Err(e) => respond(fd, 500, "Internal Server Error", "application/json", &json_bytes(&err_data(&e))),
            }
        }

        ("POST", "/sessions/hide") => {
            let s = { let n = name.clone(); KIT.run_get_or_create(&n) };
            match KIT.hide_window(&s) {
                Ok(()) => respond(fd, 200, "OK", "application/json",
                                  &json_bytes(&json!({"ok": true, "visible": false}))),
                Err(e) => respond(fd, 500, "Internal Server Error", "application/json", &json_bytes(&err_data(&e))),
            }
        }

        ("POST", "/upload") => {
            let s = { let n = name.clone(); KIT.run_get_or_create(&n) };
            let sel = j.get("selector").and_then(|v| v.as_str()).unwrap_or("");
            let filename = j.get("filename").and_then(|v| v.as_str()).unwrap_or("upload.bin");
            let mime = j.get("mime").and_then(|v| v.as_str()).unwrap_or("application/octet-stream");
            let b64 = j.get("content_base64").and_then(|v| v.as_str()).unwrap_or("");
            if sel.is_empty() {
                respond(fd, 400, "Bad Request", "application/json", &json_bytes(&err_data("selector is required")));
                return;
            }
            let sel_esc = jstr(sel);
            let name_esc = jstr(filename);
            let mime_esc = jstr(mime);
            // A FileList cannot be forged directly, but input.files IS
            // assignable from a DataTransfer built in the page — the one
            // engine-agnostic way to fill a file input without OS dialogs.
            let js = format!(
                "(function(){{var el=document.querySelector({sel});if(!el)return JSON.stringify({{ok:false,err:'no element'}});if(String(el.type).toLowerCase()!=='file')return JSON.stringify({{ok:false,err:'element is not an <input type=file>'}});var bin=atob('{b64}');var bytes=new Uint8Array(bin.length);for(var i=0;i<bin.length;i++)bytes[i]=bin.charCodeAt(i);var f=new File([bytes],{name},{{type:{mime}}});var dt=new DataTransfer();dt.items.add(f);el.files=dt.files;el.dispatchEvent(new Event('input',{{bubbles:true}}));el.dispatchEvent(new Event('change',{{bubbles:true}}));var names=Array.prototype.map.call(el.files,function(x){{return x.name+':'+x.size}}).join(',');return JSON.stringify({{ok:true,files:names}})}})()",
                sel = sel_esc, b64 = b64, name = name_esc, mime = mime_esc
            );
            match KIT.eval_js(&s, &js) {
                Ok(v) => respond(fd, 200, "OK", "application/json", &json_bytes(&json!({"ok": true, "result": v}))),
                Err(e) => respond(fd, 500, "Internal Server Error", "application/json", &json_bytes(&err_data(&e))),
            }
        }

        ("POST", "/scroll") => {
            let s = { let n = name.clone(); KIT.run_get_or_create(&n) };
            let sel = j.get("selector").and_then(|v| v.as_str()).unwrap_or("");
            let y = j.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let js = if !sel.is_empty() {
                let sel_esc = jstr(sel);
                format!(
                    "(function(){{var el=document.querySelector({sel});if(!el)return JSON.stringify({{ok:false,err:'no element'}});el.scrollIntoView({{block:'center'}});return JSON.stringify({{ok:true,y:window.scrollY}})}})()",
                    sel = sel_esc
                )
            } else {
                format!(
                    "(function(){{window.scrollTo(0,{y});return JSON.stringify({{ok:true,y:window.scrollY,max:document.documentElement.scrollHeight-window.innerHeight}})}})()",
                    y = y
                )
            };
            match KIT.eval_js(&s, &js) {
                Ok(v) => respond(fd, 200, "OK", "application/json", &json_bytes(&json!({"ok": true, "result": v}))),
                Err(e) => respond(fd, 500, "Internal Server Error", "application/json", &json_bytes(&err_data(&e))),
            }
        }

        ("POST", "/hover") => {
            let s = { let n = name.clone(); KIT.run_get_or_create(&n) };
            let sel = j.get("selector").and_then(|v| v.as_str()).unwrap_or("");
            let sel_esc = jstr(sel);
            let js = format!(
                "(function(){{var el=document.querySelector({sel});if(!el)return JSON.stringify({{ok:false,err:'no element'}});var r=el.getBoundingClientRect();var o={{clientX:r.left+r.width/2,clientY:r.top+r.height/2,bubbles:true,cancelable:true,view:window,buttons:0}};['mouseover','mousemove'].forEach(function(t){{el.dispatchEvent(new MouseEvent(t,o))}});return JSON.stringify({{ok:true}})}})()",
                sel = sel_esc
            );
            match KIT.eval_js(&s, &js) {
                Ok(v) => respond(fd, 200, "OK", "application/json", &json_bytes(&json!({"ok": true, "result": v}))),
                Err(e) => respond(fd, 500, "Internal Server Error", "application/json", &json_bytes(&err_data(&e))),
            }
        }

        ("POST", "/key") => {
            let s = { let n = name.clone(); KIT.run_get_or_create(&n) };
            let k = j.get("key").and_then(|v| v.as_str()).unwrap_or("");
            let sel = j.get("selector").and_then(|v| v.as_str()).unwrap_or("");
            if k.is_empty() {
                respond(fd, 400, "Bad Request", "application/json", &json_bytes(&err_data("key is required")));
                return;
            }
            // Native first: real OS-level events (isTrusted=true). Any failure
            // (no mapping, no focus, unsupported platform) falls back to the
            // in-page synthetic dispatch. A selector is focused via JS first —
            // native events land on the focused element.
            let mut mode = "synthetic";
            if !sel.is_empty() {
                let sel_esc = jstr(sel);
                let _ = KIT.eval_js(&s, &format!(
                    "(function(){{var el=document.querySelector({s});if(el)el.focus();}})()",
                    s = sel_esc
                ));
            }
            match KIT.native_key(&s, k) {
                Ok(()) => {
                    // Real event posted — the synthetic dispatch must NOT also
                    // fire (it would double the keystroke and mask the native
                    // one behind an untrusted copy).
                    respond(
                        fd,
                        200,
                        "OK",
                        "application/json",
                        &json_bytes(&json!({"ok": true, "mode": "native"})),
                    );
                    return;
                }
                Err(e) => eprintln!("[navette] native key fallback ({e})"),
            }
            let code = if k.len() == 1 {
                let c = k.chars().next().unwrap();
                if c.is_ascii_alphabetic() { format!("Key{}", c.to_ascii_uppercase()) }
                else if c.is_ascii_digit() { format!("Digit{}", c) }
                else { "Unidentified".to_string() }
            } else { k.to_string() };
            let k_esc = jstr(k);
            let code_esc = jstr(&code);
            // dispatch on the focused element, or on the given selector
            let js = if sel.is_empty() {
                format!(
                    "(function(){{var el=document.activeElement||document.body;var o={{key:{k},code:{c},bubbles:true,cancelable:true}};var r=true;['keydown','keyup'].forEach(function(t){{r=el.dispatchEvent(new KeyboardEvent(t,o))&&r}});return JSON.stringify({{ok:true,defaultPrevented:!r}})}})()",
                    k = k_esc, c = code_esc
                )
            } else {
                let sel_esc = jstr(sel);
                format!(
                    "(function(){{var el=document.querySelector({s});if(!el)return JSON.stringify({{ok:false,err:'no element'}});el.focus();var o={{key:{k},code:{c},bubbles:true,cancelable:true}};var r=true;['keydown','keyup'].forEach(function(t){{r=el.dispatchEvent(new KeyboardEvent(t,o))&&r}});return JSON.stringify({{ok:true,defaultPrevented:!r}})}})()",
                    s = sel_esc, k = k_esc, c = code_esc
                )
            };
            match KIT.eval_js(&s, &js) {
                Ok(v) => respond(fd, 200, "OK", "application/json", &json_bytes(&json!({"ok": true, "mode": mode, "result": v}))),
                Err(e) => respond(fd, 500, "Internal Server Error", "application/json", &json_bytes(&err_data(&e))),
            }
        }

        _ => {
            respond(fd, 404, "Not Found", "application/json",
                    &json_bytes(&err_data(&format!("unknown route {} {}", req.method, path))));
        }
    }
}

// MARK: - serve mode

fn health_ok(port: u16) -> bool {
    TcpStream::connect(("127.0.0.1", port))
        .and_then(|mut s| {
            s.write_all(format!("GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").as_bytes())?;
            let mut buf = String::new();
            s.read_to_string(&mut buf)?;
            Ok(buf.contains("\"ok\""))
        })
        .unwrap_or(false)
}

fn serve(args: &[String]) {
    #[cfg(unix)]
    install_graceful_signals();
    let _ = T0.set(Instant::now());
    let port: u16 = args
        .iter()
        .position(|a| a == "--port")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(8765);

    // Idle watchdog: drop WebKit sessions after N minutes without any HTTP
    // request. The daemon stays alive; the next request re-warms on demand.
    // 0 (default) keeps sessions forever — the original always-warm promise.
    let idle_release_min: f64 = args
        .iter()
        .position(|a| a == "--idle-release")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);

    let token: Option<String> = args
        .iter()
        .position(|a| a == "--token")
        .and_then(|i| args.get(i + 1))
        .map(|v| v.to_string());
    if let Some(t) = &token {
        let _ = TOKEN.set(t.clone());
        eprintln!("[navette] auth enabled: routes require the token (Authorization: Bearer)");
    }

    let proxy: Option<String> = args
        .iter()
        .position(|a| a == "--proxy")
        .and_then(|i| args.get(i + 1))
        .map(|v| v.to_string());
    let user_agent: Option<String> = args
        .iter()
        .position(|a| a == "--user-agent")
        .and_then(|i| args.get(i + 1))
        .map(|v| v.to_string());
    if proxy.is_some() || user_agent.is_some() {
        KIT.set_agent_options(proxy.clone(), user_agent.clone());
        if let Some(pr) = &proxy {
            eprintln!("[navette] proxy: {} (wry backends; macOS uses the system proxy)", redact_proxy(&pr));
        }
        if user_agent.is_some() {
            eprintln!("[navette] user-agent override set");
        }
    }

    let t0 = Instant::now();
    #[cfg(target_os = "macos")]
    {
        // Measured order (see navette-swift cold-start notes): AppKit first,
        // listener second (health answers while the engine warms), webview
        // pre-warm third — created on THIS main thread, before app.run().
        backend::app_init();
        eprintln!("[navette] +{} ms — appkit ready", t0.elapsed().as_millis());
    }

    #[cfg(target_os = "macos")]
    {
        // macOS: the listener starts here (the wry backends start their own
        // listener inside run_main_loop, after the proxy is ready).
        let listener = match TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => l,
            Err(e) => {
                if health_ok(port) {
                    eprintln!("[navette] port {port} already served by another navette — idling");
                    loop {
                        thread::sleep(Duration::from_secs(3600));
                    }
                }
                eprintln!("[navette] cannot bind 127.0.0.1:{port} — {e}");
                std::process::exit(1);
            }
        };
    // Idle-watchdog thread: every 30 s, drop sessions idle for longer than
    // the --idle-release threshold. Started late enough that the backend's
    // event-loop proxy exists (wry sets it inside run_main_loop startup).
    if idle_release_min > 0.0 {
        let idle_secs = (idle_release_min * 60.0) as u64;
        thread::spawn(move || loop {
            thread::sleep(Duration::from_secs(30));
            let n = KIT.reap_idle(idle_secs);
            let _ = n;
        });
        eprintln!(
            "[navette] idle-release enabled: sessions drop after {} min without requests",
            idle_release_min
        );
    }

        {
            let listener = listener;
            thread::spawn(move || {
                for stream in listener.incoming() {
                    match stream {
                        Ok(s) => {
                            thread::spawn(move || serve_fd(s));
                        }
                        Err(_) => continue,
                    }
                }
            });
        }
    }
    eprintln!("[navette] +{} ms — listener ready", t0.elapsed().as_millis());

    #[cfg(target_os = "macos")]
    {
        KIT.prewarm_default();
        eprintln!("[navette] +{} ms — pre-warm done (WebContent spawning)", t0.elapsed().as_millis());
    }

    println!("[navette] the browser for agents — listening on http://127.0.0.1:{port}  (engine: system WebView)");
    let _ = std::io::stdout().flush();

    #[cfg(target_os = "macos")]
    backend::app_run();
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        if idle_release_min > 0.0 {
            let idle_secs = (idle_release_min * 60.0) as u64;
            thread::spawn(move || loop {
                thread::sleep(Duration::from_secs(30));
                let _ = KIT.reap_idle(idle_secs);
            });
            eprintln!(
                "[navette] idle-release enabled: sessions drop after {} min without requests",
                idle_release_min
            );
        }
        backend::run_main_loop(); // tao event loop — owns main, never returns
    } // tao event loop — owns main, never returns
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        println!("[navette] no engine backend on this platform yet — HTTP skeleton only");
        loop {
            thread::sleep(Duration::from_secs(3600));
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        println!("[navette] no engine backend on this platform yet — HTTP skeleton only");
        loop {
            thread::sleep(Duration::from_secs(3600));
        }
    }
}

// MARK: - TCP listener (shared by all backends)

pub fn start_listener(port: u16) {
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            if health_ok(port) {
                eprintln!("[navette] port {port} already served by another navette — idling");
                loop {
                    thread::sleep(Duration::from_secs(3600));
                }
            }
            eprintln!("[navette] cannot bind 127.0.0.1:{port} — {e}");
            std::process::exit(1);
        }
    };
    thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(s) => {
                    thread::spawn(move || serve_fd(s));
                }
                Err(_) => continue,
            }
        }
    });
}

// MARK: - resident daemon (LaunchAgent / scheduled task / systemd user unit)

const DAEMON_LABEL: &str = "dev.navette.daemon";

fn daemon_exe() -> std::path::PathBuf {
    std::env::current_exe().unwrap_or_else(|_| {
        eprintln!("[navette] cannot resolve current executable");
        std::process::exit(1);
    })
}

#[cfg(target_os = "macos")]
fn daemon_plist_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::Path::new(&home).join("Library/LaunchAgents/dev.navette.daemon.plist")
}

#[cfg(target_os = "macos")]
fn install_daemon() {
    let exe = std::env::current_exe().unwrap_or_else(|_| {
        eprintln!("[navette] cannot resolve current executable");
        std::process::exit(1);
    });
    let plist_path = daemon_plist_path();
    if let Some(parent) = plist_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>serve</string>
        <string>--port</string>
        <string>8765</string>
    </array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>StandardOutPath</key><string>/tmp/navette-daemon.log</string>
    <key>StandardErrorPath</key><string>/tmp/navette-daemon.log</string>
</dict>
</plist>
"#,
        label = DAEMON_LABEL,
        exe = exe.display()
    );
    std::fs::write(&plist_path, plist).unwrap_or_else(|e| {
        eprintln!("[navette] cannot write {}: {e}", plist_path.display());
        std::process::exit(1);
    });
    // Kill by label first — unload alone can leave an old binary serving.
    let _ = Command::new("launchctl").args(["remove", DAEMON_LABEL]).status();
    let _ = Command::new("launchctl").args(["unload", &plist_path.to_string_lossy()]).status();
    // Wait until the previous instance actually releases the port before
    // loading — otherwise launchd leaves the old binary serving.
    for _ in 0..50 {
        if TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], 8765u16)), Duration::from_millis(200)).is_err() {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let loaded = Command::new("launchctl").args(["load", &plist_path.to_string_lossy()]).status();
    match loaded {
        Ok(s) if s.success() => {
            println!("[navette] resident daemon installed — the shuttle is warm from login.");
            println!("  plist: {}", plist_path.display());
            println!("  uninstall with: navette uninstall-daemon");
        }
        other => {
            println!("[navette] plist written but launchctl returned {other:?} — try: launchctl load {}", plist_path.display());
        }
    }
}

#[cfg(target_os = "macos")]
fn uninstall_daemon() {
    let plist_path = daemon_plist_path();
    let _ = Command::new("launchctl").args(["unload", &plist_path.to_string_lossy()]).status();
    match std::fs::remove_file(&plist_path) {
        Ok(()) => println!("[navette] resident daemon removed."),
        Err(e) => println!("[navette] nothing to remove ({e})"),
    }
}

#[cfg(target_os = "windows")]
fn install_daemon() {
    let exe = daemon_exe();
    let tr = format!("\"{}\" serve --port 8765", exe.display());
    let st = Command::new("schtasks")
        .args(["/Create", "/TN", "navette", "/TR", &tr, "/SC", "ONLOGON", "/F"])
        .status();
    match st {
        Ok(s) if s.success() => {
            println!("[navette] resident daemon installed — scheduled task 'navette', warm from login.");
            println!("  uninstall with: navette uninstall-daemon");
        }
        other => {
            println!("[navette] schtasks returned {other:?} — create a scheduled task for \"{} serve --port 8765\" manually.", exe.display());
        }
    }
}

#[cfg(target_os = "windows")]
fn uninstall_daemon() {
    let st = Command::new("schtasks").args(["/Delete", "/TN", "navette", "/F"]).status();
    match st {
        Ok(s) if s.success() => println!("[navette] resident daemon removed."),
        _ => println!("[navette] no scheduled task named 'navette'."),
    }
}

#[cfg(target_os = "linux")]
fn install_daemon() {
    let exe = daemon_exe();
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let dir = std::path::Path::new(&home).join(".config/systemd/user");
    let _ = std::fs::create_dir_all(&dir);
    let unit = format!(
        "[Unit]\nDescription=navette — the browser for agents\n\n[Service]\nExecStart={} serve --port 8765\nRestart=always\n\n[Install]\nWantedBy=default.target\n",
        exe.display()
    );
    let unit_path = dir.join("navette.service");
    std::fs::write(&unit_path, unit).unwrap_or_else(|e| {
        eprintln!("[navette] cannot write {}: {e}", unit_path.display());
        std::process::exit(1);
    });
    let reload = Command::new("systemctl").args(["--user", "daemon-reload"]).status();
    let enable = Command::new("systemctl").args(["--user", "enable", "--now", "navette.service"]).status();
    match (reload, enable) {
        (Ok(r), Ok(e)) if r.success() && e.success() => {
            println!("[navette] resident daemon installed — systemd user unit, warm from login.");
            println!("  unit: {}", unit_path.display());
            println!("  uninstall with: navette uninstall-daemon");
        }
        _ => {
            println!("[navette] unit written but systemctl --user failed (no user session?) — start it with: systemctl --user start navette.service");
        }
    }
}

#[cfg(target_os = "linux")]
fn uninstall_daemon() {
    let _ = Command::new("systemctl").args(["--user", "disable", "--now", "navette.service"]).status();
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let unit_path = std::path::Path::new(&home).join(".config/systemd/user/navette.service");
    match std::fs::remove_file(&unit_path) {
        Ok(()) => {
            let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).status();
            println!("[navette] resident daemon removed.");
        }
        Err(e) => println!("[navette] nothing to remove ({e})"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("--help") | Some("-h") => {
            println!("navette {} — the browser for agents", env!("CARGO_PKG_VERSION"));
            println!();
            println!("USAGE:");
            println!("  navette serve [--port N]    HTTP API on 127.0.0.1 (default 8765)");
            println!("             [--token SECRET] require Bearer auth on all routes but /health");
            println!("             [--idle-release MIN] drop idle WebKit sessions (memory saver)");
            println!("             [--proxy URL] HTTP CONNECT or SOCKS5 proxy for every session");
            println!("             [--user-agent UA] per-serve user-agent override");;
            println!("  navette mcp                 MCP stdio server for agent hosts");
            println!("  navette install-daemon      resident: warm from login");
            println!("  navette uninstall-daemon    remove the resident daemon");
            println!("  navette --version           print the version");
            println!();
            println!("HTTP routes: /health /sessions /navigate /read /screenshot /click /hover");
            println!("/type /key /evaluate /wait /scroll /upload /sessions/viewport /sessions/state");
            println!("/sessions/load /sessions/show /sessions/hide /sessions/close — full docs:");
            println!("https://github.com/slabbdev/navette");
        }
        Some("--version") | Some("-V") | Some("version") => {
            println!("navette {}", env!("CARGO_PKG_VERSION"));
        }
        Some("mcp") => mcp::run(),
        Some("install-daemon") => install_daemon(),
        Some("uninstall-daemon") => uninstall_daemon(),
        _ => serve(&args),
    }
}

// Re-export used by backend files.
pub use std as _std;
#[allow(unused_imports)]
use serde_json as _serde_json;
#[allow(dead_code)]
fn _keep_imports() {
    let _ = TcpStream::connect_timeout;
}
