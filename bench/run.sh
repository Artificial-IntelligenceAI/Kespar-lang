#!/bin/sh
# The perf comparison: the same VM, every check kept (--all-checks) versus
# after Precog removed the proven ones. The time is the VM run alone
# (Precog's own compile time is reported separately and is not the point).
set -e
cd "$(dirname "$0")"
K=../kespar/target/release/kespar
(cd ../kespar && cargo build --release -q)
for f in *.kpls; do
  s=${f%.kpls}
  echo "== $s"
  $K check --report "$f" | tail -1
  printf '  all checks: '; $K run --count --all-checks "$f" < "$s.in" 2>&1 >/dev/null | grep steps
  printf '  precog:     '; $K run --count "$f" < "$s.in" 2>&1 >/dev/null | grep steps
done
