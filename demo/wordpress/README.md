# WordPress smoke E2E — navette vs Playwright, same journeys, measured

The counter-demo to *"Playwright for WordPress: a practical guide"* ([softwaretestinghelp
magazine, via daily.dev](https://www.softwaretestingmagazine.com/knowledge/playwright-for-wordpress-a-practical-guide-to-automating-end-to-end-tests/)):
the same journeys the guide describes — install, login, publish in the block
editor, verify the frontend, leave a comment — run twice against the same
disposable docker WordPress, once on each engine. Everything below was
measured on one machine (M1), fresh container per run, 2026-10-09.

## Run it

```sh
docker compose up -d                                # WordPress on http://127.0.0.1:8090

NAVETTE=http://127.0.0.1:8765 python3 wp-smoke.py   # navette — 6 journeys, exit code
node wp-smoke-playwright.js                         # Playwright twin (uses ../../bench's install)
```

Both print a journey table and exit 0/1. Screenshots land in
`artifacts/` and `artifacts-playwright/`. `docker compose down -v` to erase.

## The numbers

| | navette | Playwright 1.63 + Chromium |
|---|---|---|
| **Suite runtime** (6 journeys, fresh WP) | **20.7–23.2 s** | **8.4 s** |
| One-time engine setup | **0 s — the OS WebView, already installed** | 74 s download, 182 MB over the wire, **557 MB on disk** |
| Binary footprint | 626 KB | ~5 MB npm + the 557 MB browser cache |

Read the runtime row honestly: **Playwright wins it.** Auto-waiting returns
the instant an element is actionable; navette's smoke polls conservatively
(its `/wait` fixed at 150 ms ticks, plus deliberate settle beats). The
setup row is where the thesis lives: for a CI runner or a laptop, navette is
*already installed by the OS* — the engine costs nothing to fetch, cache or
update, ever. Three engines come free with the OS matrix (WebKit on macOS,
WebView2 = Chromium on Windows, WebKitGTK on Linux) — cross-browser coverage
without one browser download.

## Same journeys, four different flake classes

Getting the Playwright twin green took four fixes that navette's suite never
needed — each is the article's own "best practices" pain list live:

1. **Hidden field, actionability refusal.** WordPress hides the install
   wizard's confirm-password box when the password is strong; Playwright
   waits 20 s for it to become visible and times out. navette's `/type`
   (synthetic React-safe setter) fills it blind.
2. **Strict-mode ambiguity.** `#wp-admin-bar-my-account .display-name`
   matches 3 nodes; Playwright's strict mode throws, `querySelector` takes
   the first.
3. **Modal interception.** The Gutenberg welcome guide's overlay intercepts
   pointer events — Playwright cannot click the title underneath. navette
   pastes through DOM events inside the canvas iframe; the overlay is
   irrelevant.
4. **Logged-in context drift.** Running the comment journey in the admin
   context makes WordPress hide the name/email fields (known user) — the
   fix is a fresh anonymous context, which the navette suite had by design
   (separate `wpfront` session).

For fairness: the navette suite had its own two bugs on the way to green —
a form-wait race (fixed with a real wait loop) and an unquoted CSS attribute
selector. Script bugs on both sides; the point is *which engine quirks*
each stack hit.

## Findings worth keeping

- **Gutenberg's canvas is a same-origin blob iframe** (WP 6.3+). navette has
  no iframe API — none needed: `/evaluate` hops through `contentDocument`,
  the title takes a synthetic `ClipboardEvent` paste, the first paragraph
  spawns by clicking the default block appender (pointer events), then takes
  the same paste. Both engines' canvas handling is one hop; Playwright
  spells it `frameLocator`, navette spells it standard DOM.
- **`storageState` is a cookie jar by another name.** The guide's answer to
  repeated logins is `context.storageState()`; navette's is
  `POST /sessions/state` → `wp-state.json` → `POST /sessions/load`. Same
  concept, same one-file portability — and `navette creds set` + `POST /login`
  go further: the wp-admin password stays in the OS keychain, origin-bound,
  and the agent never sees it.
- **What this suite is not:** no `expect()` vocabulary, no trace viewer, no
  parallel contexts. Smoke tier — "does the journey still work" — with
  plain asserts over `read`/`evaluate`. The trade is deliberate: zero engine
  footprint in exchange for framework comfort.
