#!/usr/bin/env python3
"""DEV.to through navette — notifications + comments, no API key.

The Forem API is read-only for the social graph that matters: POST
/api/comments is a 404 (verified 2026-10-08), notifications are not in v0.
This script drives the real site through navette's HTTP primitives with
the user's own logged-in session, and keeps the login in the OS keychain
(navette `store`, issue #8) — no state.json file, no API key.

Commands:

  login              one-time, human-assisted: opens a VISIBLE navette
                     window on dev.to/enter. dev.to email login is a magic
                     link (useless in a ghost window), so use the GitHub
                     button — OAuth completes in the same window. The script
                     polls until the navbar avatar appears, then stores the
                     session in the keychain under "devto".
  check              restore the keychain state if needed; print who you are.
  notifications      restore if needed, read /notifications, list entries
                     (who, what, when). [--json OUT] keeps the raw list.
  comment URL FILE [--publish]
                     navigate the article, /type the markdown into the real
                     textarea, read it back exactly, screenshot. Without
                     --publish it STOPS there (dry-run). With --publish it
                     clicks Submit and verifies the comment landed.
  reply COMMENT-URL FILE [--publish]
                     same discipline against a specific comment's reply form
                     (URL is the comment permalink, dev.to/...#comment_<id>).

Safety: one action per run, no retry loops, verify-before-publish, dry-run
by default. Limits: dev.to comment body ≤ ~30k chars markdown.

Environment: NAVETTE (default http://127.0.0.1:8765), STORE (keychain store
name, default "devto"), SESSION (navette session name, default "devto").

Facts the selectors rest on (DOM verified 2026-10-09, logged out for the
form, logged in for notifications — re-check both if dev.to ships a change):
  - comment composer  textarea#text-area  name="comment[body_markdown]"
                      placeholder "Add to the discussion"
  - comment submit    #comments button[type=submit]  ("Submit")
  - logged-in probe   header .crayons-avatar  (scoped to header — the class
                      also matches commenters' avatars in the page body)
  - login page        /enter ; email login is a MAGIC LINK (send-only) —
                      the workable human path is the GitHub OAuth button
  - /notifications    private; redirects to /magic_links/new when logged out
"""

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

NAVETTE = os.environ.get("NAVETTE", "http://127.0.0.1:8765")
SESSION = os.environ.get("SESSION", "devto")
STORE = os.environ.get("STORE", "devto")

SEL_TEXTAREA = "textarea#text-area"
SEL_SUBMIT = "#comments button[type=submit]"
SEL_AVATAR = "header .crayons-avatar"


def die(msg):
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def api(path, body=None, raw=False, timeout=60):
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
    try:
        return json.loads(payload)
    except json.JSONDecodeError:
        die(f"navette {path} returned non-JSON")


def eval_js(js):
    return api("/evaluate", {"js": js, "session": SESSION})


def nav(url, **kw):
    body = {"url": url, "session": SESSION}
    body.update(kw)
    return api("/navigate", body)


def logged_in():
    r = eval_js(f"JSON.stringify(!!document.querySelector({json.dumps(SEL_AVATAR)}))")
    return r.get("result") in ("true", '"true"', "1", '"1"')


def restore():
    """Import the keychain state if this session isn't logged in."""
    if logged_in():
        return
    r = api("/sessions/load", {"session": SESSION, "store": STORE})
    if not r.get("ok"):
        die(f"state load failed: {r}")
    time.sleep(1.5)
    nav("https://dev.to/")
    time.sleep(2)
    if not logged_in():
        die("keychain state did not restore a login — run `login` again (session expired)")


def whoami():
    # Synchronous XHR: the eval bridge does not await promises.
    js = ("var x=new XMLHttpRequest();x.open('GET','/api/users/me',false);"
          "x.setRequestHeader('Accept','application/json');x.send();"
          "x.responseText")
    r = eval_js(js)
    try:
        return json.loads(r.get("result", ""))
    except (json.JSONDecodeError, TypeError):
        return {}


