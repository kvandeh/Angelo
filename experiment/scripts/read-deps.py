"""Report how one project declares its test dependencies, as JSON on stdout.

The corpus harness cannot guess this. `pip install -e .[tests]` exits 0 with only
a warning when the extra does not exist, so guessing looks like success and
installs nothing -- which is how 24 of 50 repos reached pytest with no test
dependencies at all. Read what the project actually declares instead.

Usage: python read-deps.py <project-dir>
"""

import json
import os
import sys

# PEP 735 dependency-groups and extras, in the order worth trying. A project
# that declares several gets the most test-specific one first; "dev" is last
# because it usually drags in linters and docs builders as well.
PREFERRED = ("test", "tests", "testing", "test-random", "dev", "develop")

# Requirements files, by the names projects actually use. Checked in order and
# every match is reported, since trio splits test and docs into separate files.
REQUIREMENT_NAMES = (
    "test-requirements.txt",
    "test_requirements.txt",
    "requirements-test.txt",
    "requirements_test.txt",
    "dev-requirements.txt",
    "dev_requirements.txt",
    "requirements-dev.txt",
    "requirements_dev.txt",
)

REQUIREMENT_DIRS = ("requirements", "requirements.d")


def ordered(names):
    """Preferred names first, in PREFERRED order, then whatever else is left."""
    names = list(names)
    preferred = [name for name in PREFERRED if name in names]
    return preferred + [name for name in names if name not in preferred]


def load_pyproject(project):
    path = os.path.join(project, "pyproject.toml")
    if not os.path.exists(path):
        return {}
    try:
        import tomllib
    except ModuleNotFoundError:  # Python 3.10 and older
        return {}
    try:
        with open(path, "rb") as handle:
            return tomllib.load(handle)
    except Exception:
        # A pyproject we cannot parse is not a reason to stop; the caller still
        # has extras-free fallbacks to try.
        return {}


def requirement_files(project):
    found = []
    for name in REQUIREMENT_NAMES:
        path = os.path.join(project, name)
        if os.path.isfile(path):
            found.append(name)
    for directory in REQUIREMENT_DIRS:
        full = os.path.join(project, directory)
        if not os.path.isdir(full):
            continue
        # Only the test-ish ones: a requirements/ dir usually also holds docs
        # and lint pins, which cost minutes and buy nothing here. Plain test.txt
        # sorts ahead of test-integration.txt, which wants a live broker.
        entries = [
            entry
            for entry in sorted(os.listdir(full))
            if entry.lower().endswith(".txt")
            and ("test" in entry.lower() or "dev" in entry.lower())
        ]
        entries.sort(key=lambda entry: (len(entry), entry))
        found.extend(f"{directory}/{entry}" for entry in entries)
    return found


def poetry_groups(data):
    """Poetry keeps its dev dependencies outside PEP 735, under tool.poetry."""
    poetry = data.get("tool", {}).get("poetry", {})
    groups = list(poetry.get("group", {}) or {})
    if poetry.get("dev-dependencies"):
        groups.append("dev-dependencies")
    return groups


def main():
    project = sys.argv[1]
    data = load_pyproject(project)
    project_table = data.get("project", {}) or {}
    groups = ordered(data.get("dependency-groups", {}) or {})
    extras = ordered(project_table.get("optional-dependencies", {}) or {})
    report = {
        "groups": groups,
        "extras": extras,
        "poetry_groups": ordered(poetry_groups(data)),
        "requirements": requirement_files(project),
        "requires_python": project_table.get("requires-python", ""),
        # What the harness should actually install: the names that mean tests,
        # never the whole list. Installing a "docs" group costs minutes and can
        # pin a conflicting pytest.
        "test_groups": [name for name in groups if name in PREFERRED],
        "test_extras": [name for name in extras if name in PREFERRED],
    }
    json.dump(report, sys.stdout)


if __name__ == "__main__":
    main()
