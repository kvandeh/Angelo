"""Long-lived pytest host: import once, judge many mutants.

Two ways to keep a mutant from contaminating the next one, chosen by what the
platform offers:

**fork** (Unix). The parent imports the project once and then never runs a
mutant itself. Each mutant runs in a forked child, which starts from a copy of
that pristine process and dies with whatever it broke. Nothing leaks, because
nothing survives.

**purge** (Windows, which has no fork). One process runs every mutant, and
between runs it drops every module loaded from this tree so the next run
re-imports the mutated source. Anything a test changed outside the tree stays
changed, which is what `warm_recycle_after` exists to bound.

Which mutant is live is set by an environment variable rather than by patching a
file, when the caller says the tree holds schemata (see schemata.rs). Then
nothing needs re-importing at all, and `purge` comes through false.

Protocol, one JSON object per line on stdin, one marked line back on stdout:
    in   {"tests": ["a.py::test_x"], "stop_at_first_failure": true,
          "mutants": "3,8", "purge": false, "timeout_ms": 5000}
    out  ##angelo##{"exit_code": 1, "failed": ["a.py::test_x"]}
An empty "tests" list means the whole suite.

Replies are prefixed because pytest writes its own progress to the same stdout;
the prefix is what tells the two apart.
"""

import gc
import json
import os
import select
import signal
import sys
import threading

import pytest

ROOT = os.path.abspath(os.getcwd())
REPLY_PREFIX = "##angelo##"
# Must match ACTIVE_VAR in schemata.rs.
ACTIVE_VAR = "ANGELO_MUTANTS"


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
    stays imported, that is the whole saving.

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


def run(request, forked):
    # Schemata ask for no purge, because nothing needs re-importing to switch a
    # mutant on. That is only safe when a fork gave this run a process of its
    # own: without one, the purge is the only thing resetting anything between
    # mutants, so it happens whatever the caller asked for.
    if request.get("purge", True) or not forked:
        purge_project_modules()
    active = request.get("mutants") or ""
    os.environ[ACTIVE_VAR] = active
    # Telling the runtime directly saves it an environment lookup on every call
    # of every mutated function. It is only loaded when the tree holds schemata,
    # and a purge drops it, so it is asked for rather than imported.
    runtime = sys.modules.get("_angelo_rt")
    if runtime is not None:
        runtime.set_active(active)
    outcome = Outcome()
    args = ["-q", "--no-header", "-p", "no:cacheprovider"]
    if request.get("stop_at_first_failure"):
        args.append("-x")
    args.extend(request.get("tests") or [])
    exit_code = pytest.main(args, plugins=[outcome])
    return {"exit_code": int(exit_code), "failed": outcome.failed}


def warm_up():
    """Import the project once, here in the parent, so every child inherits it.

    Collection imports every test module and everything it pulls in without
    running a single test. Safe only because a mutant never runs in this
    process: with schemata the child re-imports nothing, and with splicing the
    child purges first.
    """
    os.environ[ACTIVE_VAR] = ""
    quiet = open(os.devnull, "w")
    stdout, sys.stdout = sys.stdout, quiet
    try:
        pytest.main(["-q", "--no-header", "-p", "no:cacheprovider", "--collect-only"])
    except BaseException:
        pass  # a project that cannot be collected still runs, just cold
    finally:
        sys.stdout = stdout
        quiet.close()


def run_in_child(request, write_fd):
    """Never returns. pytest's own output would otherwise land on the stdout the
    parent uses for the protocol, so the child sends its verdict down a pipe of
    its own and writes nothing to fd 1 at all."""
    quiet = os.open(os.devnull, os.O_WRONLY)
    os.dup2(quiet, 1)
    sys.stdout = open(os.devnull, "w")
    try:
        result = run(request, forked=True)
    except BaseException as error:  # a broken mutant must not look like a pass
        result = {"exit_code": 3, "failed": [], "error": repr(error)}
    try:
        with os.fdopen(write_fd, "w") as pipe:
            pipe.write(json.dumps(result))
    finally:
        os._exit(0)


def run_forked(request):
    """One mutant, one child, one reaped pid.

    The deadline is the parent's: a child that hangs is killed by process group,
    which reaches anything the test spawned, and the parent lives on. Under the
    purge path the same hang costs a whole worker restart.
    """
    timeout = (request.get("timeout_ms") or 0) / 1000.0 or None
    read_fd, write_fd = os.pipe()
    sys.stdout.flush()

    pid = os.fork()
    if pid == 0:
        os.close(read_fd)
        os.setsid()  # so killpg reaches the child's own subprocesses too
        run_in_child(request, write_fd)

    os.close(write_fd)
    try:
        ready, _, _ = select.select([read_fd], [], [], timeout)
        if not ready:
            os.killpg(os.getpgid(pid), signal.SIGKILL)
            os.waitpid(pid, 0)
            return {"exit_code": 0, "failed": [], "timed_out": True, "forked": True}
        with os.fdopen(read_fd, "r") as pipe:
            read_fd = None
            reply = pipe.read()
        os.waitpid(pid, 0)  # reap, or thousands of mutants leave zombies
    finally:
        if read_fd is not None:
            os.close(read_fd)
    # An empty pipe means the child died before it could answer, a segfault or
    # an OOM kill. That is a real error, not a pass.
    if not reply:
        return {"exit_code": 3, "failed": [], "error": "the mutant killed its own process"}
    result = json.loads(reply)
    # Tells the caller this worker cannot accumulate state, so it never needs
    # recycling and the warm-up it would pay again is not worth paying.
    result["forked"] = True
    return result


def forkable():
    """Fork is only defined in a single-threaded process, and Python 3.12 warns
    about it. Collection runs project code, which may have started a thread, so
    this is asked after the warm-up rather than before it."""
    return hasattr(os, "fork") and threading.active_count() == 1


def main():
    forking = hasattr(os, "fork")
    if forking:
        warm_up()
        forking = forkable()
        if forking:
            # The child inherits tens of thousands of tracked objects. Without
            # this the collector walks them, dirties every copy-on-write page it
            # touches, and the fork costs more than it saves.
            gc.freeze()

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
            result = run_forked(request) if forking else run(request, forked=False)
        except Exception as error:  # a broken mutant must not kill the worker
            result = {"exit_code": 3, "failed": [], "error": repr(error)}
        sys.stdout.write(REPLY_PREFIX + json.dumps(result) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
