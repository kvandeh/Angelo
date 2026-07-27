# Start here

This page takes you from nothing to a mutation score, then explains how to read it.

## Before you begin

Angelo drives your existing suite, so it needs three things:

1. **python and pytest on your PATH.**
2. **A green suite.** Angelo refuses to start otherwise, because a failing test cannot
   tell you anything about a planted fault. If pytest exits non zero, Angelo explains
   which kind of failure it saw and stops.
3. **`pip install coverage`.** Optional but strongly recommended. Coverage is what
   unlocks batching and test selection, which are most of the speed. Without it Angelo
   still works, one mutant per run.

## Run it

```bash
cd your-project
angelo init      # detects your layout, writes angelo.conf
angelo exec      # enumerates mutants, then runs them
```

`init` writes a config you can edit. `exec` does the work and is resumable: interrupt it,
run it again, and it picks up the pending mutants. To start over, delete `.angelo/`.

## Read the report

```
enumerated 74 mutants across 3 files
baseline green in 1.2s, timeout 7.4s for a whole-suite run, from its own tests for a selected one
9 mutants sit on lines no test executes, survived without a single run
running 17 batches on 8 workers, covering tests only

   killed: 46
 survived: 28
    score: 62.2% (46/74 detected)

survivors (changes your tests never noticed):
  calculator.py:31 >= -> >
  calculator.py:35 and -> or
  text.py:22 lower -> upper
```

Four things worth noticing.

**The survivors list is the point.** Each line is a change you can make to your code
without any test objecting. `calculator.py:31 >= -> >` says the boundary at that
comparison is untested.

**Nine mutants never ran.** No test executes those lines, so no test could possibly kill
them. Angelo marks them survived immediately rather than wasting a run.

**Seventy four mutants became seventeen runs.** That is batching.

**A score of 62 percent is normal.** Real projects commonly land between 50 and 80. A
score of 100 usually means too few mutants, not perfect tests.

## Statuses

| Status | Meaning |
| --- | --- |
| `killed` | A test failed. The suite caught it. |
| `survived` | Everything passed. The suite missed it. |
| `timeout` | The mutant hung, for example an infinite loop. Counts as caught, because the behaviour observably changed. |
| `error` | The mutant broke the syntax or an import. Excluded from the score, because it never got a fair trial. |

!!! warning "Check the error count first"
    A run where every mutant errors instantly looks fast and scores nothing useful. It
    almost always means a broken test command rather than a broken codebase. Read the
    `error` count before you trust any score.

## Make it faster or smaller

Large codebases produce a lot of mutants. Two options bound the work.

```bash
angelo exec --diff            # only lines changed since HEAD
angelo exec --diff main       # only lines changed since another revision
angelo exec --sample 500      # keep 500 mutants, drop the rest at random
```

`--diff` is the one to reach for during development, because it scopes mutation to the
change you are actually working on.

`--sample` behaves differently from what the name might suggest, and the difference
matters. It **deletes** the surplus mutants from the database rather than deferring them.
What remains is a random draw from the whole codebase, so the resulting score is an
estimate over a sample rather than a complete census. Angelo says so on every sampled
run. See [operators and sampling](06-operators-and-sampling.md).

## Configuration

`angelo.conf` is TOML.

```toml
paths = ["src"]                      # what to mutate
test_command = "python -m pytest"    # how to test it
workers = 0                          # 0 means one per CPU core
batch_size = 8                       # mutants per run, 1 disables batching
test_selection = true                # run only covering tests
warm_workers = true                  # keep a pytest process alive
warm_recycle_after = 50              # restart it every N runs
sample = 0                           # 0 keeps every mutant
timeout_factor = 2.0                 # timeout is a run's own tests * this, plus 5s
```

Any field you leave out takes its default, so a config written by an older Angelo keeps
working.

## When something goes wrong

**"Angelo needs a green baseline."** Your suite is not passing. Angelo prints which kind
of failure pytest reported. Exit code 1 means real test failures. Exit code 3 usually
means a plugin listed in `pyproject.toml` is not installed.

**Every mutant is `error`.** Your test command is probably wrong. Run it by hand first.

**Scores drift between runs.** Set `warm_workers = false` and try again. If that fixes
it, the warm process is carrying state between mutants and it is worth reporting.
