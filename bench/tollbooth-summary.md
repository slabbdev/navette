# Tollbooth — the walled web, measured

> **A measurement, not a verdict.** One exit IP (residential), no challenge solving, no retries, no logins. A site that walls itself off from this probe has not been judged — it has been measured once, from one vantage point, on one day.
> **Dataset license: [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/)** — the bench code is MIT (repo root).

URLs measured: 200 · method: real WebKit via navette navigate+read, residential IP · payment probe: passive plain GET (recorded separately, never merged into the agent-path outcome)

| Category | URLs | ok | challenge | empty/JS-gate | timeout/error | AI-blocked-all | AI-partial | llms.txt | x402 | paywall |
|---|---|---|---|---|---|---|---|---|---|---|
| docs | 40 | 39 (97%) | 0 | 1 | 0 | 0 | 0 | 13 | 0 | 0 |
| news | 40 | 34 (85%) | 3 | 3 | 0 | 35 | 0 | 5 | 0 | 1 |
| e-commerce | 40 | 33 (82%) | 2 | 5 | 0 | 8 | 2 | 9 | 0 | 0 |
| dev-tools | 40 | 33 (82%) | 5 | 2 | 0 | 4 | 1 | 14 | 0 | 0 |
| social | 40 | 26 (65%) | 2 | 12 | 0 | 25 | 1 | 14 | 0 | 0 |
