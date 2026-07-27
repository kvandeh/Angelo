# Benchmarks

!!! abstract "In one sentence"
    Stacked, Angelo's optimisations take a 48 second job down to 4.5 seconds with
    unchanged verdicts. This note records the measurements, the comparison against mutmut,
    and one bug that made an earlier set of numbers false.

## Method

- **Machine.** WSL Ubuntu on Windows 11, 16 cores, Python 3.14.
- **Synthetic project.** Forty independent functions, one test each, 200 mutants. A
  `conftest.py` sleep sets the suite length, so the same mutant pool can be measured
  against a fast suite and a slow one.
- **Control.** The score. Every configuration must produce the same one.
- Single runs, not averages. Treat any difference under about 10 percent as noise.

## The feature matrix

Two hundred mutants, eight workers.

| Suite | Batch | Selection | Warm | Time | Score |
| --- | --- | --- | --- | --- | --- |
| 0.2 s | 1 | off | off | 15.88 s | 39.5% |
| 0.2 s | 8 | on | off | 3.99 s | 39.5% |
| 0.2 s | 8 | on | on | **2.16 s** | 39.5% |
| 2.0 s | 1 | off | off | 48.13 s | 39.5% |
| 2.0 s | 8 | on | off | 6.22 s | 39.5% |
| 2.0 s | 8 | on | on | **4.45 s** | 39.5% |

**10.8x on the slow suite, 7.4x on the fast one.** The score never moved.

## Where a run's time goes

One selected single test run, measured by isolating each step:

```mermaid
pie showData
    title One 327ms selected run
    "interpreter start" : 18
    "import pytest" : 155
    "collect and configure" : 139
    "the actual test" : 15
```

About 95 percent is overhead. This is why [warm workers](03-warm-workers.md) exist, and
why [test selection](02-test-selection.md) alone reaches a ceiling.

## Which feature pays when

| Feature | Fast suite | Slow suite |
| --- | --- | --- |
| Batching | 3.7x | 4.6x |
| Test selection | 1.05x | 2.8x |
| Warm workers | 2.5x | 1.9x |

The rule of thumb: **selection removes test time, warm workers remove startup time.** A
project with a slow suite wants the first. A project with many cheap tests wants the
second. Batching helps both, though it partly competes with selection.

## A real codebase, where the numbers do not hold

The synthetic result does not survive contact with click. 957 mutants, a 4.21 second
suite, eight workers.

| Batch | Selection | Warm | Time | Per mutant | Detected | Survived |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | off | off | 1119 s | 1.170 s | 540 | 336 |
| 1 | on | off | 1189 s | 1.242 s | 540 | 336 |
| 8 | on | off | 1130 s | 1.181 s | 541 | 335 |

**The optimisations bought essentially nothing, and test selection was slightly slower
than no selection at all.** Every configuration lands near 1.2 seconds per mutant.

The likely cause is visible in the verdicts. Between 60 and 76 mutants **time out** on
every run. A timeout costs the full timeout budget, here about 22 seconds, and no amount
of batching or test selection shortens it. On click, waiting for timeouts dominates
everything the optimisations save.

The synthetic project has no timeouts at all, which is precisely why it showed 10.8x.

!!! note "The budget was the bug, and it has changed"
    Those 22 seconds came from the **whole suite's** duration, charged to every mutant,
    including the ones whose selected tests are worth 50 milliseconds. A run is now
    budgeted from [the tests it actually runs](02-test-selection.md#the-budget-follows-the-selection).
    **These three rows predate that change and have not been re-measured.** The claim
    still under test is that batching and selection start paying on a repository where
    they previously did not; until click is re-run on the same machine, treat the table
    above as the last honest measurement rather than the current one.

!!! warning "Read the synthetic numbers as an upper bound"
    Independent functions, disjoint tests, and no timeouts is the best case for every
    technique in this tool. A real project with slow or hanging mutants can see no
    speedup whatsoever.

### Verdicts moved slightly, and why that is not the batching bug

The three rows disagree: 540, 540 and 541 detected; 336, 336 and 335 survived.

This is **timeout classification**, not misattribution. A mutant sitting near the timeout
threshold is detected on a loaded machine and survives on an idle one, because
`timeout_factor` is a wall clock budget. The kill and timeout columns also trade against
each other between runs for the same reason.

The [verdict matrix](https://github.com/kvandeh/angelo/blob/main/scripts/verdict-matrix.sh)
that runs in continuous integration used a fixture with no timeouts, so it could not catch
this class of variation at all. That gap is now closed: the fixture contains one mutant
that deliberately spins forever, and all eight configurations must agree that it timed
out. A budget derived from the selected tests differs per configuration, which is exactly
the disagreement the matrix exists to catch.

### An invalid fourth row

A fourth configuration was measured, but the operator set was expanded while the
benchmark was running, so it planted 4596 mutants instead of 957. It is excluded here
rather than reported, because it compares two different tools.

### flask

Skipped. Its suite is not green on this machine out of the box, and Angelo will not run
against a red baseline.

## Comparison against mutmut

Same machine, same project, both at eight workers.

| Tool | Mutants | Wall time | Per mutant |
| --- | --- | --- | --- |
| Angelo, batch 16 | 200 | 2.5 s | 0.0125 s |
| mutmut 3.6.0 | 360 | 3.7 s | **0.0103 s** |

**mutmut is about 1.2x cheaper per mutant.** It uses schemata plus `fork()`; Angelo comes
close with neither. The operator sets differ, so per mutant cost is the only fair column.

mutmut **cannot run on Windows at all**, because it requires `fork()`.

## The bug that made earlier numbers false

Four configurations that must agree reported **77, 73, 69 and 78** kills.

A `.pyc` file is reused when the source's recorded modification time in whole seconds and
its byte size both still match. Same size replacements, such as `+` becoming `-`, written
in the same second as the previous one, therefore ran the **old bytecode**. The mutant
survived for free.

- Invisible on Windows, where a 1.9 second suite pushes writes into different seconds.
- Obvious on Linux, where a 0.2 second suite lands many runs inside one second.
- Fixed by never writing bytecode. Note that the environment variable alone is **not**
  sufficient if a `.pyc` already exists, because Python still reads it.

The lesson is worth stating plainly: a mutation tester's characteristic failure is
inventing test gaps that do not exist. The verdict matrix in continuous integration exists
because of this bug.

## What these numbers do not prove

- The synthetic project has **independent functions with disjoint tests**, which is the
  best possible case for batching. Real code shares tests, so expect less.
- Single runs on one machine, with no confidence intervals.
- Windows process spawning is far slower than Linux. Only compare within a platform.
- A tool that dies instantly on every mutant looks fast and scores zero. **Check the
  error count before trusting any score.**

## Reproducing

```bash
bash scripts/verdict-matrix.sh          # the correctness gate, runs in CI
bash scripts/setup-extra.sh             # a virtualenv per repository in extra/
bash scripts/bench-repo.sh extra/click  # the feature matrix on a real project
```

`extra/` holds gitignored shallow clones of click, flask, httpx, requests, fastapi and
django.

**Each needs its own virtualenv.** Real projects pin pytest plugins in `pyproject.toml`.
Run them against a global Python and pytest exits 3, an internal error, before collecting
anything. Angelo then refuses to start, correctly, but the fix is dependencies rather than
Angelo.

**django is cloned but Angelo cannot mutate it**, because it uses its own test runner
rather than pytest.
