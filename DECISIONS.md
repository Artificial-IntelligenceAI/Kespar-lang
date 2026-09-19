# Decisions

The running log of what the owner (Tankun Sriket) decided for Kespar,
against what was proposed. Design authority is the owner's alone; Claude
proposes and records. Anything not here is not decided — `design/*.md`
marks such picks **provisional**.

### 2026-09-16 — What Kespar is (owner)

A small proof-of-concept of Cyborg's Precog, by AI. Not Cyborg/CyborgPL.
Precog is Xag's ITMT successor. Kespar must have a test oracle and a
program generator that uses the hardware effectively.

### 2026-09-16 — Rust, dependencies allowed (owner)

Claude proposed (a) Rust with zero dependencies, (b) Rust with a few
crates, (c) C++. Owner: "No reviewers yet. Rust with any." Kept few
anyway: `rayon`, `clap`, `z3`.

### 2026-09-16 — Syntax is CyborgPL's; files are `.kpls` (owner)

First answer: custom syntax, must carry Cyborg's `:=`, extension `.kpls`
(checked unused). Revised the same day: "just copy Cyborg's syntax, don't
wanna design another lang for a quick POC." So Kespar's syntax is
CyborgPL v2's as decided in its repo, minus what the scope cuts, with
CyborgPL's open questions given provisional picks in `design/language.md`
(comments `//`, `std::read.stdin[...]` for input, `list.T`, 0-based
`'xs'[i]`, `std::len`, `std::fill`, `std::range[a, b]` inclusive,
`std::to.T` conversion, `check { }` / `nocheck { }` blocks, `/` truncates,
`mod` takes the dividend's sign, no implicit space between pieces).

### 2026-09-16 — Scope (owner)

"int, uint, bin, bool, str, if/else, func, print, input, arr, and GC",
then "loops, yes". Widths left to Claude: int/uint 8–64, bin 32/64,
`str.utf8` only, lists fixed-length. **GC** replaces ownership: no
`give`/`copy`/`lend`, and the POC proves nothing about borrow validity.

### 2026-09-16 — Bytecode, interpreted (owner)

"Compile to bytecode, then interpreted. So, yes, shit performance
already." No native backend, no LLVM. Precog's performance win is shown
relatively: the same VM with every check versus after Precog.

### 2026-09-16 — Precog in full (owner)

Claude proposed layers (known values executed; bounded values as exact
sets; an exact step by brute force, SMT, or skipped) and tiers. Owner:
"Precog" — meaning the whole thing as CyborgPL defines it: exact sets, a
real SMT solver (Z3), and all three tiers.

### 2026-09-16 — Step budgets, never wall-clock (owner)

Every compile-time limit is a count. Same as CyborgPL's
reproducible-builds rule. Budget sizes are Claude's provisional picks.

### 2026-09-16 — Oracle: three engines plus claim verification (owner)

Reference interpreter, Precog→bytecode→VM, and Precog's compile-time
answer, compared on stdout, exit code and where a check fired; and every
proven-away check attacked on random, edge and (when small) exhaustive
inputs. See `design/oracle.md`.

### 2026-09-16 — Generator (owner)

One worker per core, shared atomic counter, in-process engines, seeded
replay, shrinker, generated edge-case inputs. See `design/generator.md`.

### 2026-09-16 — The reference interpreter is written by a separate Claude subagent (owner)

From `design/language.md` alone, no shared code with `kespar/`. Owner
asked "A Claude subagent?" to Claude's (a) same author / (b) different
model; chosen as the middle ground.

### 2026-09-16 — Layout (owner: "whatever")

`design/` + `DECISIONS.md`, `kespar/`, `oracle/`, `generator/`,
`examples/`, `bench/`.

## Open

- Every **provisional** pick in `design/language.md` and
  `design/precog.md` (budget sizes, set-size cap, unroll depths, the
  provisional syntax listed above).
- Whether the build output should list every `nocheck` site (CyborgPL
  open; Kespar's report does).

### 2026-09-19 — Kespar's syntax is frozen at CyborgPL's 2026-09-16 sketch (owner)

CyborgPL redesigned its syntax around 2026-09-18 (`let`, `fn`, `struct`,
types after the name, bare conditions, `else if`, `for temp 'i' in
range[a, b]`). Claude asked whether Kespar follows; owner: "no need to
follow. Kespar was only a Precog POC." Kespar keeps the syntax CyborgPL
had on 2026-09-16, as `design/language.md` describes it.
