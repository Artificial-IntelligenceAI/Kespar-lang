# The oracle — catching Kespar lying

Compiler-development tooling. Never user-facing. Owner's direction,
2026-09-16: three-way comparison plus claim verification.

## The engines

1. **The reference interpreter** (`oracle/`) — a second, deliberately
   obvious implementation of `design/language.md`, written by a separate
   Claude agent from that file alone, sharing no code with `kespar/`
   (owner, 2026-09-16). It has no Precog: every check site is checked at
   run time, every time. It is the executable specification: what Kespar
   does is what the oracle does.
2. **The real pipeline** — `kespar build` then the VM: Precog decided
   which checks stay.
3. **Precog's compile-time answer** — for a program Precog executed fully
   (no read), the output it folded in, without running the VM.

Xag's rule (Tankun, 2026-09-07) carries over: *"If one disagrees, it's OUR
problem. If both agree, it's THEIR problem."* Three engines can also say
which one stands apart, which is not the same as which one is wrong — the
reference interpreter has been the wrong one before (Xag, 2026-09-07,
two engines shared a mistake for months).

## What is compared

For every generated program and every generated input line-set:

- **stdout**, byte for byte;
- **exit code** (0 / 1 / 2 / 3);
- **stderr**, which for exits 1 and 2 names the failure kind and the line
  — so two engines that both stop must stop *at the same site for the
  same reason*. Output alone is not enough: Xag learned that two engines
  silent in the same way is not agreement.

Any difference is a **finding**: the seed, the program, the inputs, every
engine's answer, and which engine is the odd one out.

## Claim verification — the new part

ITMT compared what two engines *did*. Precog also makes **claims**: "this
site cannot fail", "'odd' never exceeds 500". A wrong claim is the worst
bug CyborgPL could ship, and it is invisible to a plain output comparison
until an input happens to hit it. So the oracle attacks the claims
directly:

1. `kespar check --report --types` lists every **proven** site and every
   free name's width.
2. The reference interpreter is run on many inputs for the same program:
   random within the declared bounds, plus every **edge**: each bound's
   ends, 0, ±1, the type's min and max, empty and maximum-length lists and
   strs, and for lists a mix of element edges.
3. The reference interpreter, run with `--trace-checks`, reports every
   check site that **fired** or **would have fired**. If any proven site
   fires, or any free name holds a value outside its chosen width, Precog's
   proof was wrong — a finding of the highest severity, kept separate from
   ordinary disagreements.
4. When a site's domain is small enough (the product of input domains ≤
   **2^24**, **provisional**), the oracle enumerates it **exhaustively**
   across every core instead of sampling. Then "no input trips it" is a
   fact, not a probability — and for those programs the solver's `unsat`
   has been checked against the ground truth.

The reverse is checked too: a site Precog reports as **checked** with a
counterexample must actually fail on that counterexample in the reference
interpreter. A counterexample that does not reproduce is a Precog bug of
the second kind (unsound the other way — claiming danger that isn't there
is not unsafe, but it is wrong).

## What the oracle never does

- It never decides a design question. When it exposes one — two engines
  disagree because `language.md` is silent — the finding says so and the
  question goes to the owner.
- It never runs in the shipped `kespar`. Nothing here is user-facing.
- It never uses a clock to decide anything: the reference interpreter
  takes a `--steps N` budget for generated programs that might not
  terminate, and reaching it is reported as "budget", which every engine
  must reach alike (the VM takes the same flag for the oracle's use).

## Interfaces the engines expose to it

- `kespar check --report --types --json file.kpls` — the sites, tiers,
  outcomes, counterexamples, widths.
- `kespar run --steps N file.kpls < input` — the VM with a step budget.
- `oracle --steps N --trace-checks file.kpls < input` — the reference
  interpreter, printing after the run one line per check site:
  `site <line> <kind> fired|clean`, and per free name `name '<n>' min <v>
  max <v>`.
- `oracle --types file.kpls` — the reference interpreter's own forcing
  result (`language.md` §8.3 step 1) per `:=` name, so a forcing
  disagreement is caught before any run.
