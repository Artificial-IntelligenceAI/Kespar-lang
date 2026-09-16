# The generator — writing programs nobody would write

Compiler-development tooling in `generator/`. Owner's direction,
2026-09-16: one worker per core, shared counter, in-process, seeded replay,
shrinker, generated edge-case inputs — Xag's generator design, taken
further.

## What it writes

Random **valid** Kespar programs (`design/language.md`), the way Csmith
and YARPGen do for C: every program type-checks, every `:=` is boundable,
every function is called with the right arity, no `nocheck` site can fail,
and every loop terminates — `loop.for` over a bounded range or a list, and
`loop.while` only with a counter the generator knows decrements. It writes
every construct in scope: all integer widths, bins, bools, strs as pieces,
lists (literal, `std::fill`, read), indexing on both sides of `=`, all
operators including the parenthesis-required shapes, `else.if` chains,
nested loops with `break`/`continue`, recursive and non-recursive
functions with typed and untyped parameters and returns, `check` and
`nocheck` blocks, `$:=` shadowing, `std::exit`, `std::to`, `std::len`,
and reads with and without bounds.

A **case** is one program plus a set of **input line-sets** for it: several
random line-sets within the declared bounds, every edge line-set (bound
ends, 0, ±1, type min/max, empty and maximum-length lists and strs), and
— when the scalar read domains multiply to at most 4096 (**provisional**,
`--exhaust N` raises it) — the whole domain, enumerated.

Programs are grown to a **size** knob (statements, nesting depth, function
count) so a run can start small and widen. Everything comes from one
64-bit seed through a splittable PRNG, so **seed → case** is a pure
function and any finding replays from its number alone.

## How it runs

- **Workers**: one thread per core (`available_parallelism`), each pulling
  the next seed from a shared atomic counter, so a slow case never holds
  up the others.
- **In-process**: the generator links the reference interpreter and the
  compiler as libraries. A case does not spawn a process for either; the
  only process a case ever starts is none (Z3 is a library too). A case
  costs microseconds to milliseconds, so a million cases is a coffee.
- **Per-worker scratch**: each worker owns a directory under
  `generator/work/<n>/` for the rare thing that must touch disk (a
  finding's files); nothing is shared.
- **Budgets, not clocks**: every engine runs with the same `--steps`
  budget; a case that reaches it in every engine is "budget", not a
  finding.
- **Output**: a running count of cases, findings and rate; on a finding,
  the seed, the program, the inputs, every engine's answer, which engine
  is the odd one out, and for a claim failure the site and the input that
  broke the claim. `--keep-going` continues past findings; default stops
  at the first.

## Shrinking

A finding's program is reduced before it is shown: remove a statement,
simplify an expression to a literal, drop a function and inline the
constant, narrow a read's bound, shorten a list — each step re-run through
the same engines, kept only if the finding survives, until no step
survives. Deterministic, so the same seed shrinks to the same program.

## What it must never do

- Write an invalid program and count its rejection as agreement — a
  generated program the compiler or the oracle **rejects** is a finding
  (`broke`), kept whole.
- Decide what the right answer is. It reports disagreement and which
  engine stands apart; a human reads the program.
- Depend on the time, the machine, or the order workers finished.

## Commands

```
generator run   --cases 100000 --seed 1 --size 3 [--jobs N] [--keep-going]
generator one   --seed 12345          # print the program and inputs for a seed
generator replay --seed 12345         # run one case through every engine, verbose
```
