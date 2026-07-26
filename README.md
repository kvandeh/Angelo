# angelo

Fast mutation testing for Python. One Rust binary, installed through pip.

```
angelo init
angelo exec
```

## What it does

Mutation testing plants small bugs (**mutants**) in your code and runs your tests. A test
fails → the mutant is **killed**: your suite caught it. Everything passes → it
**survived**: your suite missed a real bug. The **score** is killed / (killed + survived).

It is normally slow, because one mutant means one full test run. angelo attacks that from
four sides:

| Feature | What it skips | Measured |
|---|---|---|
| [Batch mutating](docs/01-batch-mutating.md) | Runs, several mutants per run | 4.6x |
| [Test selection](docs/02-test-selection.md) | Tests, only those that cover the mutant | 2.8x |
| [Warm workers](docs/03-warm-workers.md) | Python startup, one live pytest process | 2.5x |
| `--diff` | Mutants, only lines you changed | scope-dependent |
| [`--sample N`](docs/06-operators-and-sampling.md) | Mutants, a random N of them | scope-dependent |

The [operator set](docs/06-operators-and-sampling.md) matches mutmut's: **63 mutants to
mutmut's 61** on the same file.

Stacked: **48.1s → 4.5s (10.8x)** on a 200-mutant project with a 2s suite, same score.

Coverage also retires mutants for free: one no test executes **survives without running**.

## The rule

Every one of those is a **speed feature only**. Batching, selection and warm workers must
never change a verdict, `scripts/verdict-matrix.sh` runs 8 configurations in CI and fails
the build if any disagrees.

## Usage

```
angelo init                 # detect the project, write angelo.conf
angelo exec                 # resumable; delete .angelo/ for a fresh run
angelo exec --workers 8
angelo exec --init-only     # enumerate only, inspect before running
angelo exec --diff          # only lines changed since HEAD
angelo exec --diff main     # only lines changed since another revision
angelo exec --sample 500    # keep 500 mutants, drop the rest at random
```

`angelo.conf` (TOML): `paths`, `test_command`, `workers`, `batch_size`, `test_selection`,
`warm_workers`, `warm_recycle_after`, `sample`, `timeout_factor`.

**`--sample` deletes rows**, it does not defer them: the surviving mutants are a random
sample of the whole codebase, and the score is an estimate over that sample. See
[note 06](docs/06-operators-and-sampling.md).

Results land in `.angelo/angelo.db`, plain SQLite format, open it with anything.

## Requirements

python and pytest on PATH. `pip install coverage` unlocks batching and test selection;
without it angelo still works, one mutant per run.

## Docs

[docs/](docs/), one short note per idea, each written as abstract → background → method →
result → limits.

## Status

In development. Not on PyPI yet.
