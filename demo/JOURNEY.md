# The navette journey — a real agent session, end to end

What every agent is asked to do on the web, in one run: **log into a site, then
research something** (search → open → extract). Executed live on 2026-09-30 by
an AI agent driving navette **through MCP only** — no Playwright, no Chromium,
no curl. The agent's actual tool calls, verbatim.

## Act 1 — Authenticate (the most-requested agent flow)

Target: `the-internet.herokuapp.com/login` — the standard automation-practice
site, real HTTP form, real session cookie.

```jsonc
// navigate
{"title":"The Internet","session":"journey","url":"https://the-internet.herokuapp.com/login","ok":true}
// type #username = "tomsmith"      -> {"result":"OK","ok":true}
// type #password = "************"  -> {"result":"OK","ok":true}
// click 'form button[type="submit"]' -> {"result":"OK","ok":true}
// evaluate — verify the outcome
{"url":"/secure","flash":"You logged into a secure area!","h2":"Secure Area"}
```

The submit button fired a **real form POST**: the browser navigated to `/secure`,
the session cookie was stored by the WKWebView, and the page rendered its
authenticated state.

![Login success](demo/01-login-success.png)

## Act 2 — Research loop (search → open → extract)

Target: DuckDuckGo's HTML endpoint (no bot wall), then follow the most relevant
result.

```jsonc
// navigate: https://html.duckduckgo.com/html/?q=system+webkit+browser+for+AI+agents
{"title":"system webkit browser for AI agents at DuckDuckGo","ok":true}
// evaluate — extract the first five results
[
  {"title":"11 Best AI Browser Agents in 2026","url":"https://www.firecrawl.dev/blog/best-browser-agents"},
  {"title":"WebKit ships a Safari MCP server for AI coding agents","url":"…aintelligencehub.com…"},
  {"title":"AI Web Browsers & Agents in 2026: The Complete Selection Guide","url":"…dev.to…"},
  "…"
]
// first result was dead ("Deployment Paused") — a real web; agent moves on.
// navigate: the dev.to guide -> read -> extract
{"title":"AI Web Browsers & Agents in 2026: The Complete Selection Guide - DEV Community","ok":true}
{"title":"AI Web Browsers & Agents in 2026…",
 "paragraphs":["Picking the right tools for AI agents in 2026 feels like choosing a
                Swiss Army knife for a space mission…"]}
```

![Research article](demo/02-research-article.png)

Full CSS rendering, hero image, fonts — WebKit, already in the OS. The agent
*saw* every step through `screenshot` and *acted* through `type`/`click`.

## What this journey demonstrates

| Capability | Proof |
|---|---|
| Real form login with session state | `/secure` + flash + cookie, screenshot |
| Search-result extraction | 5 results with titles + URLs |
| Dead-link resilience | skipped a parked page, continued |
| Content extraction for LLMs | markdown `read` + targeted `evaluate` |
| Visual verification | PNG screenshots returned as MCP image content |
| Zero Chromium anywhere | system WebKit, 0.3 MB of glue |

## Found along the way (competitive intel)

The search itself surfaced the landscape this project lives in: *"WebKit ships
a Safari MCP server for AI coding agents"* (2026) — Apple is moving toward the
same thesis: the system browser is the agent's browser. navette's bet is the
cross-platform, engine-free, agent-first version of that idea.
