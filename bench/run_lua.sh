#!/bin/sh
# The same four programs in Lua, timed whole-process (wall clock, microsecond
# timer), against Kespar's VM: every check kept, and after Precog. Kespar's
# time is the run alone; Precog's compile time is listed separately (spent once).
cd "$(dirname "$0")"
K=../kespar/target/release/kespar
wall() { inp="$1"; shift; perl hrtime.pl "$inp" "$@" 2>&1; }
printf "%-18s %-10s %-10s %-14s %-14s %-12s\n" program lua5.5 luajit "kespar-checks" "kespar-precog" "precog-compile"
for s in sum_squares sum_squares_known sieve collatz; do
  inp="$s.in"; [ -s "$inp" ] || inp=/dev/null
  l=$(wall "$inp" lua "lua/$s.lua")s
  j=$(wall "$inp" luajit "lua/$s.lua")s
  c=$($K run --count --all-checks "$s.kpls" < "$inp" 2>&1 >/dev/null | sed -n 's/.*executed in \([0-9.]*\)s.*/\1s/p')
  p=$($K run --count "$s.kpls" < "$inp" 2>&1 >/dev/null | sed -n 's/.*executed in \([0-9.]*\)s.*/\1s/p')
  t=$(wall /dev/null $K check "$s.kpls")s
  printf "%-18s %-10s %-10s %-14s %-14s %-12s\n" "$s" "$l" "$j" "$c" "$p" "$t"
done
