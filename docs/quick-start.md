# Run it locally

This page takes you from nothing to a mutation score on a project you have open right now,
then explains how to read it. Putting that score into CI, onto a pull request or onto a
SonarQube dashboard is [Integrations](integrations.md).

## Install it

Angelo ships as a wheel, so `pip` is the way in. Real PyPI is still to come; releases go to
**TestPyPI** while the publishing pipeline is being proven.

```bash
pip install --index-url https://test.pypi.org/simple/ angelo
```

The wheel holds a compiled binary and no Python at all. `pip` puts `angelo` on your PATH,
nothing imports it, and it does not care which interpreter installed it — so a global
install works on every project on the machine.

| Platform | Wheel |
| --- | --- |
| Windows x86-64 | yes |
| Linux x86-64 | yes, manylinux |
| macOS Apple Silicon | yes |
| Anything else | build from source |

Building from source needs a Rust toolchain and nothing else:

```bash
git clone https://github.com/kvandeh/angelo.git
cd angelo
cargo build --release   # Angelo/target/release/angelo
```

!!! warning "TestPyPI is a sandbox, not a mirror"
    That flag points pip at TestPyPI **instead of** PyPI, so anything else in the same
    command fails to resolve. Install Angelo on its own, or keep PyPI in the search:

    ```bash
    pip install --index-url https://test.pypi.org/simple/ --extra-index-url https://pypi.org/simple/ angelo
    ```

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
16:20:04 INFO   enumerated 74 mutants across 3 files
16:20:06 INFO   baseline green in 1.2s, timeout 7.4s for a whole-suite run, from its own tests for a selected one
16:20:06 INFO   9 mutants sit on lines no test executes, survived without a single run
16:20:06 INFO   running 17 batches on 8 workers, covering tests only

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

A score can also **fail a build** rather than just print. That is
[Integrations](integrations.md).

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

## Every phase draws a bar

A run has four waits, and each one shows something moving.

| Phase | Shape |
| --- | --- |
| Parsing every source file | A bar, the file count is known |
| The baseline suite under coverage | A spinner with the clock running |
| Composing batches | A bar |
| Running the mutants | A bar |

```
  mutants   [#####################---------------]  60%  1954/3210  detected 1502  survived 452  ~4m18s left
```

The baseline spins rather than filling because **nobody can know how long a test suite
takes** — that is the question it is asking. It is the longest single wait in a run, and it
used to show nothing at all.

The remaining time is a linear extrapolation and nothing better: batching settles mutants
in clumps, and a batch that goes red costs several more runs while it bisects. Hence the `~`.

**A bar costs nothing per mutant.** It repaints at most five times a second, so the cost
scales with how long the run takes and not with how many mutants it has. That is the fix: a
line per mutant got more expensive exactly when a run could least afford it.

Off a terminal the bar disappears by itself, so a redirected or piped run emits no control
characters at all.

## Turn the commentary down

**The report is output; everything else is commentary.** The report goes to stdout and
prints at every verbosity — a script can rely on it. The timestamped lines and the bar go to
stderr.

```bash
angelo exec --verbosity warn
```

| Level | Carries |
| --- | --- |
| `error` | Only what stopped a worker |
| `warn` | A red baseline, untestable mutants, a sample, a pattern that matched nothing |
| `info` | Phase transitions, and every mutant Angelo drops |
| `debug` | A line per mutant, the way older versions always printed |
| `trace` | The pytest command line and the node ids each run selected |

The default is **`info`**, or **`warn` when the `CI` environment variable is set**, since in
CI nobody is watching it scroll past. GitHub Actions sets `CI` on its Windows and macOS
runners too, so this is the signal rather than the operating system. `CI=false` and `CI=0`
turn it back off.

`RUST_LOG` is honoured for anyone who wants per-module control. Precedence, highest first:
`--verbosity`, then `RUST_LOG`, then `CI`, then `info`.

