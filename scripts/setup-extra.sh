#!/usr/bin/env bash
# Give each repo in extra/ its own venv with its own dependencies.
#
# Real projects pin plugins in pyproject.toml. Running them against your global
# Python gives collection errors and a pytest internal error, not a green
# baseline — so angelo refuses to start, correctly.
set -uo pipefail

EXTRA=$(cd "$(dirname "$0")/.." && pwd)/extra
PYTHON=${PYTHON:-python3}

setup() {
    local name=$1
    local dir=$EXTRA/$name
    [ -d "$dir" ] || { echo "skip $name (not cloned)"; return; }

    echo "=== $name"
    if [ ! -d "$dir/.venv" ]; then
        "$PYTHON" -m venv "$dir/.venv" || { echo "  venv failed"; return; }
    fi
    local pip=$dir/.venv/bin/pip
    [ -f "$pip" ] || pip=$dir/.venv/Scripts/pip.exe

    "$pip" install -q --upgrade pip
    "$pip" install -q pytest coverage || { echo "  pytest install failed"; return; }
    # Most of these declare their own test extras; fall back to a plain install.
    "$pip" install -q -e "$dir[tests]" 2>/dev/null ||
        "$pip" install -q -e "$dir[test]" 2>/dev/null ||
        "$pip" install -q -e "$dir" 2>/dev/null ||
        echo "  editable install failed — suite may not be green"

    local py=$dir/.venv/bin/python
    [ -f "$py" ] || py=$dir/.venv/Scripts/python.exe
    if (cd "$dir" && "$py" -m pytest -q -x --co >/dev/null 2>&1); then
        echo "  collects cleanly"
    else
        echo "  does NOT collect cleanly — needs extra dependencies"
    fi
    echo "  test_command = \"$py -m pytest tests\""
}

for name in click flask httpx requests fastapi; do
    setup "$name"
done

cat <<'NOTE'

django is cloned but uses its own test runner (runtests.py), not pytest.
angelo drives pytest only, so django needs pytest-django and a settings module
before it can be mutated.
NOTE
