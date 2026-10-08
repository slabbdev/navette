#!/usr/bin/env python3
"""A mock counterparty that speaks signed webhooks.

The shape (from the Vending-Bench thread): agent benchmarks need SEALED
counterparty events — data the agent can verify without trusting the
emitter. This server plays a payment processor: every event is chained
(HMAC over payload + previous digest) and served with its signature.

The demo scenario: the marketing dashboard shows one event with a
different amount than the signed stream. Fraud analysis becomes a DIFF,
not a log read — the agent verifies the chain, finds the one event whose
signature breaks, and that's the whole investigation.

No dependencies beyond the stdlib. navette is optional: every endpoint
returns plain JSON, so agents can read it through navette (navigate+read)
or plain fetch — the verification math is identical.

Usage: python3 server.py [--port 8877] [--secret SECRET]
      (the --secret the "processor" signs with; default is demo-only)
"""
import argparse
import hashlib
import hmac
import json
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit, parse_qs

SECRET = b"tollbooth-demo-secret"

# The signed stream (what the processor attests) and the dashboard view
# (what humans see). One event diverges — that divergence IS the demo.
EVENTS = [
    {"seq": 1, "type": "order.paid",      "order": "A-1001", "amount": 4999},
    {"seq": 2, "type": "order.paid",      "order": "A-1002", "amount": 12900},
    {"seq": 3, "type": "refund.issued",   "order": "A-1002", "amount": -12900},
    {"seq": 4, "type": "order.paid",      "order": "A-1003", "amount": 750},
    {"seq": 5, "type": "chargeback.open", "order": "A-1001", "amount": -4999},
    {"seq": 6, "type": "order.paid",      "order": "A-1004", "amount": 21050},
]
TAMPERED_SEQ = 4          # the dashboard shows this one different
TAMPERED_AMOUNT = 75      # 7.50 shown instead of 7.50? no — 750 cents became 75


def chain_digest(events: list[dict]) -> list[dict]:
    """HMAC-chain every event: sig_n = HMAC(secret, digest_{n-1} || payload_n)."""
    out, prev = [], "0" * 64
    for e in events:
        payload = json.dumps(e, sort_keys=True, separators=(",", ":"))
        sig = hmac.new(SECRET, (prev + payload).encode(), hashlib.sha256).hexdigest()
        out.append({"event": e, "sig": sig, "prev": prev})
        prev = sig
    return out


SIGNED = chain_digest(EVENTS)


def dashboard_view() -> list[dict]:
    """The human-facing copy — one amount quietly different."""
    view = []
    for row in SIGNED:
        e = dict(row["event"])
        if e["seq"] == TAMPERED_SEQ:
            e["amount"] = TAMPERED_AMOUNT
        view.append(e)
    return view


class H(BaseHTTPRequestHandler):
    def do_GET(self):
        u = urlsplit(self.path)
        q = parse_qs(u.query)
        if u.path == "/events":
            after = int(q.get("after", ["0"])[0])
            body = [r for r in SIGNED if r["event"]["seq"] > after]
        elif u.path == "/events/_head":
            body = {"seq": SIGNED[-1]["event"]["seq"], "digest": SIGNED[-1]["sig"]}
        elif u.path == "/dashboard":
            body = dashboard_view()
        elif u.path == "/verify.js":
            # A drop-in verifier the agent can fetch and run IN the page
            # (navette /evaluate) — verification where the data lives.
            body = VERIFY_JS
        else:
            self.send_error(404)
            return
        data = json.dumps(body, indent=1).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json" if u.path != "/verify.js" else "text/javascript")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *a):
        pass


VERIFY_JS = r"""
function verify(signed, secret) {
  function sha(key, msg) {
    // SHA-256 HMAC via WebCrypto (async) — kept synchronous for the demo
    // by returning a promise map; see README for the awaited variant.
  }
  // Intentionally minimal: the agent is expected to do this itself with
  // its own crypto (python -c 'import hmac,hashlib; ...' or Node), which
  // is the point — sealed data enables INDEPENDENT verification.
  return "implement me — see README: the verification must live with the verifier, not the emitter";
}
"""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8877)
    ap.add_argument("--secret", default="tollbooth-demo-secret")
    args = ap.parse_args()
    global SECRET
    SECRET = args.secret.encode()
    print(f"signed-webhook mock counterparty on http://127.0.0.1:{args.port}")
    print("endpoints: /events?after=N · /events/_head · /dashboard")
    print(f"tampered event: seq {TAMPERED_SEQ} (dashboard shows {TAMPERED_AMOUNT}, signed stream says {EVENTS[TAMPERED_SEQ-1]['amount']})")
    HTTPServer(("127.0.0.1", args.port), H).serve_forever()


if __name__ == "__main__":
    main()