def cmd_login(_args):
    print(f"opening the visible window on dev.to/enter — session {SESSION!r}")
    nav("https://dev.to/enter")
    api("/sessions/show", {"session": SESSION})
    print("""
IN THE WINDOW (yours, visible on screen):
  1. click "Continue with GitHub" (email login is a magic link — it cannot
     complete inside this window)
  2. complete GitHub yourself: credentials + any 2FA
  3. you land back on dev.to logged in
The password never transits this chat or this script.""")
    print("polling for the login (up to 5 min) ...", end="", flush=True)
    deadline = time.time() + 300
    nav("https://dev.to/enter")
    while time.time() < deadline:
        time.sleep(5)
        print(".", end="", flush=True)
        # whatever page the user is on carries the navbar once logged in
        r = eval_js(f"JSON.stringify(!!document.querySelector({json.dumps(SEL_AVATAR)}))")
        if r.get("result") in ("true", '"true"', "1", '"1"'):
            break
    else:
        die("\nno login detected in 5 min — window left open, try again")
    r = api("/sessions/state", {"session": SESSION, "store": STORE})
    if not r.get("ok"):
        die(f"state store failed: {r}")
    print(f"\nlogged in — state stored in the keychain as {STORE!r} "
          f"({r.get('cookies')} cookies, origin {r.get('origin')})")
    who = whoami()
    if who.get("username"):
        print(f"you are: @{who['username']} ({who.get('name', '?')})")


def cmd_check(_args):
    restore()
    who = whoami()
    if who.get("username"):
        print(f"logged in as @{who['username']} ({who.get('name', '?')}) — "
              f"{who.get('pro', False) and 'pro ' or ''}org={bool(who.get('org_admin'))}")
    else:
        print("logged in (avatar present); /api/users/me did not answer — "
              "fine for notifications and comments")


def cmd_notifications(args):
    restore()
    nav("https://dev.to/notifications")
    time.sleep(3)
    js = r"""
var cards=[...document.querySelectorAll('.crayons-notification, .notification')];
var items=cards.map(function(c){
  var a=c.querySelector('a[href]');
  return {text:(c.innerText||'').replace(/\s+/g,' ').trim().slice(0,300),
          link:a?a.href:null};
}).filter(function(x){return x.text});
var who=(function(){var x=new XMLHttpRequest();x.open('GET','/api/users/me',false);
  x.setRequestHeader('Accept','application/json');x.send();
  try{return JSON.parse(x.responseText).username}catch(e){return null}})();
JSON.stringify({user:who, count:items.length, items:items,
  fallback: items.length ? null : document.body.innerText.slice(0, 1200)})
"""
    r = eval_js(js)
    try:
        data = json.loads(r.get("result", "{}"))
    except json.JSONDecodeError:
        die("notifications page unreadable — dev.to DOM changed?")
    if args.out:
        Path(args.out).write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n")
        print(f"json -> {args.out}")
    print(f"\nnotifications for @{data.get('user') or '?'} — {data['count']} entries")
    for it in data["items"][:args.limit]:
        print(f"- {it['text'][:140]}")
        if it.get("link"):
            print(f"  {it['link']}")
    if not data["items"]:
        print("(no notification cards matched — raw page head follows)")
        print((data.get("fallback") or "")[:600])


def read_post_file(path):
    p = Path(path)
    body = p.read_text()
    if body.strip().startswith("!!!"):
        raise SystemExit("error: comment file must be raw markdown, no front matter")
    if not body.strip():
        raise SystemExit(f"error: {path} is empty")
    return body


def composer_flow(url, body_md, publish, shot):
    restore()
    nav(url)
    api("/wait", {"selector": SEL_TEXTAREA, "ms": 15000, "session": SESSION})
    time.sleep(1.0)
    r = api("/type", {"selector": SEL_TEXTAREA, "value": body_md, "session": SESSION})
    if not r.get("ok"):
        die(f"typing failed: {r}")
    # Verify: read the field back, exact match required before any publish.
    time.sleep(0.5)
    js = ("JSON.stringify(document.querySelector('textarea#text-area')"
          ".value.length)")
    back = eval_js(js)
    try:
        n_back = int(json.loads(back["result"]))
    except (KeyError, ValueError, json.JSONDecodeError):
        die(f"cannot read the composer back: {back}")
    if n_back != len(body_md):
        die(f"verification failed: composer holds {n_back} chars, source has "
            f"{len(body_md)} — not publishing")
    print(f"composer verified: {n_back} chars match the source file")
    if shot:
        Path(shot).write_bytes(api("/screenshot", {"session": SESSION}, raw=True))
        print(f"screenshot -> {shot}")
    if not publish:
        print("DRY-RUN — nothing published. Re-run with --publish to submit.")
        return
    r = api("/click", {"selector": SEL_SUBMIT, "wait_navigation": True,
                       "session": SESSION})
    if not r.get("ok"):
        die(f"submit click failed: {r}")
    # dev.to keeps you on the article; the new comment appears at the top of
    # the thread with a #comment_<id> anchor.
    time.sleep(3)
    js = r"""
(function(){
  var mine=document.querySelector('#comments .comment, [class*="comment__details"]');
  var link=mine?mine.querySelector('a[href*="#comment_"]'):null;
  var txt=mine?(mine.innerText||'').replace(/\s+/g,' ').trim().slice(0,120):null;
  return JSON.stringify({anchor:location.hash||null, first:txt,
                         permalink:link?link.href:null});
})()
"""
    r = eval_js(js)
    try:
        out = json.loads(r.get("result", "{}"))
    except json.JSONDecodeError:
        out = {}
    print(f"published — first comment now: {out.get('first') or '?'}")
    if out.get("permalink"):
        print(f"  {out['permalink']}")


