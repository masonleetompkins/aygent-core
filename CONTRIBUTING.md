# Contributing to AYGENT Core

Thanks for building. Core is the open agent harness; fixes and hardenings here
flow into everything, including Pro. A few rules keep that pipeline clean.

## How to contribute
1. Fork, branch, open a pull request against `main`.
2. Keep it green: the matrix CI builds macOS / Windows / Linux, runs the Rust
   test suite, type-checks the UI, and enforces the gates below.
3. One concern per PR. Describe what you tested on real hardware.

## License
By submitting a pull request you agree your contribution is licensed under the
Apache License, Version 2.0 (see `LICENSE`), with no additional terms. The
maintainer may ship your contribution in open and commercial builds, including
AYGENT Pro — that's the point: Core improvements harden Pro for everyone.

## The Core-clean rule (hard gate, enforced by CI)
Core must never contain proprietary surfaces. A PR fails CI if it introduces,
anywhere in the tree:
- subscription / credit / wallet / tier-lock billing code or strings
- sign-in, session-token, or entitlement-check code or strings
- `Dashboard` / `Sparks` / `Video` panels or their routes
- private backend URLs, API keys, tokens, or personal data of any kind

Concretely the `core-clean` CI job greps for the banned marker list (see
`.github/workflows/matrix.yml`). Genuine English words (`subscribe` as in an
event subscription, `managed` as in OS-managed) are fine — the list targets
exact proprietary identifiers. If the gate flags your diff, rename to neutral
terms or ask in the PR.

## What belongs where
- Core: agent loop, providers (BYOK + local), jail/broker, tools, memory,
  chat UI, platforms, tests, docs that ship.
- NOT Core (maintainer-only, lives outside this repo): billing, hosted
  relays, account/sign-in systems, Pro panels. Don't re-add them here —
  the gate will catch it and the PR will be closed.

## Security
Found a secret, private data, or a safety hole in the tree? Don't open a
public PR — email the maintainer (see README) with details and give time
to fix before disclosing.
