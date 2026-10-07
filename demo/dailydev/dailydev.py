#!/usr/bin/env python3
"""Publish a post on daily.dev through navette — no API, just the real UI.

daily.dev has no public write API, so this drives the actual web app over
navette's HTTP primitives: cookie state import, navigate, click, type, read,
screenshot. It is also a deliberate real-world workout for navette.

Three commands:

  login   one-time, human-assisted: opens a VISIBLE navette window on the
          daily.dev login page, you log in (GitHub or email recommended —
          Google often refuses embedded WebViews), the script polls until the
          session cookie lands, then exports it to ./state.json (chmod 600).
  check   verifies state.json still authenticates (prints who you are).
  post    FILE.md [--publish] [--title "Override"]
          opens the composer at /squads/create, flips it to Markdown mode,
          /type's the title and body into real <textarea>s (React-safe native
          setter), reads the values back to verify, screenshots the preview.
          Without --publish it stops there — dry-run. With --publish it clicks
          "Post" and waits for the client-side route to /posts/<slug>.

Post file format: the first `# Heading` is the title (max 250 chars), the rest
is the body in markdown (max 10,000 chars — daily.dev's own limits).

Environment: NAVETTE (default http://127.0.0.1:8765), STATE (default
./state.json next to this script). The login command needs a navette build
that serves POST /sessions/show (the visible-window primitive).

Facts the selectors rest on (dailydotdev/apps, packages/shared, 2026-10):
  - composer page  /squads/create → <form id="smart_composer">
  - title          <textarea name="title" aria-label="Post title">
  - markdown mode  button[aria-label="Switch to Markdown"] → plain textarea
  - publish        button[form="smart_composer"][type="submit"] labeled "Post"
  - auth           cookie-based (credentials: 'include'); boot cache in
                   localStorage 'boot:local' (.user => logged in)
  - success        client-side route push to /posts/<slug>
"""

import argparse
import json
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

NAVETTE = os.environ.get("NAVETTE", "http://127.0.0.1:8765")
STATE = Path(os.environ.get("STATE", Path(__file__).resolve().parent / "state.json"))
SESSION = "dailydev"

COMPOSER_URL = "https://app.daily.dev/squads/create"
TITLE_MAX = 250
BODY_MAX = 10_000

SEL_FORM = 'form#smart_composer'
SEL_TITLE = 'textarea[name="title"]'
SEL_MD_TOGGLE = '[aria-label="Switch to Markdown"]'
SEL_MD_BODY = 'textarea[placeholder="Share your thoughts"]'
SEL_PUBLISH = 'button[form="smart_composer"][type="submit"]'


# ---------- navette HTTP client (stdlib only) ----------

