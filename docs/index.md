# angelo

> Python mutation testing is slow and expensive, but what if it wasn't?

angelo measures how good your tests actually are. It breaks your code on purpose, one
small change at a time, and checks whether your test suite notices. It is a single Rust
binary that drives your ordinary pytest suite.

On a project with 200 planted faults and a two second suite, angelo takes **4.5 seconds**
where the standard approach takes **48 seconds**. The score is identical.

## What mutation testing measures

Test coverage tells you a line ran. It does not tell you whether anything checked the
result. A test suite can execute every line of a function and still pass when that
function is wrong.

Mutation testing answers the harder question. Change the code so it is definitely broken,
then run the tests:

| Term | Meaning |
| --- | --- |
| **Mutant** | One deliberate small change, such as `+` becoming `-` |
| **Killed** | A test failed. Your suite caught the bug. Good. |
| **Survived** | Every test passed. Your suite missed a real bug. |
| **Score** | killed / (killed + survived) |

A survivor is the useful output. It points at a line where you can break the behaviour
and nobody complains.

```python
def is_adult(age):
    return age >= 18
```

Change `>=` to `>` and the function now rejects eighteen year olds. If your tests still
pass, you never tested the boundary. angelo tells you that.

## Why it is normally slow

The standard approach runs the whole test suite once per mutant. Five hundred mutants
means five hundred runs. Worse, most of each run is not testing at all. Measured on a
selected single test:

| Step | Time |
| --- | --- |
| Start the interpreter | 18 ms |
| Import pytest | 155 ms |
| Collect and configure | 139 ms |
| Run the actual test | 15 ms |

Roughly 95 percent of the work is setup. That is the problem angelo attacks.

## How angelo is fast

Four techniques, each measured, each documented in its own note.

| Technique | Idea | Speedup |
| --- | --- | --- |
| [Batch mutating](01-batch-mutating.md) | Test several mutants in one run | 4.6x |
| [Test selection](02-test-selection.md) | Run only the tests that touch the mutant | 2.8x |
| [Warm workers](03-warm-workers.md) | Keep one pytest process alive | 2.5x |
| [Diff mode](06-operators-and-sampling.md) | Only mutate lines you changed | varies |

### Batching in one picture

Batching is the central idea, so it is worth understanding before anything else.

Normally each mutant gets its own run:

```mermaid
flowchart LR
    M1[mutant 1] --> R1[full test run]
    M2[mutant 2] --> R2[full test run]
    M3[mutant 3] --> R3[full test run]
```

angelo puts several mutants into the same run:

```mermaid
flowchart LR
    M1[mutant 1] --> R[one test run]
    M2[mutant 2] --> R
    M3[mutant 3] --> R
    R --> V[three verdicts]
```

The catch is obvious. If the run fails, which mutant caused it? angelo solves this by
only grouping mutants that **no single test can reach at the same time**. It learns which
test touches which line from one coverage run, before any mutating starts. Two mutants
that share a test are kept apart. A failing test therefore points at exactly one mutant.

[Read the full argument](01-batch-mutating.md), including what happens when a result
cannot be explained.

## The rule that governs everything

Batching, test selection and warm workers are **speed features only**. None of them is
allowed to change a verdict.

This is enforced, not promised. `scripts/verdict-matrix.sh` runs eight configurations
against the same project and fails the build if any of them disagrees about the score. It
runs in continuous integration on every push. A speedup that changes a result is treated
as a bug.

## Where to go next

- **New here?** [Start here](start-here.md) installs angelo and reads your first report.
- **Want the evidence?** [Benchmarks](05-benchmarks.md) has every measurement, including
  the ones that did not work out.
- **Curious how it is built?** [Architecture](04-architecture.md).
- **Want to help?** [Support](support.md), or read [CONTRIBUTING.md](https://github.com/kvandeh/angelo/blob/main/CONTRIBUTING.md) in the repository.
