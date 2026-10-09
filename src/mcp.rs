// mcp — MCP stdio adapter inside the navette binary.
// `navette mcp` speaks JSON-RPC 2.0 / MCP on stdin+stdout and proxies tool
// calls to the `navette serve` HTTP server, auto-starting it if it is down.
// stdout carries ONLY protocol lines; everything else goes to stderr.

use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::net::TcpStream;
use std::process::Command;
use std::time::{Duration, Instant};

fn port() -> u16 {
    std::env::var("NAVETTE_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8765)
}

fn log(s: &str) {
    eprintln!("[navette-mcp] {s}");
}

// ---------- Minimal loopback HTTP client

fn http_call(path: &str, method: &str, body: Option<&Value>) -> (u16, Value) {
    let mut stream = match TcpStream::connect(("127.0.0.1", port())) {
        Ok(s) => s,
        Err(e) => return (0, json!({"error": e.to_string()})),
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(130)));
    let body_s = body.map(|v| v.to_string()).unwrap_or_default();
    let auth = std::env::var("NAVETTE_TOKEN")
        .ok()
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
        body_s.len(),
        body_s
    );
    if stream.write_all(req.as_bytes()).is_err() {
        return (0, json!({"error": "write failed"}));
    }
    let mut buf = Vec::new();
    use std::io::Read;
    if stream.read_to_end(&mut buf).is_err() {
        return (0, json!({"error": "read failed"}));
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body_str = text
        .find("\r\n\r\n")
        .map(|i| &text[i + 4..])
        .unwrap_or("{}");
    let v = serde_json::from_str(body_str).unwrap_or(json!({}));
    (status, v)
}

fn ok(status: u16) -> bool {
    (200..300).contains(&status)
}

// ---------- Server lifecycle

fn server_up() -> bool {
    http_call("/health", "GET", None).0 == 200
}

