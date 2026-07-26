#!/usr/bin/env bash
# Measure angelo's four configurations against one real repository.
# Usage: ./scripts/bench-repo.sh <repo-dir> [sources] [tests]
set -uo pipefail

REPO=${1:?usage: bench-repo.sh <repo-dir> [sources] [tests]}
SOURCES=${2:-}
TESTS=${3:-tests}
ANGELO=$(cd "$(dirname "$0")/.." && pwd)/target/release/angelo
PYTHON=$(cd "$REPO" && pwd)/.venv/bin/python
OUT=bench-results.md

# src-layout projects keep their package under src/; flat ones do not.
if [ -z "$SOURCES" ]; then
    SOURCES=$([ -d "$REPO/src" ] && echo src || echo .)
fi

seconds() { date +%s.%N; }
took() { echo "scale=2; ($2 - $1)/1" | bc; }

measure() {
    local batch=$1 selection=$2 warm=$3
    rm -rf "$REPO/.angelo"
    cat > "$REPO/angelo.conf" <<EOF
paths = ["$SOURCES"]
test_command = "$PYTHON -m pytest $TESTS"
workers = 0
batch_size = $batch
test_selection = $selection
warm_workers = $warm
warm_recycle_after = 50
timeout_factor = 4.0
EOF
    local start end out
    start=$(seconds)
    out=$(cd "$REPO" && "$ANGELO" exec --workers 8 2>&1)
    end=$(seconds)
    printf '| %s | %s | %s | %ss | %s |\n' \
        "$batch" "$selection" "$warm" "$(took "$start" "$end")" \
        "$(echo "$out" | grep -E '^\s+(killed|survived|timeout|error):' | tr -d ' ' | sort | tr '\n' ' ')" \
        >> "$OUT"
}

start=$(seconds)
(cd "$REPO" && .venv/bin/python -m pytest -q -p no:cacheprovider "$TESTS" >/dev/null 2>&1)
end=$(seconds)

{
    echo "# Benchmark: $REPO"
    echo
    echo "Suite: **$(took "$start" "$end")s** · sources: \`$SOURCES\` · tests: \`$TESTS\`"
    echo
    echo "| batch | selection | warm | time | verdicts |"
    echo "|---|---|---|---|---|"
} > "$OUT"

measure 1 false false
measure 1 true false
measure 8 true false
measure 8 true true

{
    echo
    echo "Every row must show the same verdicts. A difference is a correctness bug."
} >> "$OUT"

cat "$OUT"
