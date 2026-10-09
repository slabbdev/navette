# DEV.to notifications + comments — the no-API recipe, via navette

The Forem API stops exactly where the social graph gets interesting:
`POST /api/comments` is a **404** (verified 2026-10-08 — the API is
read-only for comments), notifications are not in v0 either. `devto.py`
drives the real site through navette with the user's own logged-in
session, and keeps the login in the **OS keychain** (`navette store`,
[#8](https://github.com/slabbdev/navette/issues/8)) — no API key, no
`state.json` sidecar.

```
python3 devto.py login                 # one-time, human in the visible window
python3 devto.py check                 # who am I
python3 devto.py notifications         # the private /notifications page
python3 devto.py comment URL FILE      # markdown file → composer, verified
python3 devto.py reply COMMENT-URL FILE
```

Every write is **dry-run by default**: the composer is filled, read back
character-exact, screenshotted — and nothing is submitted without
`--publish`. One action per run, no retry loops.

## Login (the one human step)

dev.to's email login is a **magic link** — useless inside a ghost window
(the link opens in your default browser, not the session). The workable
human path: the visible window (`session_show`), click **Continue with
GitHub** (or Google/Apple…), complete the OAuth there. The script polls
for the navbar avatar, then stores the session under the `devto` keychain
name. The password never transits the chat or the script.

## DOM facts (verified 2026-10-09)

| What | Where |
|---|---|
| comment composer | `textarea#text-area` · `name="comment[body_markdown]"` · placeholder "Add to the discussion" |
| comment submit | `#comments button[type=submit]` ("Submit") |
| logged-in probe | `header .crayons-avatar` — **scoped to header**: the class also matches commenters' avatars |
| login page | `/enter` — OAuth buttons + email form; `/notifications` redirects to `/magic_links/new` when logged out |
| identity | sync XHR `GET /api/users/me` (session cookie auth) |
| reply flow | open the comment permalink (`#comment_<id>`), click its Reply button, the inline form reuses the composer |

## API facts that make this demo exist

- `PUT /api/articles/{id}` works (article writes) — but `POST /api/comments`
  → 404 HTML page. Comments go through the web or nothing.
- Notifications: v0 has no endpoint; v1's is API-key scoped. The page is
  the only faithful source of "who commented/reacted/followed".

## Safety

One comment per instruction; verify-before-publish; expired login state →
stop and re-run `login`; no scheduling, no bulk. Your account, your voice:
the comment file is published verbatim — read it before `--publish`.
