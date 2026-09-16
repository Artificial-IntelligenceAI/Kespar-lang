# Kespar-lang
Kespar is **not Cyborg/CyborgPL**. It's a small POC for Cyborg's Precog.
Cyborg's Precog is Xag's ITMT successor, it runs the entire program at comptime for safety, and making runtime perf better. 
"That's not possible." It is, but sometimes not fully, the design isn't even finished yet. And will probably be one of my projects that would take long.

"48.2 minutes, that's faster than usual. Did they update Precog?" -ChatGPT. So, yeah, devs are gonna suffer 😭.

## Layout

- `design/` — the spec: [`language.md`](design/language.md), [`precog.md`](design/precog.md), [`oracle.md`](design/oracle.md), [`generator.md`](design/generator.md).
- [`DECISIONS.md`](DECISIONS.md) — who decided what, and what is still provisional.
- `kespar/` — compiler + bytecode VM. `oracle/` — reference interpreter. `generator/` — program generator and differential runner. `examples/`, `bench/`.

Syntax is CyborgPL's (see `/design/language.md` for the cut-down and provisional parts). Files are `.kpls`.

## What it does

`kespar check file.kpls` runs **Precog** over the program before anything runs:

1. **A program that reads nothing is run to the end at compile time.** Its
   bytecode is its output; a check that would fail is a compile error.
2. **A program that reads is run over the sets of values its reads allow** —
   exact sets, then intervals; counted loops followed trip by trip. Every
   overflow, division-by-zero and index check whose property holds on every
   value is removed; free `:=` names get the narrowest width that holds
   everything they held.
3. **What is left is asked exactly**, as a bit-vector query to Z3 over the
   real path to the site: `unsat` removes the check, `sat` is a
   counterexample, which the `check { }` tier turns into a compile error.

```
$ kespar check --types examples/squares.kpls
Precog analysed the program over its input bounds (17011 steps).
line 3    'x' x 'x'                    overflow           proven   (layer 2: result within 1..1000000)
line 11   'total' + square[…]          overflow           proven   (layer 2: result within 1..333833500)
line 12   'i' mod 2                    overflow           proven   (layer 2: result within {0, 1})
line 12   'i' mod 2                    division by zero   proven   (layer 2: divisor 2)
line 12   'odd' + 1                    overflow           proven   (free name: width chosen so this cannot fail)
'odd' (line 9): uint16 — holds 0..500
5 site(s): 5 proven, 0 checked at run time, 0 trusted

$ kespar check examples/checkfail.kpls
error: `check` cannot remove the out of bounds check in `'xs'[…] =`: it fails for 'n'=10 at line 6
```

`kespar run` builds and runs; `--all-checks` keeps every check (the baseline
for `bench/run.sh`); `--count` reports VM steps, time, and how many emitted
instructions carry a check.

## The oracle

`oracle/` is a second implementation of `design/language.md`, written by a
separate agent from the spec alone, checking everything at run time.
`generator/` writes random valid programs and runs each, on random, edge and
(when small) every input, through Precog's build, the same VM with every check
kept, and the oracle. Any difference in output, exit code or where a check
fired is a finding; so is a check Precog removed that fires in the oracle, or
a free name holding a value outside the width Precog chose.

```
cd generator && cargo run --release -- run --cases 1000 --size 3
```

On the first runs it found, in Precog: unsigned element bounds encoded with
signed comparisons; a value recorded after its own check passed folded into a
constant, which deleted the check; the symbolic path continuing past a
failing check; a loop-shortcut that ignored bodies reading the counter; a
condition's own failing check not carried into its branches. All fixed and
all the kind of bug the oracle exists for.

## Building

Rust (`~/.cargo/bin`); Z3 from Homebrew (`brew install z3`). This Mac has an
unaccepted Xcode licence, so `.cargo/config.toml` points at the
CommandLineTools toolchain and Homebrew's Z3.

```
cd kespar && cargo build --release && cargo test
cd oracle && cargo build --release && cargo test
cd generator && cargo build --release
```
