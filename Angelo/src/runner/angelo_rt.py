"""Picks the mutant under test. Written into the worker copy, never the project.

The generated schemata hold every mutant of a file at once; this decides which
of them is live. An environment variable holds the ids, so a batch is a list and
an empty value runs the original code, which is what the baseline and every
unrelated test get.

This sits on somebody else's hot path: it runs on every call of every mutated
function in the project, for the whole test run, not just for the mutant being
judged. So the answer is worked out once per function per batch and cached, and
the steady-state cost of a call is an integer compare and a list index.
"""

import os

ACTIVE_VAR = "ANGELO_MUTANTS"

# None until somebody resolves it, so a cold subprocess can fall back to the
# environment without the warm path paying for a lookup on every call.
_active = None
# Bumped whenever the live set changes, which is what tells a cached answer it
# has gone stale. The warm worker changes it once per mutant.
_generation = 0


def set_active(raw):
    """Tell the runtime which mutants are live.

    The warm worker calls this directly, so a forked child never reads the
    environment at all. A cold subprocess never calls it and falls back to the
    variable, which it inherits once and which never changes for its lifetime.
    """
    global _active, _generation
    parsed = tuple(int(part) for part in (raw or "").split(",") if part)
    if parsed != _active:
        _active = parsed
        _generation += 1


def _angelo_pick(orig, mutants, cache):
    """The original, unless one of this function's mutants is live.

    `cache` is a two-slot list held as a default argument of the wrapper, one
    per function. A mutable default is usually a bug; here it is the point, it
    is the only per-function storage available without a lookup.

    A batch never holds two mutants of the same function -- they would be
    indistinguishable in the result -- so the first match is the only match.
    """
    if _active is None:
        set_active(os.environ.get(ACTIVE_VAR, ""))
    if cache[0] != _generation:
        cache[0] = _generation
        cache[1] = orig
        for mutant_id in _active:
            mutated = mutants.get(mutant_id)
            if mutated is not None:
                cache[1] = mutated
                break
    return cache[1]
