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

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

static T0: OnceLock<Instant> = OnceLock::new();
pub static PORT: OnceLock<u16> = OnceLock::new();
static LOGGED_FIRST_NAV: AtomicBool = AtomicBool::new(false);

// MARK: - JS snippets (shared across backends; keep in sync with the reference)

fn click_js(sel: &str) -> String {
    format!(
        r#"(function(sel){{
  const el = document.querySelector(sel);
  if (!el) return 'MISSING';
  const r = el.getBoundingClientRect();
  const o = {{bubbles:true, cancelable:true, view:window, clientX:r.x + r.width/2, clientY:r.y + r.height/2}};
  el.dispatchEvent(new MouseEvent('mousedown', o));
  el.dispatchEvent(new MouseEvent('mouseup', o));
  if (el instanceof HTMLElement) {{ el.click(); }} else {{ el.dispatchEvent(new MouseEvent('click', o)); }}
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

// MARK: - HTTP plumbing

struct Req {
    method: String,
    path: String,
    body: Vec<u8>,
}

fn parse_request(data: &[u8]) -> Option<Req> {
    let idx = data.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&data[..idx]).to_string();
    let mut lines = head.split("\r\n");
    let first = lines.next()?.to_string();
    let mut cl = 0usize;
    for l in lines {
        let mut kv = l.splitn(2, ':');
        if let (Some(k), Some(v)) = (kv.next(), kv.next()) {
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
    let path = req.path.split('?').next().unwrap_or("/").to_string();
    let j: Value = serde_json::from_slice(&req.body).unwrap_or(json!({}));
    let name = j.get("session").and_then(|v| v.as_str()).unwrap_or("default").to_string();

    match (req.method.as_str(), path.as_str()) {
        ("GET", "/health") => {
            respond(fd, 200, "OK", "application/json",
                    &json_bytes(&json!({"ok": true, "name": "navette", "engine": "system WebView"})));
        }

        ("GET", "/sessions") => {
            respond(fd, 200, "OK", "application/json", &json_bytes(&backend::list_sessions()));
        }

        ("POST", "/sessions/close") => {
            backend::close_session(&name);
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

            let s = { let n = name.clone(); backend::run_get_or_create(&n) };
            // with_content: the extraction is FOLDED into the didFinish
            // callback — one main-loop hop total, zero after the load.
            let after = if with_content {
                Some(combined_content_js(&format))
            } else {
                None
            };
            match backend::navigate(&s, &url, after) {
                Err(e) => respond(fd, 502, "Bad Gateway", "application/json", &json_bytes(&err_data(&e))),
                Ok(out) => {
                    let mut payload = json!({"ok": true, "session": name, "url": url, "title": ""});
                    if with_content {
                        match serde_json::from_str::<Value>(&out) {
                            Ok(v) => {
                                payload["title"] = v.get("t").cloned().unwrap_or(json!(""));
                                payload["content"] = v.get("c").cloned().unwrap_or(json!(""));
                                payload["format"] = json!(format);
                            }
                            Err(_) => payload["title"] = json!(out),
                        }
                    } else if let Ok(t) = backend::eval_js(&s, "document.title") {
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
            let s = { let n = name.clone(); backend::run_get_or_create(&n) };
            let js: String = match format.as_str() {
                "text" => "document.body.innerText".into(),
                "html" => "document.documentElement.outerHTML".into(),
                _ => MARKDOWN_JS.into(),
            };
            match backend::eval_js(&s, &js) {
                Ok(text) => {
                    let body = if text.len() > 3_000_000 {
                        json!({"ok": true, "content": text.chars().take(1_000_000).collect::<String>(), "truncated": true})
                    } else {
                        json!({"ok": true, "format": format, "content": text})
                    };
                    respond(fd, 200, "OK", "application/json", &json_bytes(&body));
                }
                Err(e) => respond(fd, 500, "Internal Server Error", "application/json", &json_bytes(&err_data(&e))),
            }
        }

        ("POST", "/screenshot") => {
            let s = { let n = name.clone(); backend::run_get_or_create(&n) };
            match backend::screenshot(&s) {
                Ok(png) => respond(fd, 200, "OK", "image/png", &png),
                Err(e) => respond(fd, 500, "Internal Server Error", "application/json", &json_bytes(&err_data(&e))),
            }
        }

        ("POST", "/click") => {
            let sel = j.get("selector").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let wait_nav = j.get("wait_navigation").and_then(|v| v.as_bool()).unwrap_or(false);
            let s = { let n = name.clone(); backend::run_get_or_create(&n) };
            let clicked = backend::eval_js(&s, &click_js(&jstr(&sel)));
            if wait_nav {
                // The click may have started a form-POST navigation: settle.
                if clicked.is_ok() {
                    backend::wait_settle(&s);
                }
            }
            match clicked {
                Ok(v) => respond(fd, 200, "OK", "application/json",
                                 &json_bytes(&json!({"ok": v == "OK", "result": v}))),
                Err(e) => respond(fd, 500, "Internal Server Error", "application/json", &json_bytes(&err_data(&e))),
            }
        }

        ("POST", "/type") => {
            let sel = j.get("selector").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let val = j.get("value").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let s = { let n = name.clone(); backend::run_get_or_create(&n) };
            match backend::eval_js(&s, &type_js(&jstr(&sel), &jstr(&val))) {
                Ok(v) => respond(fd, 200, "OK", "application/json",
                                 &json_bytes(&json!({"ok": v == "OK", "result": v}))),
                Err(e) => respond(fd, 500, "Internal Server Error", "application/json", &json_bytes(&err_data(&e))),
            }
        }

        ("POST", "/evaluate") => {
            let js = j.get("js").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let s = { let n = name.clone(); backend::run_get_or_create(&n) };
            match backend::eval_js(&s, &js) {
                Ok(v) => respond(fd, 200, "OK", "application/json", &json_bytes(&json!({"ok": true, "result": v}))),
                Err(e) => respond(fd, 500, "Internal Server Error", "application/json", &json_bytes(&err_data(&e))),
            }
        }

        ("POST", "/wait") => {
            let sel = j.get("selector").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let ms = j.get("ms").and_then(|v| v.as_u64()).unwrap_or(10_000);
            let s = { let n = name.clone(); backend::run_get_or_create(&n) };
            let probe = format!("!!document.querySelector({})", jstr(&sel));
            let deadline = Instant::now() + Duration::from_millis(ms);
            let mut found = false;
            while Instant::now() < deadline {
                if let Ok(v) = backend::eval_js(&s, &probe) {
                    if v == "true" {
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
            let s = { let n = name.clone(); backend::run_get_or_create(&n) };
            match backend::export_cookies(&s) {
                Ok(v) => respond(fd, 200, "OK", "application/json", &json_bytes(&v)),
                Err(e) => respond(fd, 500, "Internal Server Error", "application/json", &json_bytes(&err_data(&e))),
            }
        }

        ("POST", "/sessions/load") => {
            let s = { let n = name.clone(); backend::run_get_or_create(&n) };
            match backend::import_cookies(&s, &j) {
                Ok(n_cookies) => respond(fd, 200, "OK", "application/json",
                                         &json_bytes(&json!({"ok": true, "imported": n_cookies}))),
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
    let _ = T0.set(Instant::now());
    let port: u16 = args
        .iter()
        .position(|a| a == "--port")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(8765);

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

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    backend::init_main_loop();
    #[cfg(target_os = "macos")]
    {
        backend::prewarm_default();
        eprintln!("[navette] +{} ms — pre-warm done (WebContent spawning)", t0.elapsed().as_millis());
    }

    println!("[navette] the browser for agents — listening on http://127.0.0.1:{port}  (engine: system WebView)");
    let _ = std::io::stdout().flush();

    #[cfg(target_os = "macos")]
    backend::app_run();
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    backend::run_main_loop(); // tao event loop — owns main, never returns
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

// MARK: - resident daemon (LaunchAgent)

const DAEMON_LABEL: &str = "dev.navette.daemon";

fn daemon_plist_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::Path::new(&home).join("Library/LaunchAgents/dev.navette.daemon.plist")
}

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

fn uninstall_daemon() {
    let plist_path = daemon_plist_path();
    let _ = Command::new("launchctl").args(["unload", &plist_path.to_string_lossy()]).status();
    match std::fs::remove_file(&plist_path) {
        Ok(()) => println!("[navette] resident daemon removed."),
        Err(e) => println!("[navette] nothing to remove ({e})"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
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
