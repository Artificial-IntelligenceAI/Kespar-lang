# bench

`run.sh` — the same VM with every check kept versus after Precog.
`run_lua.sh` — the same four programs in Lua 5.5 and LuaJIT 2.1 (`lua/`),
wall-clock, against Kespar's VM run time; Precog's own compile time is the
last column and is spent once.

Apple M5, 2026-09-16:

| program | Lua 5.5 | LuaJIT | Kespar, all checks | Kespar, Precog | Precog compile |
|---|---|---|---|---|---|
| sum_squares (n read, 5000 reps) | 0.021 s | 0.004 s | 0.277 s | 0.284 s | 0.02 s |
| sum_squares_known (n = 1000) | 0.016 s | 0.003 s | 0.282 s | **0.000 s** (folded to its output) | 0.30 s |
| sieve (30 reps) | 0.013 s | 0.004 s | 0.060 s | 0.060 s | 2.6 s |
| collatz (300 reps) | 0.027 s | 0.006 s | 0.046 s | 0.046 s | 3.1 s |

What it says: Kespar's stack VM (i128 arithmetic, no register allocation,
no JIT) is 3–13× slower than PUC Lua and 10–70× slower than LuaJIT — the
"shit performance already" the owner accepted for a bytecode POC. Removing
the proven checks is within noise on this VM, because a check is one
compare on a value the VM already holds; the win Precog demonstrates
here is the read-free case, where the whole program becomes its output.
Precog's compile time is the price of following loops trip by trip (no
Z3 was needed for any of these four) — "compile time is free" is the
design's premise, not this benchmark's. The sieve was 14.7 s until layer 2
stopped cloning whole states to read a condition, joined loop-exit states
in place, made exact sets reference-counted, and stopped spelling out
ranges as sets past 64 values.