## Keep the result

```bash
angelo exec --html-report angelo.html   # one self-contained file, opens with no network
angelo exec --report angelo.json        # the mutation-testing-report schema, version 2
```

There is a third, `--sonar-report`, for SonarQube. All three are formats that already exist
rather than shapes Angelo invented, so other tools read an Angelo run directly: see
[reports](07-reports.md) for what each one holds, and [Integrations](integrations.md) for
feeding them to something.

## Make it faster or smaller

Large codebases produce a lot of mutants. Two options bound the work.

```bash
angelo exec --diff            # only lines changed since HEAD
angelo exec --diff main       # only lines changed since another revision
angelo exec --sample 500      # keep 500 mutants, drop the rest at random
```

`--diff` is the one to reach for during development, because it scopes mutation to the
change you are actually working on. There is a second one, `--diff-base`, for a pull
request — and the difference between them is not cosmetic, so it is spelled out in
[Integrations](integrations.md).

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
warm_recycle_after = 50              # restart it every N runs (purge path only)
schemata = true                      # compile every mutant into its file at once;
                                     # Unix only, needs warm_workers
sample = 0                           # 0 keeps every mutant
timeout_factor = 2.0                 # timeout is a run's own tests * this, plus 5s
exclude = []                         # globs to leave alone
fail_under = 0                       # 0 means no threshold, exit 1 below this score
report = ""                          # write the run here in the report schema, "" is off
html_report = ""                     # write one self-contained HTML file here, "" is off
sonar_report = ""                    # write SonarQube's issue import format here, "" is off
```

Any field you leave out takes its default, so a config written by an older Angelo keeps
working.

### Excluding code you did not write

`paths` says what to mutate. `exclude` carves out the parts of it that are not worth a
score: generated code, vendored code, or one module that hangs and eats the timeout budget
on every run. Survivors there are noise nobody will act on.

```toml
paths = ["src"]
exclude = [
  "**/migrations/**",     # generated by the ORM
  "src/generated/*.py",   # protobuf, OpenAPI clients
  "src/legacy_parser.py", # one file
]
```

| In a pattern | Matches |
| --- | --- |
| `**` | any run of directories, including none |
| `*` | any run of characters inside one name, never a `/` |
| anything else | that name, exactly |

Patterns are **relative to the project root** and written with forward slashes. Backslashes
work too, on either side, because Angelo normalises both before matching.

An excluded directory is never descended into, so an exclusion is a saving rather than a
scan. Angelo reports the count — `enumerated 812 mutants across 34 files (3 paths excluded
by angelo.conf)` — because a silent exclusion quietly raises the score. A pattern that
matches nothing is not an error, but Angelo warns about it, since a typo is otherwise
invisible.

!!! warning "`exclude` applies at enumeration"
    Adding a pattern to a run that already has an `.angelo/` changes nothing: those mutants
    were enumerated already. Delete `.angelo/` and re-run. Same rule as `--diff`.

## When something goes wrong

**"Angelo could not run the baseline suite."** pytest never got as far as judging your
code, so there is nothing to measure. Exit code 3 usually means a plugin listed in
`pyproject.toml` is not installed; exit code 5 means it collected no tests at all. Real
test failures are exit code 1, and those only warn.

**"The baseline suite is red and Angelo cannot work around that here."** Your suite has
failing tests *and* no way to route around them. Install `coverage`, keep the default
`python -m pytest` test command (a virtualenv's own interpreter counts), and leave `test_selection = true`.

**Every mutant is `error`.** Your test command is probably wrong. Run it by hand first.

**Scores drift between runs.** Set `warm_workers = false` and try again. If that fixes
it, the warm process is carrying state between mutants and it is worth reporting.

**"No matching distribution found for angelo."** There is no wheel for your platform, and
there is deliberately no sdist, so pip fails immediately rather than compiling Rust for five
minutes. Build from source, or check the wheel table above.
