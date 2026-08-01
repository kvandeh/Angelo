#!/usr/bin/env bash
# angelo's central safety claim: batching, test selection and warm workers are
# speed features only. Every configuration must produce the SAME score on the
# same project. This script fails CI if any of them disagrees.
#
# The fixture deliberately contains one mutant that hangs. A timeout counts as
# detected, so a timeout budget that is too tight does not merely slow a run
# down, it invents a kill; and a budget derived from the selected tests differs
# per configuration. That is precisely the disagreement this script must catch,
# so the hanging mutant is load-bearing and not merely decorative.
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


# Two mutable tokens in one function, on branches no single test covers
# together. Schemata put every mutant of a function behind one wrapper, and the
# wrapper can only call one copy, so batching these two would leave the second
# switched off and score it survived. Load-bearing, like `spin` below.
def fee(kind, amount):
    if kind == "flat":
        return amount + 1
    return amount * 2


# `not` is the only mutable token here, and removing it spins forever.
def spin(flag):
    while not flag:
        pass
    return flag
PY

cat > "$WORK/project/test_calc.py" <<'PY'
from calc import add, clamp, fee, is_adult, scale, spin


def test_spin():
    assert spin(True)


def test_fee_flat():
    assert fee("flat", 10) == 11


def test_fee_rate():
    assert fee("rate", 10) == 20


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
    local batch=$1 selection=$2 warm=$3 schemata=$4
    rm -rf "$WORK/run" && cp -r "$WORK/project" "$WORK/run"
    cat > "$WORK/run/angelo.conf" <<EOF
paths = ["."]
test_command = "python -m pytest"
workers = 0
batch_size = $batch
test_selection = $selection
warm_workers = $warm
warm_recycle_after = 10
schemata = $schemata
timeout_factor = 4.0
EOF
    # `untestable` is exactly as wide as the summary's column, so it starts at
    # column zero where every other status is indented. Missing it would let a
    # configuration quietly refuse to score a mutant the others judged.
    (cd "$WORK/run" && "$OLDPWD/$ANGELO" exec --workers 2 2>&1) |
        grep -E '^\s*(killed|survived|timeout|error|untestable):' |
        tr -d ' ' | sort | tr '\n' ' '
}

OLDPWD=$PWD
echo "config                                             verdicts"
echo "--------------------------------------------------------------"

baseline=""
failed=0
configurations=0
for batch in 1 8; do
    for selection in true false; do
        for warm in true false; do
            # schemata do nothing without the fork worker and without a
            # platform that has fork(), so on Windows both settings run the
            # same path. Running both anyway costs one fixture and proves it.
            for schemata in true false; do
                verdicts=$(run "$batch" "$selection" "$warm" "$schemata")
                printf 'batch=%-2s selection=%-5s warm=%-5s schemata=%-5s  %s\n' \
                    "$batch" "$selection" "$warm" "$schemata" "$verdicts"
                configurations=$((configurations + 1))
                if [ -z "$baseline" ]; then
                    baseline="$verdicts"
                elif [ "$verdicts" != "$baseline" ]; then
                    echo "  ^^ MISMATCH: expected '$baseline'"
                    failed=1
                fi
            done
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
echo "PASS: all $configurations configurations agree on $baseline"
