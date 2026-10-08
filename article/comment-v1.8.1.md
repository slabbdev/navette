**v1.8.1 is out** — cross-session cookie isolation is now a build-breaking CI assertion on all three engines (named WebView2 profiles + InPrivate on Windows, non-persistent stores on macOS, isolated contexts on Linux), plus loopback-API hardening: constant-time token checks, a DNS-rebinding guard, credential-redacted proxy logs. The full posture is now documented: [docs/SECURITY.md](https://github.com/slabbdev/navette/blob/main/docs/SECURITY.md).

Release notes: https://github.com/slabbdev/navette/releases/tag/v1.8.1

And since the last update here: I pointed the resident daemon at 200 real URLs and measured the walled web — 82.5% readable by a vanilla agent, 72 hosts blocking every tracked AI crawler, zero x402 signals yet, and one named site (The Intercept) running a declared paywall + full AI-bot blocks + an open front door at the same time. Dataset is CC BY 4.0: https://dev.to/slabb/my-agent-met-the-real-web-403s-challenges-and-the-coming-tollbooth-1h2g
