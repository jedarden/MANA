#!/bin/bash
# Benchmark MANA injection latency

for i in 1 2 3 4 5 6 7 8 9 10; do
  start=$(date +%s%N)
  echo '{"tool": "Bash", "input": {"command": "cargo build"}}' | .mana/mana inject --tool bash >/dev/null 2>&1
  end=$(date +%s%N)
  echo "Run $i: $(( (end - start) / 1000000 ))ms"
done