fn ensure_server() {
    if server_up() {
        return;
    }
    let exe = std::env::current_exe().unwrap_or_default();
    log(format!("starting navette serve: {}", exe.display()).as_str());
    let log_path = "/tmp/navette-server.log";
    if !std::path::Path::new(log_path).exists() {
        let _ = std::fs::File::create(log_path);
    }
    if let Ok(fh) = std::fs::OpenOptions::new().append(true).create(true).open(log_path) {
        let _ = Command::new(&exe)
            .args(["serve", "--port", &port().to_string()])
            .stdout(fh.try_clone().unwrap())
            .stderr(fh)
            .spawn();
    } else {
        let _ = Command::new(&exe)
            .args(["serve", "--port", &port().to_string()])
            .spawn();
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if server_up() {
            log("navette server is up");
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    log("navette server did not come up in time");
}

// ---------- Tool definitions

fn tool(name: &str, desc: &str, props: Value, required: &[&str]) -> Value {
    let mut schema = json!({"type": "object", "properties": props});
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    json!({"name": name, "description": desc, "inputSchema": schema})
}

fn p_string(desc: &str) -> Value {
    json!({"type": "string", "description": desc})
}

fn p_bool(desc: &str) -> Value {
    json!({"type": "boolean", "description": desc})
}

fn p_enum(desc: &str, values: &[&str]) -> Value {
    json!({"type": "string", "description": desc, "enum": values})
}

fn p_number(desc: &str) -> Value {
    json!({"type": "number", "description": desc})
}

fn tools() -> Value {
    json!([
        tool("navigate",
             "Open a URL in a navette browser session and wait for the page to load. Returns the final title. Set with_content to also get the page content in the same round-trip.",
             json!({
                 "url": p_string("URL to open (http, https or file)"),
                 "with_content": p_bool("Also return page content in the same round-trip (markdown by default)"),
                 "format": p_enum("Content format when with_content is set", &["markdown", "text"]),
                 "session": p_string("Session name; defaults to 'default'")
             }),
             &["url"]),
        tool("read",
             "Read the current page content. Prefer markdown for agents.",
             json!({
                 "session": p_string("Session name"),
                 "format": p_enum("Output format, default markdown", &["markdown", "text", "html"])
             }),
             &[]),
        tool("screenshot",
             "Take a PNG screenshot of the current page and return it as an image, so you can SEE the page.",
             json!({"session": p_string("Session name")}),
             &[]),
        tool("click",
             "Click an element matched by a CSS selector (dispatches real pointer and mouse events, so Radix/HeadlessUI dropdown menus open too). Set wait_navigation when the click submits a form or navigates — navette then waits for the new page to finish loading.",
             json!({
                 "selector": p_string("CSS selector of the element to click"),
                 "wait_navigation": p_bool("Wait for the navigation triggered by this click (form submit, link)"),
                 "session": p_string("Session name")
             }),
             &["selector"]),
        tool("type",
             "Fill an input matched by a CSS selector. React-safe: sets the value through the native prototype setter, then dispatches input and change events.",
             json!({
                 "selector": p_string("CSS selector of the input"),
                 "value": p_string("Text to type"),
                 "session": p_string("Session name")
             }),
             &["selector", "value"]),
        tool("evaluate",
             "Run JavaScript in the page and return the result (use for checks, extraction, anything the other tools miss).",
             json!({
                 "js": p_string("JavaScript expression to evaluate"),
                 "session": p_string("Session name")
             }),
             &["js"]),
        tool("wait",
             "Wait until a CSS selector exists in the page (polls every 150 ms).",
             json!({
                 "selector": p_string("CSS selector to wait for"),
                 "ms": p_number("Max wait in milliseconds, default 10000"),
                 "session": p_string("Session name")
             }),
             &["selector"]),
        tool("sessions",
             "List open browser sessions with their current URL and title.",
             json!({}),
             &[]),
        tool("state_export",
             "Export the session's logged-in state. With store set, the cookie jar goes straight to the OS keychain under that name and is NOT returned — the safe path, since the jar IS a login. Without store, the jar comes back as JSON; keep it secret. Re-import later with state_import, even after a navette restart.",
             json!({
                 "session": p_string("Session name"),
                 "store": p_string("Save into the OS keychain under this name instead of returning the cookies (see state_import store)")
             }),
             &[]),
        tool("state_import",
             "Restore a logged-in state into a session without re-logging in. Pass either cookies (JSON from state_export without store) or store (a name previously saved with state_export store) — never both.",
             json!({
                 "cookies": p_string("The cookies JSON exported by state_export (raw or wrapped in {\"cookies\":[...]})"),
                 "store": p_string("Keychain store name saved earlier via state_export store"),
                 "session": p_string("Session name")
             }),
             &[]),
        tool("session_close",
             "Close a browser session and free its window.",
             json!({"session": p_string("Session name to close")}),
             &["session"]),
        tool("session_show",
             "Show the session's window on screen so a HUMAN can interact with it — the one-time-login primitive: navigate to the login page, call this, let the person type their credentials, then hide and export the state.",
             json!({"session": p_string("Session name")}),
             &[]),
        tool("session_hide",
             "Hide the session's window again after a human finished interacting with it (the window stays alive — the session and its state are untouched).",
             json!({"session": p_string("Session name")}),
             &[]),
        tool("login",
             "Fill and submit the site's login form from the OS keychain — the agent NEVER sees the credentials. A human saves them first with the CLI `navette creds set SITE` (binds to the site's exact origin). The session must currently be ON that origin (navigate to the login page first) or the call is refused. submit=false fills without submitting. Verify the logged-in state yourself afterwards (page probe); 2FA steps still need session_show for the human. One call, no retry loops.",
             json!({
                 "site": p_string("Credential name saved by `navette creds set`"),
                 "submit": p_bool("Click the form's submit control after filling (default true)"),
                 "session": p_string("Session name — must be on the credential's origin")
             }),
             &["site"]),
        tool("hover",
             "Hover an element matched by a CSS selector (dispatches mouseover/mousemove at its center — reveals hover menus and tooltips).",
             json!({
                 "selector": p_string("CSS selector of the element to hover"),
                 "session": p_string("Session name")
             }),
             &["selector"]),
        tool("key",
             "Send a key press (keydown+keyup) to the focused element or to a given selector: Enter, Tab, Escape, ArrowDown, single characters…",
             json!({
                 "key": p_string("Key name, e.g. Enter, Tab, Escape, ArrowDown, a, 4"),
                 "selector": p_string("Optional CSS selector to focus first"),
                 "session": p_string("Session name")
             }),
             &["key"]),
        tool("viewport",
             "Set the session's viewport size in pixels (default 1280x800). Affects rendering and screenshots.",
             json!({
                 "width": p_number("Viewport width in pixels"),
                 "height": p_number("Viewport height in pixels"),
                 "session": p_string("Session name")
             }),
             &[]),
        tool("scroll",
             "Scroll the page to an absolute Y offset, or bring an element into view (scrollIntoView, centered).",
             json!({
                 "y": p_number("Absolute Y offset in pixels (ignored when selector is given)"),
                 "selector": p_string("Optional CSS selector to scroll into view"),
                 "session": p_string("Session name")
             }),
             &[]),
        tool("upload",
             "Fill a file input (<input type=file>) with in-memory content: builds a real File in the page and dispatches change. The page never sees an OS dialog.",
             json!({
                 "selector": p_string("CSS selector of the file input"),
                 "filename": p_string("File name the page will see"),
                 "content_base64": p_string("File content, base64-encoded"),
                 "mime": p_string("MIME type (default application/octet-stream)"),
                 "session": p_string("Session name")
             }),
             &["selector", "filename", "content_base64"]),
    ])
}

// ---------- Tool execution

fn text_content(s: String, is_err: bool) -> Value {
    json!({"content": [{"type": "text", "text": s}], "isError": is_err})
}

fn compact(v: Value) -> String {
    serde_json::to_string(&v).unwrap_or_else(|_| "{}".into())
}

fn run_tool(name: &str, a: &Value) -> Value {
    ensure_server();
    let sget = |k: &str, d: &str| a.get(k).and_then(|v| v.as_str()).unwrap_or(d).to_string();
    let session = sget("session", "default");

    match name {
        "navigate" => {
            let mut body = json!({"url": a.get("url").cloned().unwrap_or(json!("")), "session": session});
            if a.get("with_content").and_then(|v| v.as_bool()).unwrap_or(false) {
                body["with_content"] = json!(true);
            }
            if let Some(f) = a.get("format").and_then(|v| v.as_str()) {
                body["format"] = json!(f);
            }
            let (code, data) = http_call("/navigate", "POST", Some(&body));
            if ok(code) { text_content(compact(data), false) }
            else { text_content(format!("navigate failed ({}): {}", code, compact(data)), true) }
        }
        "read" => {
            let mut body = json!({"session": session});
            if let Some(f) = a.get("format").and_then(|v| v.as_str()) {
                body["format"] = json!(f);
            }
            let (code, data) = http_call("/read", "POST", Some(&body));
            if ok(code) { text_content(compact(data), false) }
            else { text_content(format!("read failed ({}): {}", code, compact(data)), true) }
        }
        "screenshot" => {
            let (code, png) = raw_post("/screenshot", &json!({"session": session}));
            if !ok(code) {
                return text_content(format!("screenshot failed ({})", code), true);
            }
            json!({
                "content": [
                    {"type": "image", "data": base64_encode(&png), "mimeType": "image/png"},
                    {"type": "text", "text": format!("Screenshot captured ({} bytes).", png.len())}
                ],
                "isError": false
            })
        }
        "click" => {
            let mut body = json!({"selector": a.get("selector").cloned().unwrap_or(json!("")), "session": session});
            if a.get("wait_navigation").and_then(|v| v.as_bool()).unwrap_or(false) {
                body["wait_navigation"] = json!(true);
            }
            let (code, data) = http_call("/click", "POST", Some(&body));
            if ok(code) { text_content(compact(data), false) }
            else { text_content(format!("click failed ({}): {}", code, compact(data)), true) }
        }
        "hover" => {
            let body = json!({
                "selector": a.get("selector").cloned().unwrap_or(json!("")),
                "session": session
            });
            let (code, data) = http_call("/hover", "POST", Some(&body));
            if ok(code) { text_content(compact(data), false) }
            else { text_content(format!("hover failed ({}): {}", code, compact(data)), true) }
        }
        "key" => {
            let body = json!({
                "key": a.get("key").cloned().unwrap_or(json!("")),
                "selector": a.get("selector").cloned().unwrap_or(json!("")),
                "session": session
            });
            let (code, data) = http_call("/key", "POST", Some(&body));
            if ok(code) { text_content(compact(data), false) }
            else { text_content(format!("key failed ({}): {}", code, compact(data)), true) }
        }
        "viewport" => {
            let body = json!({
                "width": a.get("width").cloned().unwrap_or(json!(1280)),
                "height": a.get("height").cloned().unwrap_or(json!(800)),
                "session": session
            });
            let (code, data) = http_call("/sessions/viewport", "POST", Some(&body));
            if ok(code) { text_content(compact(data), false) }
            else { text_content(format!("viewport failed ({}): {}", code, compact(data)), true) }
        }
        "scroll" => {
            let body = json!({
                "y": a.get("y").cloned().unwrap_or(json!(0)),
                "selector": a.get("selector").cloned().unwrap_or(json!("")),
                "session": session
            });
            let (code, data) = http_call("/scroll", "POST", Some(&body));
            if ok(code) { text_content(compact(data), false) }
            else { text_content(format!("scroll failed ({}): {}", code, compact(data)), true) }
        }
        "upload" => {
            let body = json!({
                "selector": a.get("selector").cloned().unwrap_or(json!("")),
                "filename": a.get("filename").cloned().unwrap_or(json!("upload.bin")),
                "content_base64": a.get("content_base64").cloned().unwrap_or(json!("")),
                "mime": a.get("mime").cloned().unwrap_or(json!("application/octet-stream")),
                "session": session
            });
            let (code, data) = http_call("/upload", "POST", Some(&body));
            if ok(code) { text_content(compact(data), false) }
            else { text_content(format!("upload failed ({}): {}", code, compact(data)), true) }
        }
        "type" => {
            let body = json!({
                "selector": a.get("selector").cloned().unwrap_or(json!("")),
                "value": a.get("value").cloned().unwrap_or(json!("")),
                "session": session
            });
            let (code, data) = http_call("/type", "POST", Some(&body));
            if ok(code) { text_content(compact(data), false) }
            else { text_content(format!("type failed ({}): {}", code, compact(data)), true) }
        }
        "evaluate" => {
            let body = json!({"js": a.get("js").cloned().unwrap_or(json!("")), "session": session});
            let (code, data) = http_call("/evaluate", "POST", Some(&body));
            if ok(code) { text_content(compact(data), false) }
            else { text_content(format!("evaluate failed ({}): {}", code, compact(data)), true) }
        }
        "wait" => {
            let mut body = json!({"selector": a.get("selector").cloned().unwrap_or(json!("")), "session": session});
            if let Some(ms) = a.get("ms").and_then(|v| v.as_u64()) {
                body["ms"] = json!(ms);
            }
            let (code, data) = http_call("/wait", "POST", Some(&body));
            if ok(code) { text_content(compact(data), false) }
            else { text_content(format!("wait failed ({}): {}", code, compact(data)), true) }
        }
        "sessions" => {
            let (code, data) = http_call("/sessions", "GET", None);
            if ok(code) { text_content(compact(data), false) }
            else { text_content(format!("sessions failed ({})", code), true) }
        }
        "state_export" => {
            let mut body = json!({"session": sget("session", "default")});
            if let Some(store) = a.get("store").and_then(|v| v.as_str()) {
                body["store"] = json!(store);
            }
            let (code, data) = http_call("/sessions/state", "POST", Some(&body));
            if ok(code) { text_content(compact(data), false) }
            else { text_content(format!("state_export failed ({}): {}", code, compact(data)), true) }
        }
        "state_import" => {
            let store = a.get("store").and_then(|v| v.as_str()).map(|s| s.to_string());
            let body = if let Some(store) = store {
                // keychain mode: only the name crosses the wire
                json!({"session": session, "store": store})
            } else if a.get("cookies").is_some() {
                let raw = a.get("cookies").and_then(|v| v.as_str()).unwrap_or("");
                let parsed: Value = serde_json::from_str(raw).unwrap_or_else(|_| a.clone());
                let cookies = parsed.get("cookies").cloned().unwrap_or_else(|| parsed.clone());
                json!({"session": session, "cookies": cookies})
            } else {
                return text_content(
                    "state_import needs either cookies (from state_export) or store (a keychain name saved with state_export store)".to_string(), true);
            };
            let (code, data) = http_call("/sessions/load", "POST", Some(&body));
            if ok(code) { text_content(compact(data), false) }
            else { text_content(format!("state_import failed ({}): {}", code, compact(data)), true) }
        }
        "session_close" => {
            let (code, data) = http_call("/sessions/close", "POST", Some(&json!({"session": session})));
            if ok(code) { text_content(compact(data), false) }
            else { text_content(format!("close failed ({})", code), true) }
        }
        "session_show" => {
            let (code, data) = http_call("/sessions/show", "POST", Some(&json!({"session": session})));
            if ok(code) { text_content(compact(data), false) }
            else { text_content(format!("show failed ({}): {}", code, compact(data)), true) }
        }
        "session_hide" => {
            let (code, data) = http_call("/sessions/hide", "POST", Some(&json!({"session": session})));
            if ok(code) { text_content(compact(data), false) }
            else { text_content(format!("hide failed ({}): {}", code, compact(data)), true) }
        }
        "login" => {
            let mut body = json!({"site": sget("site", ""), "session": session});
            if let Some(b) = a.get("submit").and_then(|v| v.as_bool()) {
                body["submit"] = json!(b);
            }
            let (code, data) = http_call("/login", "POST", Some(&body));
            if ok(code) { text_content(compact(data), false) }
            else { text_content(format!("login failed ({}): {}", code, compact(data)), true) }
        }
        _ => text_content(format!("unknown tool {name}"), true),
    }
}

fn raw_post(path: &str, body: &Value) -> (u16, Vec<u8>) {
    let mut stream = match TcpStream::connect(("127.0.0.1", port())) {
        Ok(s) => s,
        Err(_) => return (0, Vec::new()),
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(130)));
    let body_s = body.to_string();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body_s.len(),
        body_s
    );
    if stream.write_all(req.as_bytes()).is_err() {
        return (0, Vec::new());
    }
    let mut buf = Vec::new();
    use std::io::Read;
    if stream.read_to_end(&mut buf).is_err() {
        return (0, Vec::new());
    }
    let Some(idx) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        return (0, Vec::new());
    };
    let status = String::from_utf8_lossy(&buf[..idx])
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (status, buf[idx + 4..].to_vec())
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(data)
}

