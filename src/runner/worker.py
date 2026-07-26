"""Long-lived pytest host: import once, judge many mutants.

Windows has no fork(), so angelo cannot clone a warm interpreter per mutant.
This is the substitute: keep one process alive and reset the part that
actually changes — the project's own modules — between runs.

Protocol, one JSON object per line on stdin, one marked line back on stdout:
    in   {"tests": ["a.py::test_x"], "stop_at_first_failure": true}
    out  ##angelo##{"exit_code": 1, "failed": ["a.py::test_x"]}
An empty "tests" list means the whole suite.

Replies are prefixed because pytest writes its own progress to the same
stdout; the prefix is what tells the two apart.
"""

import json
import os
import sys

import pytest

ROOT = os.path.abspath(os.getcwd())
REPLY_PREFIX = "##angelo##"


class Outcome:
    """Records failures without going through a junit file."""

    def __init__(self):
        self.failed = []

    def pytest_runtest_logreport(self, report):
        if report.failed and report.nodeid not in self.failed:
            self.failed.append(report.nodeid)


def purge_project_modules():
    """Drop every module loaded from this tree so the next run re-imports the
    mutated source. Anything outside the tree (pytest, stdlib, site-packages)
    stays imported — that is the whole saving.

    __main__ is this driver, which also lives in the tree: dropping it breaks
    any stdlib that does `import __main__` (pdb does, via rlcompleter)."""
    for name, module in list(sys.modules.items()):
        if name == "__main__":
            continue
        path = getattr(module, "__file__", None)
        if not path:
            continue
        if os.path.abspath(path).startswith(ROOT):
            del sys.modules[name]


def run(request):
    purge_project_modules()
    outcome = Outcome()
    args = ["-q", "--no-header", "-p", "no:cacheprovider"]
    if request.get("stop_at_first_failure"):
        args.append("-x")
    args.extend(request.get("tests") or [])
    exit_code = pytest.main(args, plugins=[outcome])
    return {"exit_code": int(exit_code), "failed": outcome.failed}


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            result = run(json.loads(line))
        except Exception as error:  # a broken mutant must not kill the worker
            result = {"exit_code": 3, "failed": [], "error": repr(error)}
        sys.stdout.write(REPLY_PREFIX + json.dumps(result) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
