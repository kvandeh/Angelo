# Start here

This page takes you from nothing to a mutation score, then explains how to read it.

## Before you begin

Angelo drives your existing suite, so it needs three things:

1. **python and pytest on your PATH.**
2. **A suite that runs.** A handful of already-failing tests is fine: Angelo warns, refuses
   to score the mutants those tests cover, and measures the rest. See
   [a red baseline](#a-red-baseline-warns-rather-than-stops). What it cannot work with is a
   suite pytest never judged at all, such as a missing plugin, and it says which it saw.
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

survivors (changes your tests never noticed):
  calculator.py:31 >= -> >
  calculator.py:35 and -> or
  text.py:22 lower -> upper

=== mutation report ===
    killed: 46
  survived: 28
     score: 62.2% (46/74 detected)
```

Five things worth noticing.

**The survivors list is the point.** Each line is a change you can make to your code
without any test objecting. `calculator.py:31 >= -> >` says the boundary at that
comparison is untested.

**The report comes last on purpose.** A real codebase produces hundreds of survivors, and
the score is the number you ran the command for. Printed above the list it would scroll off
the top; printed below it, it is the last line on screen.

**Nine mutants never ran.** No test executes those lines, so no test could possibly kill
them. Angelo marks them survived immediately rather than wasting a run.

**Seventy four mutants became seventeen runs.** That is batching.

**A score of 62 percent is normal.** Real projects commonly land between 50 and 80. A
score of 100 usually means too few mutants, not perfect tests.

Verdicts are **coloured on a terminal**: detected green, survived yellow, error red,
untestable dim. Redirect the output, or set `NO_COLOR` to anything at all, and it comes
back byte for byte plain, because a log full of escape codes helps nobody and `grep` least
of all.

## Statuses

| Status | Meaning |
| --- | --- |
| `killed` | A test failed. The suite caught it. |
| `survived` | Everything passed. The suite missed it. |
| `timeout` | The mutant hung, for example an infinite loop. Counts as caught, because the behaviour observably changed. |
| `error` | The mutant broke the syntax or an import. Excluded from the score, because it never got a fair trial. |
| `untestable` | The only tests covering it were already failing. Excluded from the score, for the same reason. |

!!! warning "Check the error count first"
    A run where every mutant errors instantly looks fast and scores nothing useful. It
    almost always means a broken test command rather than a broken codebase. Read the
    `error` count before you trust any score.

## A red baseline warns rather than stops

Real suites carry a few known-red tests: something flaky, something xfailing on this
platform, one module halfway through a rewrite. Blocking mutation testing of an entire
codebase over three of them is not useful, so Angelo does not.

```
baseline RED in 3.4s, timeout 11.8s for a whole-suite run, from its own tests for a selected one
warning: 3 tests were already failing before any mutant was planted
warning: 374 mutants set untestable, the tests covering them are already red
```

The part worth keeping is the **reason** the rule existed. A test that was already failing
fails again under a mutant, pytest exits 1, and exit 1 means `killed`. Every mutant those
tests touch would be scored as detected without anything detecting anything. That is
exactly this tool's characteristic failure — inventing a test gap that does not exist —
running in the flattering direction.

So Angelo refuses to score them instead. A mutant is judgeable only when its run can name
its own tests and **avoid every already-failing one**. The rest come back `untestable` and
sit outside the score, next to `error`.

!!! warning "This needs coverage and test selection"
    Naming a mutant's tests is exactly what [test selection](02-test-selection.md) does.
    Without `coverage` installed, or with `test_selection = false`, every run executes the
    whole red suite and comes back exit 1, so there is no way to tell a contaminated mutant
    from a clean one. Angelo stops there and says so, because a confident 100 percent is
    worse than no answer.

## Big runs draw a bar

Past **1000 mutants**, a line each stops being a report and becomes a wall. Angelo collapses
them into one redrawn line instead.

```
  [#####################---------------]  60%  1954/3210  detected 1502  survived 452  ~4m18s left
```

The remaining time is a linear extrapolation and nothing better: batching settles mutants
in clumps, and a batch that goes red costs several more runs while it bisects. Hence the
`~`. Set `show_loading = true` to force the bar on a smaller project, in CI where
scrollback costs money. `error` lines always print on their own line and the bar redraws
underneath them.

## Make it faster or smaller

Large codebases produce a lot of mutants. Two options bound the work.

```bash
angelo exec --diff            # only lines changed since HEAD
angelo exec --diff main       # only lines changed since another revision
angelo exec --diff-base main  # only the lines this branch adds on top of main
angelo exec --sample 500      # keep 500 mutants, drop the rest at random
```

`--diff` is the one to reach for during development, because it scopes mutation to the
change you are actually working on. `--diff-base` is the one for a pull request, and the
next section is about why they are not the same thing.

`--sample` behaves differently from what the name might suggest, and the difference
matters. It **deletes** the surplus mutants from the database rather than deferring them.
What remains is a random draw from the whole codebase, so the resulting score is an
estimate over a sample rather than a complete census. Angelo says so on every sampled
run. See [operators and sampling](06-operators-and-sampling.md).

## Run it on a pull request

`--diff` compares a revision against **your working tree**. That is the right question
while you are editing and the wrong one in CI, where a pushed branch has nothing
uncommitted and the answer is therefore nothing at all.

`--diff-base` compares against the **merge base** instead: the point where your branch left
the base. That is what the branch adds, however many commits it took, and whether or not
the base branch moved on since.

| Flag | Question it answers | What it compares |
| --- | --- | --- |
| `--diff [REV]` | What is different on this machine right now? | `REV` against the working tree |
| `--diff-base [REV]` | What does this branch add on top of `REV`? | `REV` against the branch, from where they last met |

```yaml
- uses: actions/checkout@v4
  with:
    fetch-depth: 0
- run: angelo exec --diff-base $GITHUB_BASE_REF
```

Given no revision, `--diff-base` works one out: the branch the pull request targets, then
origin's own default branch, then `main` or `master`.

!!! warning "`fetch-depth: 0` is not optional"
    Checkouts fetch a single commit by default, and a shallow clone has no merge base to
    diff from. Angelo stops and tells you rather than quietly comparing something else,
    because the alternative is a confident score for lines you never wrote.

**A branch that changes no Python enumerates zero mutants.** Angelo says so and prints no
score, because zero mutants is zero information rather than a pass.

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
show_loading = false                 # force the progress bar under 1000 mutants
```

Any field you leave out takes its default, so a config written by an older Angelo keeps
working.

## When something goes wrong

**"Angelo could not run the baseline suite."** pytest never got as far as judging your
code, so there is nothing to measure. Exit code 3 usually means a plugin listed in
`pyproject.toml` is not installed; exit code 5 means it collected no tests at all. Real
test failures are exit code 1, and those only warn.

**"The baseline suite is red and Angelo cannot work around that here."** Your suite has
failing tests *and* no way to route around them. Install `coverage`, keep the default
`python -m pytest` test command, and leave `test_selection = true`.

**Every mutant is `error`.** Your test command is probably wrong. Run it by hand first.

**Scores drift between runs.** Set `warm_workers = false` and try again. If that fixes
it, the warm process is carrying state between mutants and it is worth reporting.
