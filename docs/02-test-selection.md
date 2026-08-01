# Test selection

!!! abstract "In one sentence"
    A fault can only be caught by a test that executes it, so Angelo runs those tests and
    no others. Measured at **2.8x faster** on its own, though it partly competes with
    batching.

## The problem

Planting a fault in one function and then running three thousand tests is mostly waste.
Only the handful of tests that actually execute that function can possibly notice.

This is not a new observation. PIT built its reputation on it, describing the difference
as minutes instead of days. Stryker calls per test coverage analysis its single largest
speedup setting.

Angelo already collected the necessary data for [batching](01-batch-mutating.md) and was
using it for one purpose only.

## The awkward part

Coverage and pytest do not spell a test the same way.

| Source | How it names one test |
| --- | --- |
| coverage.py context | `pkg.test_mod.test_x` |
| pytest node id | `pkg/test_mod.py::test_x` |

Converting between them is not string manipulation, because a dotted name is ambiguous.
Given `a.b.c`, the module could be `a/b/c.py`, or it could be `a/b.py` containing a class
`c`. Nothing in the name says which.

Angelo resolves this by walking prefixes and taking the longest one that exists as a file
on disk. The test inventory comes from the junit report of the baseline run, which lists
every test with its class name.

```mermaid
flowchart LR
    A[coverage contexts] --> C[match]
    B[baseline junit report] --> C
    C --> D[pytest node ids]
    D --> E[run only those tests]
```

## Two rules that keep it honest

**Anything unresolvable falls back to running everything.** If a single covering test
cannot be named exactly, Angelo runs the whole suite for that batch. Running too many
tests costs time. Running too few would invent survivors, which is the one failure this
tool must never have.

**A run judging a single mutant stops at the first failure.** Once one test has failed,
the mutant is caught, and the rest of the suite has nothing left to say. A batch never
stops early, because it needs to see every failure in order to attribute them.

## The budget follows the selection

A run that has chosen its tests is also **timed** by them.

| Run | Budget |
| --- | --- |
| Selected tests | their baseline time × `timeout_factor` + 5 s |
| Whole suite | the suite's duration × `timeout_factor` + 5 s |

The per-test durations come free: the baseline run's junit report already lists every test
with a `time` attribute.

**Why this is not a small saving.** A timeout costs its entire budget, every time. On
click a 4.21 second suite gave every mutant about 22 seconds, including the mutants whose
tests are worth 50 milliseconds, and 60 to 76 mutants collected that bill on every run.
That is the measurement where batching and selection
[bought nothing at all](05-benchmarks.md#a-real-codebase-where-the-numbers-do-not-hold).

**The floor is a constant, deliberately.** Interpreter start, imports and collection are
most of a short run and none of it appears in a junit `time`. Scaling the headroom with
the tests would give the fastest tests the least room, which is backwards. Five seconds
covers a cold start on a loaded machine, and it has to, because any warm worker failure
falls back to a cold subprocess.

## Which direction the error runs

A budget that is too generous wastes time. A budget that is too tight **invents kills**,
because a timeout counts as detected. Those two mistakes are not equally bad, so both
terms err long.

The honest limit: a mutant that makes the code slower without making it hang now crosses a
tighter line than before, and a mutant like that would be scored as detected where it once
survived. The
[verdict matrix](https://github.com/kvandeh/angelo/blob/main/scripts/verdict-matrix.sh)
now carries a deliberately hanging mutant, so a configuration that disagrees about a
timeout fails the build rather than the reader.

## Result

Two hundred mutants, eight workers.

| suite length | batching alone | selection alone | both |
| --- | --- | --- | --- |
| 2.0 s | 4.6x | **2.8x** | **7.7x** |
| 0.2 s | 3.7x | 1.05x | 4.0x |

## The interaction worth understanding

Selection is worth 2.8x at a batch size of one, but only 1.7x at a batch size of eight.

The reason is structural. A batch runs the **union** of its members' covering tests. The
larger the batch, the larger that union, and the closer it gets to simply running
everything. The two features are competing for the same saving.

```mermaid
flowchart LR
    subgraph one [batch of 1]
        A1[1 mutant] --> B1[1 test]
    end
    subgraph eight [batch of 8]
        A2[8 mutants] --> B2[8 tests]
    end
```

They still stack to 7.7x, which is well short of the 12.9x their product would suggest.
No published figure exists for this interaction, so the honest advice is to measure
`batch_size` on your own project rather than trusting a default.

## Limits

- Selection removes **test** time, not **startup** time. On a fast suite there is very
  little test time to remove, which is why the 0.2 second row shows almost nothing.
  [Warm workers](03-warm-workers.md) attack the other half.
- Parametrised tests collapse to their function, so the selection is a safe superset.
- Requires coverage.py and a `python -m pytest` test command.
