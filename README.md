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
