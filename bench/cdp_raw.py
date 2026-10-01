#!/usr/bin/env python3
"""cdp_raw — measure browsers through a MINIMAL raw CDP client.

Why this exists: the main harness drives Lightpanda through Playwright's
connectOverCDP, which adds client overhead. This script drives ANY CDP
browser (Lightpanda, Chromium headless shell) through the same thin
websocket client, so engine-vs-engine numbers carry zero client bias.

Usage: python3 cdp_raw.py <lightpanda|chromium> <port> <url1> [url2 ...]
Prints one JSON line: {"ok_rate":…, "median_ms":…, "times":[…], "failed":[…]}
"""

import json
import statistics
import sys
import time
import urllib.parse
import urllib.request

import websocket


def cdp_call(ws, msg_id, method, params=None, session=None, timeout=30):
    payload = {"id": msg_id, "method": method}
    if params is not None:
        payload["params"] = params
    if session:
        payload["sessionId"] = session
    ws.send(json.dumps(payload))
    deadline = time.time() + timeout
    while time.time() < deadline:
        raw = ws.recv()
        if isinstance(raw, bytes):
            raw = raw.decode("utf-8", "replace")
        msg = json.loads(raw)
        if msg.get("id") == msg_id:
            if "error" in msg:
                raise RuntimeError(f"{method}: {msg['error']}")
            return msg.get("result", {})
    raise RuntimeError(f"{method}: timeout")


def cdp_wait_event(ws, method, session, timeout=30):
    deadline = time.time() + timeout
    while time.time() < deadline:
        raw = ws.recv()
        if isinstance(raw, bytes):
            raw = raw.decode("utf-8", "replace")
        msg = json.loads(raw)
        if msg.get("method") == method and msg.get("sessionId") == session:
            return msg
    raise RuntimeError(f"event {method}: timeout")


def main():
    kind, port = sys.argv[1], sys.argv[2]
    urls = sys.argv[3:]
    if not urls:
        print(json.dumps({"error": "no urls"}))
        return

    if kind == "navette":
        # navette speaks its own loopback HTTP API — same measurement shape.
        times, failed = [], []
        for url in urls:
            t0 = time.time()
            try:
                req = urllib.request.Request(
                    f"http://127.0.0.1:{port}/navigate",
                    data=json.dumps({"url": url, "with_content": True, "session": "real"}).encode(),
                    headers={"Content-Type": "application/json"})
                r = json.loads(urllib.request.urlopen(req, timeout=30).read())
                if not r.get("ok") or len(r.get("content", "")) < 1:
                    raise RuntimeError(str(r.get("error", "bad response"))[:120])
                times.append((time.time() - t0) * 1000)
            except Exception as e:
                failed.append({"url": url, "error": str(e)[:120]})
        print(json.dumps({
            "kind": "navette",
            "ok_rate": round(len(times) / len(urls), 3),
            "median_ms": round(statistics.median(times), 1) if times else None,
            "times": [round(t, 1) for t in times],
            "failed": failed[:3],
        }))
        return

    ver = json.loads(urllib.request.urlopen(f"http://127.0.0.1:{port}/json/version", timeout=5).read())
    ws = websocket.create_connection(ver["webSocketDebuggerUrl"], timeout=30, suppress_origin=True)
    mid = 0

    def call(method, params=None, session=None, timeout=30):
        nonlocal mid
        mid += 1
        return cdp_call(ws, mid, method, params, session, timeout)

    # browser-level: one page target, reused for every navigation (session reuse,
    # same semantics as navette's sessions and Playwright's page).
    target = call("Target.createTarget", {"url": "about:blank"})["targetId"]
    session = call("Target.attachToTarget", {"targetId": target, "flatten": True})["sessionId"]
    call("Page.enable", session=session)

    times, failed = [], []
    for url in urls:
        t0 = time.time()
        try:
            call("Page.navigate", {"url": url}, session=session, timeout=25)
            # readiness poll (identical client cost for every engine): wait for
            # the TARGET url to be complete — immune to stale loadEventFired.
            target_n = url.rstrip("/")
            deadline = time.time() + 25
            while True:
                res = call(
                    "Runtime.evaluate",
                    {"expression": "JSON.stringify({u:location.href,r:document.readyState})",
                     "returnByValue": True},
                    session=session,
                    timeout=25,
                )
                raw_v = res.get("result", {}).get("value", "{}")
                st = json.loads(raw_v) if raw_v else {}
                if st.get("r") == "complete" and st.get("u", "").rstrip("/") == target_n:
                    break
                if time.time() > deadline:
                    raise RuntimeError(f"load timeout on {url}")
                time.sleep(0.002)
            res = call(
                "Runtime.evaluate",
                {"expression": "document.body.innerText", "returnByValue": True},
                session=session,
                timeout=25,
            )
            text = res.get("result", {}).get("value", "")
            if text is None or len(str(text)) < 1:
                raise RuntimeError(f"empty read, state={st}")
            times.append((time.time() - t0) * 1000)
        except Exception as e:
            failed.append({"url": url, "error": str(e)[:120]})

    ws.close()
    print(json.dumps({
        "kind": kind,
        "ok_rate": round(len(times) / len(urls), 3),
        "median_ms": round(statistics.median(times), 1) if times else None,
        "times": [round(t, 1) for t in times],
        "failed": failed[:3],
    }))


if __name__ == "__main__":
    main()