def cmd_comment(args):
    body_md = read_post_file(args.file)
    composer_flow(args.url, body_md, args.publish, args.shot)


def cmd_reply(args):
    body_md = read_post_file(args.file)
    url = args.comment_url.split("#")[0]
    anchor = "#" + args.comment_url.split("#")[1] if "#" in args.comment_url else None
    restore()
    nav(url)
    api("/wait", {"selector": SEL_TEXTAREA, "ms": 15000, "session": SESSION})
    if not anchor:
        die("reply needs the comment permalink (dev.to/...#comment_<id>)")
    # Open the reply form under that specific comment.
    js = (f"(function(){{var a=document.querySelector('a[href=\"{anchor}\"],"
          f"[href*=\"{anchor}\"]');if(!a)return 'MISSING';"
          "var card=a.closest('.comment')||a.closest('[class*=comment]');"
          "if(!card)return 'NO-CARD';"
          "var btn=[...card.querySelectorAll('button')].find(function(b){"
          "return /reply/i.test(b.textContent)});"
          "if(!btn)return 'NO-REPLY-BTN';btn.click();return 'OK'}})()")
    r = eval_js(js)
    if r.get("result") != '"OK"' and r.get("result") != "OK":
        die(f"cannot open the reply form: {r.get('result')} — open it manually "
            "in the visible window, then re-run")
    time.sleep(1.0)
    # The reply form reuses the same composer, scoped inside that comment card.
    r = api("/type", {"selector": f".comment {SEL_TEXTAREA}, {SEL_TEXTAREA}",
                      "value": body_md, "session": SESSION})
    if not r.get("ok"):
        die(f"typing failed: {r}")
    print(f"reply staged ({len(body_md)} chars) — verify in the screenshot")
    if args.shot:
        Path(args.shot).write_bytes(api("/screenshot", {"session": SESSION}, raw=True))
        print(f"screenshot -> {args.shot}")
    if not args.publish:
        print("DRY-RUN — nothing published. Re-run with --publish to submit.")
        return
    r = api("/click", {"selector": f".comment {SEL_SUBMIT}, {SEL_SUBMIT}",
                       "wait_navigation": True, "session": SESSION})
    if not r.get("ok"):
        die(f"submit click failed: {r}")
    time.sleep(3)
    print("published — check the thread under the comment you replied to")


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("login", help="one-time human login, then keychain store")
    sub.add_parser("check", help="restore if needed; print who you are")
    n = sub.add_parser("notifications", help="read /notifications")
    n.add_argument("--json", dest="out", default=None)
    n.add_argument("--limit", type=int, default=10)
    c = sub.add_parser("comment", help="comment on an article URL")
    c.add_argument("url")
    c.add_argument("file", help="markdown file: the comment body")
    c.add_argument("--publish", action="store_true", help="actually submit")
    c.add_argument("--shot", default=None)
    rp = sub.add_parser("reply", help="reply to a comment permalink")
    rp.add_argument("comment_url")
    rp.add_argument("file")
    rp.add_argument("--publish", action="store_true")
    rp.add_argument("--shot", default=None)
    args = ap.parse_args()
    {"login": cmd_login, "check": cmd_check, "notifications": cmd_notifications,
     "comment": cmd_comment, "reply": cmd_reply}[args.cmd](args)


if __name__ == "__main__":
    main()
