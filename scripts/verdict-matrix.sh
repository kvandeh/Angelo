#!/usr/bin/env bash
# angelo's central safety claim: batching, test selection and warm workers are
# speed features only. Every configuration must produce the SAME score on the
# same project. This script fails CI if any of them disagrees.
set -uo pipefail

ANGELO=${ANGELO:-./target/release/angelo}
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

mkdir -p "$WORK/project"
cat > "$WORK/project/calc.py" <<'PY'
def add(a, b):
    return a + b


def scale(x, factor):
    return x * factor


def is_adult(age):
    return age >= 18


def clamp(value, low, high):
    if value < low:
        return low
    if value > high:
        return high
    return value


def untested(x):
    return x + 1
PY

cat > "$WORK/project/test_calc.py" <<'PY'
from calc import add, clamp, is_adult, scale


def test_add():
    assert add(2, 3) == 5


def test_scale():
    assert scale(3, 4) == 12


def test_is_adult():
    assert is_adult(30)
    assert not is_adult(10)


def test_clamp():
    assert clamp(1, 5, 10) == 5
    assert clamp(7, 5, 10) == 7
PY

run() {
    local batch=$1 selection=$2 warm=$3
    rm -rf "$WORK/run" && cp -r "$WORK/project" "$WORK/run"
    cat > "$WORK/run/angelo.conf" <<EOF
paths = ["."]
test_command = "python -m pytest"
workers = 0
batch_size = $batch
test_selection = $selection
warm_workers = $warm
warm_recycle_after = 10
timeout_factor = 4.0
EOF
    (cd "$WORK/run" && "$OLDPWD/$ANGELO" exec --workers 2 2>&1) |
        grep -E '^\s+(killed|survived|timeout|error):' | tr -d ' ' | sort | tr '\n' ' '
}

OLDPWD=$PWD
echo "config                              verdicts"
echo "--------------------------------------------------------------"

baseline=""
failed=0
for batch in 1 8; do
    for selection in true false; do
        for warm in true false; do
            verdicts=$(run "$batch" "$selection" "$warm")
            printf 'batch=%-2s selection=%-5s warm=%-5s  %s\n' \
                "$batch" "$selection" "$warm" "$verdicts"
            if [ -z "$baseline" ]; then
                baseline="$verdicts"
            elif [ "$verdicts" != "$baseline" ]; then
                echo "  ^^ MISMATCH: expected '$baseline'"
                failed=1
            fi
        done
    done
done

echo "--------------------------------------------------------------"
if [ "$failed" -ne 0 ]; then
    echo "FAIL: an optimisation changed the verdicts"
    exit 1
fi
if [ -z "$baseline" ] || [ "$baseline" = " " ]; then
    echo "FAIL: no verdicts produced at all"
    exit 1
fi
echo "PASS: all 8 configurations agree on $baseline"
