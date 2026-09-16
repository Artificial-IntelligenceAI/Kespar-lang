# Precog in Kespar

What the compiler does between parsing and bytecode, to meet the rules in
`language.md` §8. Owner's direction, 2026-09-16: "Precog" — the whole thing
as CyborgPL defines it, not a cut-down version. CyborgPL's definition (its
`DECISIONS.md`, `design/core-features.md`, `design/reproducible-builds.md`)
is the authority; this file is how Kespar meets it with a bytecode VM
behind it.

## The pipeline

```
.kpls ──parse──▶ AST ──types──▶ typed AST ──Precog──▶ annotated AST ──emit──▶ bytecode ──▶ VM
                                              │
                                              ▼
                                        proof report
```

`kespar check file.kpls` runs everything up to the report. `kespar build`
also writes `file.kpbc`. `kespar run` builds and runs.

## Layer 1 — known values are executed

Everything that does not depend on a `std::read.stdin` is **known**. Precog
executes it at compile time with the same semantics the VM has (exact
integer arithmetic checked against the width, IEEE bins at the right
width) and folds the result in. A program with no read is executed to
the end; its bytecode is the `print`s of its answers, and if a check would
have failed at run time, that is a **compile error** reported at the line:
the program would always fail there, so it never ships. Code that only ever
runs on known values does not exist in the bytecode.

The step budget (`language.md` §8.5) is counted here: one step per AST node
evaluated. The count is the only limit; there is no clock anywhere in the
compiler (owner, 2026-09-16).

## Layer 2 — bounded values are tracked as sets

Every `std::read.stdin` declares a type and possibly bounds. That set is
the value's **domain**. Precog runs the program abstractly: each expression
gets the set of values it can take, computed exactly through every
operation (`{1,2,3} x {10}` is `{10,20,30}`). A set larger than **4096
values** (**provisional**) becomes an **interval** `[lo, hi]` — still
sound, less precise. Bins are tracked as intervals from the start (or "any
bin" for an unbounded bin read). Strs carry a length interval; lists a
length interval and an element domain.

Control flow: at an `if`, the condition's set decides which branches are
possible and each branch is analysed with the condition assumed (an `if
'n' > 5` narrows `'n'` inside). Loops iterate the analysis to a fixpoint,
widening to intervals when a set keeps growing; a `loop` over a bounded
range unrolls abstractly at most **64 times** (**provisional**) before
widening. Functions are analysed per call site with the arguments' sets
(inlined analysis, depth-limited at **8** for recursion, **provisional**),
so `square['i']` with `'i'` in 1..1000 proves its own multiply.

At each check site Precog asks: is every value in the result set legal? If
yes, the site is **proven** and gets no check. If the set says no value is
legal, it is a **certain failure** and a compile error. Otherwise the site
is **unproven** and goes to layer 3.

## Layer 3 — the exact question

An unproven site is asked exactly, as CyborgPL specifies: a **bit-precise
SMT query** over the real program semantics. Kespar uses **Z3** through
the `z3` crate (owner allowed dependencies, 2026-09-16). The query encodes
the path to the site — the read domains, every assignment on the way,
the branch conditions taken, loops unrolled to a bound of **32 iterations**
(**provisional**) — as bit-vectors of the exact widths, and asks whether
any input reaches the site with an illegal value.

- `unsat` → **proven**. No check.
- `sat` → a **counterexample**: concrete input values that make the site
  fail. In the default tier the site keeps its check; in a `check` block it
  is a compile error that prints the counterexample so the programmer can
  run it.
- unknown, or the solver's step budget exhausted → **unproven**; treated as
  `sat` without a counterexample. The budget is Z3's resource limit
  (`rlimit`, a step count, never `timeout`), **provisional** 10,000,000
  per query, so the answer is the same on every machine.

Loop unrolling to a bound means a site inside a loop may be proven for the
first 32 iterations and unproven after; Precog reports it as unproven
unless layer 2's fixpoint already covered every iteration. This is where
CyborgPL's invariant templates would go (sums, counters, indices proven
inductively); the POC does not implement them and says so in the report.

## Tiers

Per `language.md` §8.4. In the emitted bytecode:

- **proven** site → plain arithmetic instruction.
- **default, unproven** → checked instruction (the VM traps with exit 1).
- **check, unproven** → compile error with counterexample or "solver gave
  up at line N; add a bound at the read on line M".
- **nocheck** → plain instruction, no proof attempted, listed in the
  report.

The read contract check is always emitted and is not a site.

## `:=` widths

Computed from layer 2's sets as `language.md` §8.3 describes: the union of
the sets of every value the free name holds and of every expression it
types; the narrowest width holding the union. If layer 2 only has an
interval that touches the 64-bit limit, or no bound at all, layer 3 is
asked for the maximum (bounded binary search over "can it exceed K?"), and
failing that it is the compile error in §8.3.

## Determinism

Owner, 2026-09-16, and CyborgPL's reproducible-builds rules: same source →
same bytecode and same report, on every machine.

- No wall-clock anywhere; every limit above is a count.
- Z3 is called with a fixed configuration and `rlimit`; results do not
  depend on thread scheduling.
- Sites are analysed in source order; the report lists them in source
  order.
- Compile-time arithmetic is performed at the exact width the VM uses.
- Nothing is read at compile time except the source: no files, clock, env.
- The compiler and solver versions are printed in the report header.

## Parallelism

CyborgPL's Precog auto-parallelises independent loops. Kespar's VM does
not (a bytecode interpreter has nothing to gain), and the POC does not
prove loop independence — there is no ownership model to prove it with
(GC, owner's decision). What Kespar *does* parallelise is its own work:
the layer-3 queries for independent sites run across all cores, and the
oracle's runs do too. Recorded as out of scope, not as disagreement.

## The report

`kespar check --report` prints, per check site, in source order:

```
line 7   'total' + square['i']   overflow           proven   (layer 2: max 333833500)
line 12  'xs'['k']               out of bounds      checked  (layer 3: sat, k=100 when len=100)
line 15  'a' / 'b'               division by zero   trusted  (nocheck)
line 9   square: 'x' x 'x'       overflow           proven   (layer 2, at 1 call site)
```

and, with `--types`, every `:=` name with its chosen width and the value
that decided it. The oracle reads this report to know which claims to
attack (`design/oracle.md`).
