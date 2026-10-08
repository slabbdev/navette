# Signed-webhook counterparty mode — fraud analysis as a diff

From the Vending-Bench thread (reidmarlow's HMAC-webhook point): agent benchmarks need **sealed counterparty events** — data the agent can verify without trusting the emitter. This demo plays both sides: a mock payment processor that HMAC-chains every event, and the investigation an agent runs when the human dashboard and the signed stream disagree.

The payoff shape: **fraud analysis becomes a diff, not a log read.** No heuristics over logs — the agent verifies signatures, and the one event that doesn't verify IS the finding.

## Run it

```sh
python3 demo/signed-webhook/server.py          # mock counterparty on :8877
```

The server plants one divergence: the dashboard shows event `seq 4` with amount `75`; the signed stream says `750`. Nobody tells the agent which event is bad — that's the exercise.

## The endpoints

- `GET /events?after=N` — the signed stream from sequence N+1: `{event, sig, prev}` per row
- `GET /events/_head` — the current head digest (chain tip)
- `GET /dashboard` — the human-facing copy (one amount differs)

## The recipe (agent side)

Point navette at it — or plain fetch; the math is identical:

```sh
NAVETTE=http://127.0.0.1:8766
curl -s -X POST $NAVETTE/navigate -H 'Content-Type: application/json' \
  -d '{"session":"audit","url":"http://127.0.0.1:8877/events"}' >/dev/null
curl -s -X POST $NAVETTE/read -H 'Content-Type: application/json' -d '{"session":"audit"}'
# same for /dashboard
```

Then verify the chain independently — the verifier's crypto lives with the **verifier**, never with the emitter:

```sh
python3 - <<'EOF'
import hmac, hashlib, json, urllib.request
SECRET = b"tollbooth-demo-secret"
signed = json.load(urllib.request.urlopen("http://127.0.0.1:8877/events"))
prev = "0" * 64
for row in signed:
    payload = json.dumps(row["event"], sort_keys=True, separators=(",", ":"))
    sig = hmac.new(SECRET, (prev + payload).encode(), hashlib.sha256).hexdigest()
    if sig != row["sig"]:
        print(f"TAMPERED: seq {row['event']['seq']} — signature mismatch")
    prev = sig
print("chain otherwise intact")
EOF
```

The chain itself verifies end-to-end — because the emitter's stream is internally consistent. The fraud is the **diff against the dashboard**: `seq 4`, dashboard `75` vs signed `750`. One API call, one HMAC loop, zero log heuristics.

## Why this matters for benchmarks

An agent scored on this task is scored on **verification**, not vibes: it cannot pass by summarizing the dashboard or pattern-matching "suspicious" amounts — only by actually recomining signatures. That's the class of sealed-counterparty tasks Vending-Bench wanted, produced for free by a 150-line mock.

Park: this stays a demo — navette remains navigate/read/screenshot/act; the mock counterparty is a layer above.
