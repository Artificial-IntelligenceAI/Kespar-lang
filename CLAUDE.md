# Kespar

A small proof-of-concept of **Cyborg's Precog** — the engine that runs the
whole program at compile time for safety and run-time performance. Kespar
is **not** Cyborg/CyborgPL; it borrows CyborgPL's syntax (owner's
decision) to avoid designing another language for a POC.

## Ground rules

- **Design authority is the owner's** (Tankun Sriket). Claude proposes with
  a recommendation and records; it never decides syntax, semantics or
  architecture, and never fills in a default silently — a pick made so the
  code can run is marked **provisional** in `design/` and listed under
  Open in `DECISIONS.md`.
- **Never use the AskUserQuestion tool.** Ask in plain text, one question
  at a time, and wait.
- Record every decision in `DECISIONS.md` with who decided.
- CyborgPL is the authority on Precog and on the copied syntax:
  `/Users/ts/CyborgPL v2` (`DECISIONS.md`, `design/`, `oracle/`).

## Layout

- `design/language.md` — the complete language spec. The oracle is written
  from this file alone; the compiler must agree with it.
- `design/precog.md`, `design/oracle.md`, `design/generator.md`.
- `kespar/` — the compiler and VM (Rust): `kespar check | build | run`.
- `oracle/` — the reference interpreter (Rust), written by a separate
  Claude agent from the spec; **shares no code with `kespar/`**.
- `generator/` — program generator + differential runner (Rust).
- `examples/` — `.kpls` programs with expected `.out` (and `.in`, `.exit`).
- `bench/` — the perf comparison, checks kept vs Precog-removed.

## Building

Rust via `~/.cargo/bin`. On this Mac the Xcode licence is not accepted, so
`/usr/bin/git`, `cc` and `python3` fail: put
`/Library/Developer/CommandLineTools/usr/bin` first on `PATH`.

## Git

Branch `main`, remote over HTTPS. Author (repo-local, already set):
`Tankun Sriket <tankunsriket63741ecff056.invalid>`. Every commit ends with
`Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`. Push after
committing.
