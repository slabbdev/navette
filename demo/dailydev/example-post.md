# Ship your agent's browser, not Chromium

Posting this from a script driving navette — a 626 KB Rust binary that
browses through the WebView your OS already ships.

What the automation needed, and what the OS WebView gave it for free:

- **A session** — cookies export/import, like a `storageState`
- **Hands** — `click` (real mouse events) and `type` (React-safe native setter)
- **Eyes** — markdown `read` and PNG `screenshot` for verification before publish
- **A window** — `session_show`, because OAuth needs a human exactly once

No API on the target site. No headless-Chromium download. No puppet strings
attached to a 218 MB browser.

*This is the example file that ships with the recipe — edit it, then:*
`dailydev.py post example-post.md`
