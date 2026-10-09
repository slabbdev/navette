# navette

> **The browser for agents.** A tiny MCP server over the WebView your OS already ships — no Chromium, no download, no RAM bonfire.

**Name lore.** *Navette* is French for **shuttle** — the small vessel that carries your agent from page to page. Light, always fueled (the engine ships with your OS), zero ceremony. It skips the human web's garbage — cookie banners, popups, autoplay — so your agent never has to see it.
(Mascot: a grumpy shuttle. It does the nav. It's literally its job.)

**Not a scraper — an actor.** navette doesn't fetch pages for later parsing; it *acts*: opens sessions, fills forms, clicks through flows, verifies with screenshots, keeps state across steps. Scraping is the demo; acting is the product.

---

## The problem

Every agent that touches the web today drags along the same stack: headless Chromium.

- **Playwright/Puppeteer** download ~300–400 MB of browser per machine.
- **~100–200 MB of RAM per open page**; agents run many.
- Chrome is driven by **CDP**, a protocol built for DevTools, not for agents.
- Cloud alternatives (Browserbase, Steel, Hyperbrowser) are full Chrome **as a paid subscription**.

Meanwhile, what an agent actually needs is small: *go to a page, execute its JS, read the text, take a screenshot, click, type.*

## Who is this really for

The agent web has two halves, and they don't need the same browser:

- **Server fleets** (scraping at scale, cloud browser APIs): Linux boxes running Chromium headless. Real, big, already served. Not our fight today.
- **Local agents** (MCP clients, desktop assistants, CLI agents, IDEs): the agent runs *on the user's machine*, which already ships a full engine. This half is exploding in 2026 — and here, downloading 400 MB of Chromium to read one page is a blocking defect. **This is navette's beachhead: the agents that live where you live.**

Windows deserves its own line: the system WebView there is **WebView2 — Chromium**, preinstalled on every Windows 10/11. So on Windows, navette delivers Chromium-grade rendering from a 0.6 MB binary with zero download. The Chromium without the Chromium.

Linux servers are a roadmap milestone, not a promise: WebKitGTK is a system package (`apt install`, not a proprietary 400 MB download), and the 8 primitives are engine-agnostic — but against Chromium fleets, navette would win RAM, not rendering fidelity. Analogy: **headless Chromium is the Postgres of scraping; navette is the embedded SQLite.** SQLite won because nobody wants to run a server to hold a little state; navette wins because nobody wants to download a browser to read one page.

## Engine strategy: system-first, embedded fallback

"No system WebView" is a container-shaped problem, and it has a layered answer:

- **Tier 1 — system engine (the 95% case).** WKWebView on macOS, WebView2 on Windows: always present, security-patched by the OS. 0.6 MB of glue, the headline stays true.
- **Tier 2 — official embed.** WebView2 ships a **Fixed Version** mode: a pinned runtime bundled with the app, Microsoft-sanctioned. macOS never needs it.
- **Tier 3 — bare Linux containers.** Ship the engine *the distro way*: a Dockerfile with `webkit2gtk` via apt (patched by the distribution, never a fossil we vendored) — or **WPE WebKit**, WebKit's embedded port built for exactly this (TVs, cars, headless boxes, ~60 MB, no desktop stack). Still 2–7x lighter than Chromium with full rendering.

The industrial path for one-codebase-three-engines: **Rust + `wry`** (Tauri's webview layer: WKWebView, WebView2, WebKitGTK behind one API), keeping the WPE option open. The Swift prototype remains the reference implementation of the 8 primitives. What we will not do is vendor a browser the way Playwright does — that trade is the thing navette exists to refuse.

## The idea

A **native WebView** — the one your platform already ships: **WKWebView** on macOS, **WebView2** on Windows, **WebKitGTK** on Linux — driven by a small local server that speaks **MCP**, the protocol agents already speak natively (Claude, and friends). One server per platform, same 8 primitives, same MCP surface.

- **Zero engine download.** The browser is your OS. navette is a few hundred KB of glue.
- **Full rendering engine**, not a partial one — real WebKit layout, real JS.
- **Agent-first surface**: 8 primitives, no DevTools baggage.

### The 8 primitives

| Primitive | What it does |
|---|---|
| `navigate` | open a URL in a session |
| `read` | page as markdown or DOM |
| `screenshot` | current viewport, PNG |
| `click` | click an element |
| `type` | fill inputs |
| `evaluate` | run JS in the page |
| `wait` | wait for selector / idle / timeout |
| `sessions` | multiple parallel named sessions |

## Landscape

| | Playwright + Chromium | Lightpanda | Browserbase (cloud) | **navette** |
|---|---|---|---|---|
| Engine download | ~300–400 MB | small binary | none (remote) | **none — engine is the OS** |
| Engine completeness | full | partial (alpha) | full | **full (system WebKit)** |
| RAM per page | ~100–200 MB | ~123 MB peak | n/a (their bill) | **WebView-native** |
| Protocol | CDP | CDP | REST | **MCP native** |
| Runs where | anywhere | anywhere | their cloud | **anywhere with a system WebView** |

## Honest trade-offs

- **No CDP.** Not a drop-in for Playwright. The bet: agents speak MCP now — and if they don't, a thin CDP shim can come later.
- **WebKit ≠ Blink.** A minority of sites are tuned for Chrome and may render slightly differently. For *reading*, this rarely matters — and on Windows, WebView2 *is* Chromium.
- **Every platform needs its own plumbing.** WKWebView, WebView2 and WebKitGTK are three different controls with three sets of quirks; the ghost-window/headless work is per OS. The concept ports everywhere; the glue doesn't. Feasible (ZCode's own browser tooling does it on macOS), not free.

## Benchmarks — the bar to clear

The claims above are claims until measured. Protocol: same machine, same page set (static, JS-heavy, SPA), medians over repeated runs.

| Metric | What it measures |
|---|---|
| Install size | what lands on disk (navette: glue only, no engine) |
| Cold start | launch → first `navigate` answered |
| Navigate → readable | `navigate` + `read` markdown round-trip |
| Act latency | `click` / `type` round-trip |
| RAM per session | idle, one page, then a 10-page session |
| Crawl | 100-page run: wall time + peak RSS |

Run against **Playwright + Chromium** and **Lightpanda**. Done — [BENCHMARKS.md](BENCHMARKS.md) publishes the numbers, reproducible with one command. navette wins install size, cold start, navigate→read, act latency and the 100-page crawl; the two rows Lightpanda takes (fresh-process boot, peak RAM) are the price of rendering, and the doc says so.

## Status

One-pager, 2026-09-29; status refreshed 2026-10-09. Name collision-checked at pitch time: `navette` free on npm, free GitHub handle, `navette.dev` and `navette.com` both unregistered — the cleanest name checked that session. Since then it shipped — **v1.9.0 (2026-10-08)**, CI green on all three OSes:

- **GitHub** — [slabbdev/navette](https://github.com/slabbdev/navette) · site: [slabbdev.github.io/navette](https://slabbdev.github.io/navette/)
- **crates.io** — `cargo install navette-browser`
- **Homebrew** — `brew install slabbdev/tap/navette` (macOS arm64)
- **Docker** — `ghcr.io/slabbdev/navette` (amd64 + arm64, stdio MCP)
- **Official MCP registry** — listed as `io.github.slabbdev/navette`

Built on the shoulders of `tinyjs wrap` (tinyjsapp). MIT.