// ---------- MCP stdio loop

fn emit(v: &Value) {
    let line = serde_json::to_string(v).unwrap_or_default();
    println!("{line}");
    let _ = std::io::stdout().flush();
}

fn reply(id: &Value, result: Value) {
    emit(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
}

fn reply_error(id: &Value, code: i64, message: &str) {
    emit(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}));
}

pub fn run() {
    log(&format!("navette-mcp starting — proxying to http://127.0.0.1:{}", port()));
    let stdin = std::io::stdin();
    let mut handle = stdin.lock();
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 65536];
    loop {
        use std::io::Read;
        match handle.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line[..line.len() - 1]).to_string();
            if line.trim().is_empty() {
                continue;
            }
            let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
            let Some(method) = msg.get("method").and_then(|v| v.as_str()).map(String::from) else { continue };
            let null_id = json!(null);
            let id = msg.get("id").cloned().unwrap_or_else(|| null_id.clone());
            let is_notification = msg.get("id").is_none();

            match method.as_str() {
                "initialize" => {
                    let pv = msg
                        .pointer("/params/protocolVersion")
                        .and_then(|v| v.as_str())
                        .unwrap_or("2025-06-18")
                        .to_string();
                    reply(&id, json!({
                        "protocolVersion": pv,
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "navette", "version": env!("CARGO_PKG_VERSION")}
                    }));
                }
                "ping" => reply(&id, json!({})),
                "tools/list" => reply(&id, json!({"tools": tools()})),
                "tools/call" => {
                    let params = msg.get("params").cloned().unwrap_or(json!({}));
                    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let args = params.get("arguments").cloned().unwrap_or(json!({}));
                    reply(&id, run_tool(&name, &args));
                }
                _ => {
                    if !is_notification {
                        reply_error(&id, -32601, &format!("method not found: {method}"));
                    }
                }
            }
        }
    }
}
