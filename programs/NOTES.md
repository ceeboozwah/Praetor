# N24B — real-program hardening log

Semantics frozen. Metrics per program: lines, forced workarounds,
annotation count, compile time, bugs found, stdlib gaps, diagnostic
quality, authority-model friction.

## Pre-flight gaps (discovered before writing code)

- G1: no argc/argv — programs can't take CLI arguments. Paths must
  be literals or read from a file/stdin. Blocks "real CLI app".
- G2: no break/continue — loops are condition-only. Search loops
  need flag variables.