def api(path, body=None, raw=False, timeout=60):
    """POST JSON to the navette server; return parsed JSON (or raw bytes)."""
    url = f"{NAVETTE}{path}"
    data = json.dumps(body or {}).encode()
    req = urllib.request.Request(url, data=data, method="POST",
                                 headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            payload = r.read()
    except urllib.error.HTTPError as e:
        die(f"navette {path} -> HTTP {e.code}: {e.read().decode(errors='replace')[:200]}")
    except urllib.error.URLError as e:
        die(f"cannot reach navette at {NAVETTE} ({e.reason}) — is `navette serve` running?")
    if raw:
        return payload
    out = json.loads(payload)
    if out.get("ok") is False:
        die(f"navette {path} failed: {out}")
    return out


def nav_navigate(url):
    return api("/navigate", {"url": url, "session": SESSION})


def nav_evaluate(js):
    res = api("/evaluate", {"js": js, "session": SESSION})
    raw = res.get("result")
    if raw in (None, ""):
        return None
    try:
        return json.loads(raw)
    except (ValueError, TypeError):
        return raw  # engine returned a bare string, not JSON-encoded


def nav_click(selector, wait_navigation=False):
    return api("/click", {"selector": selector, "session": SESSION,
                          "wait_navigation": wait_navigation})


def nav_type(selector, value):
    res = api("/type", {"selector": selector, "value": value, "session": SESSION})
    if res.get("result") != '"OK"' and "OK" not in str(res.get("result")):
        # /type returns {ok: true, result: "\"OK\""} on hit, "MISSING" otherwise
        if "MISSING" in str(res.get("result")):
            die(f"type: element not found: {selector}")
    return res


def nav_wait(selector, ms=15000):
    try:
        return api("/wait", {"selector": selector, "ms": ms, "session": SESSION})
    except SystemExit:
        return None


def nav_screenshot(path):
    png = api("/screenshot", {"session": SESSION}, raw=True, timeout=90)
    Path(path).write_bytes(png)
    return len(png)


def die(msg):
    print(f"  ✗ {msg}", file=sys.stderr, flush=True)
    sys.exit(1)


def info(msg):
    print(f"  · {msg}", flush=True)


# ---------- session state (cookies = the login) ----------

def logged_in_probe(session=None):
    """Who the SPA thinks it is — the boot cache persists the user, never the
    token (that rides in cookies), so `boot:local` is our cheap truth source.
    Always JSON.stringify on the JS side: the macOS eval bridge returns
    NSDictionary descriptions for raw objects, which is not JSON."""
    res = api("/evaluate", {
        "js": "(function(){try{var b=JSON.parse(localStorage.getItem('boot:local')||'null');"
              "var u=b&&b.user;return JSON.stringify({origin:location.origin, path:location.pathname,"
              "user:(u&&(u.name||u.username||u.id))||null});}catch(e){return JSON.stringify({err:String(e)})}})()",
        "session": session or SESSION})
    raw = res.get("result")
    if raw in (None, ""):
        return None
    try:
        return json.loads(raw)
    except (ValueError, TypeError):
        return raw


def save_state():
    cookies = api("/sessions/state", {"session": SESSION})
    STATE.write_text(json.dumps(cookies, indent=2))
    os.chmod(STATE, 0o600)
    info(f"cookies saved to {STATE} (0600) — this file IS the login, keep it secret")
    return cookies


def load_state():
    if not STATE.exists():
        die(f"no {STATE} — run `{sys.argv[0]} login` once first")
    return json.loads(STATE.read_text())


def restore_state():
    cookies = load_state()
    if isinstance(cookies, dict) and "cookies" in cookies:
        cookies = cookies["cookies"]
    api("/sessions/load", {"session": SESSION, "cookies": cookies})
    info(f"imported {len(cookies) if isinstance(cookies, list) else '?'} cookies into session '{SESSION}'")


# The session cookies daily.dev sets: `__Secure-dast` (email login, ~7 days)
# or `__Secure-daily.state` (GitHub OAuth flow, ~20 min rolling) — a state
# snapshot is only as durable as the longest-lived one it holds.
SESSION_COOKIES = ("__Secure-dast", "__Secure-daily.state")


def token_guard():
    """daily.dev's session cookies are short-lived (20 min for the GitHub
    flow, ~7 days for email) — a state.json snapshot is perishable. Refuse to
    drive the composer with a token about to die mid-post."""
    state = load_state()
    left = max(
        ((c.get("expires") or 0) for c in (state.get("cookies") or [])
         if c.get("name") in SESSION_COOKIES),
        default=0,
    ) - time.time()
    if left < 120:
        die(f"daily.dev session token expires in {max(left, 0):.0f}s — "
            f"re-run `login` right before posting")
    info(f"session token still valid for {left/3600:.1f} h")


# ---------- commands ----------

def cmd_login(args):
    print("== daily.dev login (one-time, human in the loop) ==")
    nav_navigate("https://app.daily.dev/")
    api("/sessions/show", {"session": SESSION})
    print("""
  ┌────────────────────────────────────────────────────────────────┐
  │  A navette window is now on screen, on daily.dev.              │
  │  Log in there — GitHub or email work best in embedded          │
  │  WebViews; Google may refuse. Solve any captcha yourself.      │
  │  This script polls until the login cookie lands (10 min max).  │
  └────────────────────────────────────────────────────────────────┘""")
    deadline = time.time() + 600
    while time.time() < deadline:
        probe = logged_in_probe() or {}
        if probe.get("user"):
            print(f"\n  ✓ logged in as {probe['user']!r}")
            break
        time.sleep(2)
    else:
        api("/sessions/hide", {"session": SESSION})
        die("timed out waiting for the login (10 min) — run login again")
    cookies = save_state()
    api("/sessions/hide", {"session": SESSION})
    api("/sessions/close", {"session": SESSION})
    # Roundtrip check, immediately: import the just-exported cookies into a
    # FRESH session and boot the SPA. A broken export (auth cookie lost) must
    # fail HERE, not later at post time.
    v = SESSION + "-verify"
    vcookies = cookies.get("cookies", cookies) if isinstance(cookies, dict) else cookies
    api("/sessions/load", {"session": v, "cookies": vcookies})
    # Verify on an APP route (the root redirects to the marketing origin,
    # whose localStorage has no boot:local — a false "logged out").
    api("/navigate", {"url": COMPOSER_URL, "session": v})
    time.sleep(4)
    vprobe = logged_in_probe(v) or {}
    api("/sessions/close", {"session": v})
    if vprobe.get("user"):
        print(f"  ✓ roundtrip verified: the saved state re-authenticates as {vprobe['user']!r}")
        print("  ✓ window hidden, session closed. Next: `post your-file.md` (dry-run)")
    else:
        die(f"login cookie did NOT survive the export/import roundtrip "
            f"(fresh session landed on {vprobe.get('path')}) — do NOT close the "
            f"window early next time; re-run `login`")


def cmd_check(args):
    print("== checking saved daily.dev state ==")
    token_guard()
    restore_state()
    # Probe on an APP route, never the root: app.daily.dev/ redirects to the
    # marketing site at daily.dev — a different ORIGIN whose localStorage has
    # no boot:local, which reads as "logged out" even when the cookies work.
    nav_navigate(COMPOSER_URL)
    time.sleep(3)
    probe = logged_in_probe() or {}
    if probe.get("user"):
        print(f"  ✓ authenticated as {probe['user']!r} (on {probe.get('path')})")
    else:
        die(f"NOT authenticated (boot cache on {probe.get('path')} has no user) — re-run `login`")
    api("/sessions/close", {"session": SESSION})


def parse_post(path, title_override=None):
    text = Path(path).read_text()
    m = re.match(r"\s*#\s+(.+)", text)
    title = (title_override or (m.group(1).strip() if m else "")).strip()
    body = text[m.end():].strip() if m else text.strip()
    if not title:
        die(f"no title: put a `# Heading` first line in {path} or pass --title")
    if len(title) > TITLE_MAX:
        die(f"title is {len(title)} chars, daily.dev caps it at {TITLE_MAX}")
    if len(body) > BODY_MAX:
        die(f"body is {len(body)} chars, daily.dev caps it at {BODY_MAX}")
    return title, body


def cmd_post(args):
    title, body = parse_post(args.file, args.title)
    print(f"== composing on daily.dev: “{title}” ({len(body)} chars of body) ==")
    token_guard()
    restore_state()
    # Straight to the composer — an APP route. app.daily.dev/ redirects to the
    # marketing origin (daily.dev) whose localStorage has no boot:local: a
    # probe there reads "logged out" even with perfectly good cookies.
    nav_navigate(COMPOSER_URL)
    time.sleep(3)
    probe = logged_in_probe() or {}
    if not probe.get("user"):
        api("/sessions/close", {"session": SESSION})
        die(f"saved state does not authenticate (boot cache on {probe.get('path')} has no user) "
            f"— token older than ~20 min or revoked; re-run `login`")
    info(f"authenticated as {probe['user']!r}")

    if not nav_wait(SEL_FORM, 40000):
        api("/sessions/close", {"session": SESSION})
        die("composer form #smart_composer never appeared")

    # Primary path: flip the rich editor to Markdown mode — the body becomes a
    # plain <textarea>, exactly what navette's React-safe /type was built for.
    # Click the toggle ONLY on a settled form, then wait for BOTH fields: a
    # click during hydration makes React re-mount everything and the title
    # textarea blinks out of the DOM right as we'd type into it.
    if nav_wait(SEL_MD_TOGGLE, 8000):
        time.sleep(1.5)  # let React own the server-rendered shell first
        nav_click(SEL_MD_TOGGLE)
        if nav_wait(SEL_MD_BODY, 8000) and nav_wait(SEL_TITLE, 8000):
            time.sleep(0.5)
            info("composer flipped to Markdown mode (real <textarea>s)")
        else:
            info("markdown toggle clicked but no textarea appeared — staying on rich text")
    else:
        info("no markdown toggle found — falling back to URL prefill")

    typed = True
    if nav_evaluate(f"JSON.stringify(!!document.querySelector({json.dumps(SEL_MD_BODY)}))"):
        nav_type(SEL_TITLE, title)
        nav_type(SEL_MD_BODY, body)
    else:
        # Fallback: the composer prefills from query params (?title=&body=) —
        # same form, filled by the app itself instead of by /type.
        typed = False
        prefill = urllib.parse.urlencode({"title": title, "body": body})
        nav_navigate(f"{COMPOSER_URL}?{prefill}")
        nav_wait(SEL_FORM, 40000)
        info("body prefilled via /squads/create?title=…&body=…")

    # Verify what the form actually holds before any button gets clicked.
    # Retry: React re-mounts the textareas around the markdown switch and the
    # draft autosave — a read in that window sees detached nodes (title None).
    got = None
    for attempt in range(3):
        time.sleep(1.5 if attempt else 0.5)
        got = nav_evaluate(
            "(function(){var t=document.querySelector(" + json.dumps(SEL_TITLE) + ");"
            "var b=document.querySelector(" + json.dumps(SEL_MD_BODY) + ")||"
            "  document.querySelector('.ProseMirror[contenteditable]');"
            "return JSON.stringify({title:t?t.value:null, body:b?(b.value!==undefined?b.value:b.innerText):null}})()") or {}
        if got.get("title") == title and len(got.get("body") or "") >= len(body) * 0.5:
            break
    if got.get("title") != title:
        api("/sessions/close", {"session": SESSION})
        die(f"title verification failed — form holds {got.get('title')!r}")
    body_got = (got.get("body") or "").strip()
    if len(body_got) < len(body) * 0.5:
        api("/sessions/close", {"session": SESSION})
        die(f"body verification failed — form holds {len(body_got)} of {len(body)} chars")
    info(f"verified: title + body are in the form ({'typed via /type' if typed else 'URL prefill'})")

    shot = Path(args.file).with_name(Path(args.file).stem + "-preview.png")
    size = nav_screenshot(shot)
    info(f"preview screenshot -> {shot} ({size // 1024} KB)")

    if not args.publish:
        api("/sessions/close", {"session": SESSION})
        print(f"\n  ✓ DRY-RUN complete — nothing was published. Eyeball {shot.name},")
        print(f"    then re-run with --publish to click “Post” for real.")
        return

    print("  · clicking “Post” …")
    nav_click(SEL_PUBLISH)
    deadline = time.time() + 45
    slug = None
    while time.time() < deadline:
        where = nav_evaluate("JSON.stringify(location.pathname)") or ""
        if isinstance(where, str) and where.startswith("/posts/"):
            slug = where
            break
        time.sleep(1)
    api("/sessions/close", {"session": SESSION})
    if slug:
        print(f"\n  ✓ PUBLISHED — https://app.daily.dev{slug}")
    else:
        die("publish clicked but no /posts/<slug> route within 45 s — "
            "check https://app.daily.dev manually before retrying")


def main():
    try:
        sys.stdout.reconfigure(line_buffering=True)  # live logs even when piped
    except AttributeError:
        pass
    p = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    sub.add_parser("login", help="one-time human-assisted login; saves state.json")
    sub.add_parser("check", help="verify the saved state still authenticates")
    sp = sub.add_parser("post", help="compose a post from a markdown file")
    sp.add_argument("file", help="markdown file: first `# Heading` is the title")
    sp.add_argument("--title", help="override the title")
    sp.add_argument("--publish", action="store_true",
                    help="actually click Post (default: dry-run with screenshot)")
    args = p.parse_args()
    {"login": cmd_login, "check": cmd_check, "post": cmd_post}[args.cmd](args)


if __name__ == "__main__":
    main()
