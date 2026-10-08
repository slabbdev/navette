# navette — security posture

This document is the reference for what navette guarantees, how it is enforced, and what it deliberately does not promise. Last audited: **2026-10-08** (v1.8.1).

## Scope and threat model

navette is a **single-user, loopback-only** automation server: it drives the OS WebView so that one local agent (or one local human) can read and act on the web. The threats in scope:

1. **Hostile web content** — pages the agent visits run JS inside the system WebView sandbox, never in the host process. navette's own API is not reachable from page JS (no bridge from web content to the HTTP server).
2. **Other local processes** — anything on the machine can talk to 127.0.0.1. The mitigation is `--token` (below); the default, no-token mode is a trust-the-machine model, stated plainly in the README.
3. **Cross-site/cross-session contamination** — logins from one agent session must not leak into another, and nothing should hit disk that isn't asked for.
4. **DNS rebinding** — a public site rebinds to 127.0.0.1 and fetches the API from the browser.

Out of scope: multi-user machines without `--token`, remote exposure (the server cannot bind a non-loopback interface), and defense against the local user themselves.

## Guarantees, and where they are enforced

| Guarantee | Enforcement | Verified by |
|---|---|---|
| Loopback only | `TcpListener::bind(("127.0.0.1", port))` at both listener sites; no flag or env changes it | code review |
| Optional bearer auth | every route except `/health` requires `Authorization: Bearer` or `X-Navette-Token`; comparison is constant-time (byte-fold XOR, no early exit) | live test: no-token 401 / good 200 / bad 401 |
| DNS-rebinding guard | non-local `Host` headers (anything but `127.0.0.1`, `localhost`, `[::1]`) get 403 on everything except `/health` | live test: `Host: evil.com` → 403 |
| Cross-session cookie isolation | macOS: non-persistent `WKWebsiteDataStore` per session · Windows: one WebView2 **profile per session** + InPrivate (InPrivate alone shares one profile across controllers — measured, see [#6](https://github.com/slabbdev/navette/issues/6)) · Linux: fresh `WebContext` per webview, no cookie DB ever hits disk | **CI assertion on all three OSes**: cookie imported into session A must not appear in session B's exported jar, or the build fails |
| Cross-run state is explicit | nothing persists a session's cookies across restarts on any engine; state moves only through `state_export`/`state_import` (token-gated) | audit [#6](https://github.com/slabbdev/navette/issues/6) + CI |
| Secrets never logged | `--proxy` URLs are credential-redacted (`scheme://***@host`) before any log line; tokens are never printed | code review |
| Request size bounded | HTTP bodies are capped at 10 MB; larger requests are dropped | code review |
| Injection-safe JS templating | every string interpolated into page JS goes through serde_json string escaping (`jstr`) | code review |
| Uploads stay page-side | `/upload` builds a page-side `DataTransfer` — the server never writes the visitor's filesystem | code review |

## Known residuals, stated honestly

- **Linux disk footprint**: WebKitGTK writes HTTP cache and HSTS state to `~/.cache/navette` and `~/.local/share/navette`. No cookies, no logins — a fingerprint/disk surface only. Fixing it needs the wry ephemeral-context lifetime bug (wry 0.57 is current; there is no upgrade path today). Tracked in [#6](https://github.com/slabbdev/navette/issues/6).
- **[#7](https://github.com/slabbdev/navette/issues/7)**: on WebKitGTK, `/evaluate` scripts that *write* `document.cookie` time out (reads work). Workaround: `/sessions/load` for cookie writes. No security impact; a correctness bug.
- **Session-create failures are slow**: if WebView/WebView2 session creation fails, callers wait out their command timeout (20–45 s) instead of getting a fast 500 with the cause. Refactor note in `backend_wry.rs` (`GetOrCreate` should carry `Result`).
- **`state_export` returns live session cookies** — it is the product (state portability) and the risk (jar exfiltration by any local process when `--token` is unset). The loopback trust model covers this; `--token` closes it.
- **macOS keychain / credential managers inside sessions** are whatever the WebView allows — navette adds no credential storage of its own.

## Reporting

Open an issue (or a security advisory if you prefer non-public disclosure) at https://github.com/slabbdev/navette/security/advisories. The bar is the one this doc holds itself to: measured evidence, not assumptions — the way every guarantee above was earned.
