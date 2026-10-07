# daily.dev, posted from navette — the no-API recipe

daily.dev has no public write API. This recipe publishes a post anyway, by
driving the real web app over navette's HTTP primitives — and it earned its
keep as a real-world test: it is the reason `POST /sessions/show` exists.

```sh
# terminal 1 — a navette with /sessions/show (any build after 2026-10-07)
../../target/release/navette serve --port 8765

# terminal 2
python3 dailydev.py login                    # one-time: YOU log in, in the window that appears
python3 dailydev.py check                    # "authenticated as 'you'"
python3 dailydev.py post example-post.md     # dry-run: fills the composer, screenshots it, stops
python3 dailydev.py post example-post.md --publish   # for real
```

`NAVETTE=http://127.0.0.1:8766` to point elsewhere; state lands in
`state.json` (gitignored, chmod 600 — it **is** the login).

## The flow, primitive by primitive

| Step | navette | Notes |
|---|---|---|
| Log in (once) | `navigate` + `sessions/show` | a titled, keyable window appears; you complete GitHub/email OAuth yourself (Google often refuses embedded WebViews; Turnstile captchas are human-solved by design) |
| Capture the login | `sessions/state` | daily.dev auth is cookie-based (`credentials: 'include'`, better-auth) — navette's cookie jar is exactly the right primitive |
| Restore it | `sessions/load` + `navigate` | fresh session, logged in, no window ever shown |
| Compose | `navigate` → `/squads/create` | the deterministic composer page (the sidebar button only opens a client-side modal) |
| Title + body | `click` the "Switch to Markdown" toggle, then `type` ×2 | markdown mode swaps the TipTap editor for a plain `<textarea>` — the React-safe `/type` fills it natively |
| Fallback | `navigate` → `/squads/create?title=…&body=…` | the composer prefills from query params; used if the toggle is missing |
| Verify | `evaluate` + `screenshot` | the script re-reads both fields and refuses to publish on mismatch; the preview PNG is saved next to the post file |
| Publish | `click` the Post button | opt-in via `--publish`; success = client-side route to `/posts/<slug>` |

Dry-run is the default. Nothing clicks "Post" unless you say so.

## What the real-world test shook out of navette

This recipe ran end-to-end for real on 2026-10-08 (post published at
`app.daily.dev/posts/ship-your-agent-s-browser-not-chromium-5qljyntyg`).
Four findings, in the order they bit:

- **Ghost windows couldn't host a human.** The macOS ghost is borderless and
  parked off-screen — a borderless `NSWindow` can never become the key window,
  so a login form would swallow every keystroke. Fix: `POST /sessions/show`
  restyles the window titled, sizes the content rect back to the viewport,
  centers it, and fronts the app. Proof it worked where the old experimental
  on-screen approach failed: `document.hasFocus()` flips to `true` (the old
  path stayed `0` — which is also why macOS native key input was stuck).
- **`/wait` was broken on macOS.** Its probe compared the evaluate result to
  `"true"`, but the macOS bridge stringifies a JS boolean through NSNumber's
  `description` — which is `"1"`, so every present selector "timed out".
  Fixed: the probe coerces with `String()` and the route accepts both shapes.
- **Probe the APP origin, not the root.** `app.daily.dev/` redirects to the
  marketing site at `daily.dev` — a different origin. A `boot:local` probe
  there reads "logged out" even when the imported cookies work perfectly;
  every early "state does not authenticate" failure was this false negative.
  Always probe an app route (`/squads/create`).
- **`/type`'s textContent fallback is a trap for rich editors.** TipTap keeps
  its own model; `textContent =` + `input` event desyncs it. Flip to a real
  textarea (this recipe's markdown toggle) — `/type` is only for
  `input`/`textarea`, which is exactly what it documents. Also: read the form
  back with a retry — React re-mounts the textareas around the markdown
  switch and a read in that window sees detached nodes.

Session-cookie lifetimes (checked at login time): the **email** login sets
`__Secure-dast` (~7 days — a state.json worth keeping); the **GitHub OAuth**
flow sets `__Secure-daily.state` (~20 min rolling — re-login per session).
`token_guard()` refuses to drive the composer with a token about to die.

## Where the facts come from

daily.dev's frontend is open source (`dailydotdev/apps`, 2026-10):
`packages/shared/src/components/modals/post/SmartComposerModal.tsx` (the
`#smart_composer` form, the submit button),
`components/fields/RichTextInput.tsx` (TipTap, markdown mode, paste handling),
`lib/links.ts` (`/squads/create`), `graphql/common.ts` (cookie auth). The
selectors here are `name`/`aria-label` based — stable across restyles.

The internal GraphQL (`api.daily.dev`, `CreateFreeformPost` mutation) would
also work from inside the authenticated session, but driving the UI is the
point: it survives API churn and it is the navette test.
