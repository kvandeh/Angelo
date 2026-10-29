# Run it locally

This page tells you how to get a mutation score for a project on your computer. It also
tells you how to read the score. To use the score in CI, on a pull request or in
SonarQube, see [Integrations](integrations.md).

## Install it

```bash
pip install angelo-mut
```

The package name is `angelo-mut`. The command is `angelo`. A different project already
uses the name `angelo` on PyPI.

The wheel contains a compiled binary and no Python code. Thus any Python interpreter can
install it. One global installation works for all projects on the computer.

| Platform | Wheel |
| --- | --- |
| Windows x86-64 | yes |
| Linux x86-64 | yes, manylinux |
| macOS Apple Silicon | yes |
| Other platforms | build from source |

To build from source, you need a Rust toolchain:

```bash
git clone https://github.com/kvandeh/angelo.git
cd angelo
cargo build --release --manifest-path Angelo/Cargo.toml   # writes Angelo/target/release/angelo
```

!!! note "Test builds on TestPyPI"
    Each new version on the `develop` branch goes to TestPyPI as `angelo` before it goes to
    PyPI. Use it only to test a version that is not released:

    ```bash
    pip install --index-url https://test.pypi.org/simple/ --extra-index-url https://pypi.org/simple/ angelo
    ```

## Before you start

Angelo runs your existing test suite. It needs three things:

1. **Python and pytest on your PATH.**
2. **A suite that runs.** Some failing tests are permitted. Angelo gives a warning and does
   not score the mutants that those tests cover. See
   [A red baseline](#a-red-baseline-gives-a-warning). If pytest cannot run your suite at
   all, for example because a plugin is missing, Angelo stops and tells you the cause.
3. **`pip install coverage`.** This is optional, but recommended. Batching and test
   selection need coverage, and they give most of the speed. Without coverage, Angelo runs
   one mutant at a time.

## Run it

```bash
cd your-project
angelo init      # finds your layout and writes angelo.conf
angelo exec      # makes the mutants and runs them
```

`init` writes a configuration file that you can edit. `exec` does the work. If you stop
`exec`, run it again. It continues with the mutants that did not run. To start again from
the beginning, delete the `.angelo/` directory.

`init` does not overwrite an `angelo.conf` that exists. Thus your changes are safe. To
write a new file, use `angelo init --force`. Use `--force` in scripts, because a script
can otherwise use an old configuration file without a warning.

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

| What you see | What it means |
| --- | --- |
| The survivor list | Each line is a change that no test detected. `calculator.py:31 >= -> >` means that no test checks the boundary on line 31. |
| The report is last | A real project can have hundreds of survivors. The score is the last line, so it stays on your screen. |
| 9 mutants did not run | No test executes those lines. No test can kill them, so Angelo sets them to `survived` immediately. |
| 74 mutants, 17 runs | This is batching. Some mutants share one run. |
| A score of 62% | This is normal. Real projects usually have a score between 50% and 80%. A score of 100% usually means that there are too few mutants. |

To make a build fail on a low score, see [Integrations](integrations.md).

On a terminal, the verdicts have colours: detected is green, survived is yellow, error is
red, untestable is dim. When you redirect the output, or set `NO_COLOR` to any value, the
output has no colour codes. Thus `grep` and log files get plain text.

## Statuses

| Status | Meaning |
| --- | --- |
| `killed` | A test failed. The suite detected the mutant. |
| `survived` | All tests passed. The suite did not detect the mutant. |
| `timeout` | The mutant did not stop, for example because of an infinite loop. This counts as detected, because the behaviour changed. |
| `error` | The mutant broke the syntax or an import. It is not in the score, because the tests could not judge it. |
| `untestable` | Only tests that already failed cover this mutant. It is not in the score, for the same reason. |

!!! warning "Look at the error count first"
    If all mutants have the status `error` and the run is very fast, your test command is
    usually wrong. Your code is usually not the problem. Look at the `error` count before
    you trust a score.

## A red baseline gives a warning

The **baseline** is one run of your suite before Angelo makes any change. Many real suites
have some failing tests. Angelo does not stop for them.

```
baseline RED in 3.4s, timeout 11.8s for a whole-suite run, from its own tests for a selected one
warning: 3 tests were already failing before any mutant was planted
warning: 374 mutants set untestable, the tests covering them are already red
```

A failing test is dangerous for the score. A test that fails before the mutation also
fails after it. pytest then exits with code 1, and exit code 1 means `killed`. Angelo would
count mutants as detected when no test detected them.

Thus Angelo scores a mutant only when two conditions are true:

- Angelo can select the tests that cover the mutant.
- None of those tests failed in the baseline.

All other mutants get the status `untestable`. They are not in the score.

!!! warning "This needs coverage and test selection"
    [Test selection](02-test-selection.md) finds the tests for each mutant. Without
    `coverage`, or with `test_selection = false`, each run executes the full red suite.
    Each run then exits with code 1, and Angelo cannot find the correct verdict. In this
    case, Angelo stops and tells you why. No score is better than a false score of 100%.

## Progress bars

Each phase of a run shows progress.

| Phase | Display |
| --- | --- |
| Parse the source files | A bar. The number of files is known. |
| Run the baseline with coverage | A spinner and a clock. The duration is not known. |
| Make the batches | A bar |
| Run the mutants | A bar |

```
  mutants   [#####################---------------]  60%  1954/3210  detected 1502  survived 452  ~4m18s left
```

The remaining time is an estimate, so it starts with `~`. Batching completes mutants in
groups, and a failed batch can need more runs.

The bar updates a maximum of five times each second. Thus its cost depends on the duration
of the run, not on the number of mutants. When the output is not a terminal, Angelo does
not show the bar.

## Show fewer log lines

The report goes to stdout. Angelo always prints it. The log lines and the bar go to stderr.

```bash
angelo exec --verbosity warn
```

| Level | Shows |
| --- | --- |
| `error` | Only the errors that stopped a worker |
| `warn` | A red baseline, untestable mutants, a sample, a pattern that matched nothing |
| `info` | Each phase, and each group of mutants that Angelo removes |
| `debug` | One line for each mutant |
| `trace` | The pytest command line, and the tests that each run selected |

The default level is **`info`**. When the `CI` environment variable is set, the default is
**`warn`**. GitHub Actions sets `CI` on all its runners. `CI=false` and `CI=0` do not count.

You can also use `RUST_LOG`. The order of priority, from highest to lowest, is:
`--verbosity`, `RUST_LOG`, `CI`, then `info`.

## Keep the result

```bash
angelo exec --html-report angelo.html   # one self-contained file, it needs no network
angelo exec --report angelo.json        # the mutation-testing-report schema, version 2
```

A third flag, `--sonar-report`, writes a file for SonarQube. All three formats are
standard formats, so other tools can read them. See [Reports](07-reports.md) for the
contents, and [Integrations](integrations.md) for how to use them.

## Make the run smaller

A large project has many mutants. Three flags decrease the work:

```bash
angelo exec --diff            # only the lines that changed since HEAD
angelo exec --diff main       # only the lines that changed since a different revision
angelo exec --sample 500      # keep 500 random mutants, delete the others
```

Use `--diff` while you write code. It mutates only your current change. For a pull
request, use `--diff-base`. The two flags are different. See
[Integrations](integrations.md).

`--sample` **deletes** the other mutants from the database. The remaining mutants are a
random sample of the full project. Thus the score is an estimate, not a full count. Angelo
tells you this on each sampled run. See
[operators and sampling](06-operators-and-sampling.md).

## Configuration

`angelo.conf` uses TOML.

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
operators = [...]                    # which operator families to plant; see below
arid = ["log", "logger", "logging", "print", "warn", "warnings"]
per_line_cap = 0                     # 0 keeps every mutant on a line
```

If you do not write a field, it gets its default value. Thus a configuration file from an
older version of Angelo continues to work.

### Select the mutations

`operators` is the list of mutation families. The default list contains only the families
that research supports. Three families are off by default: unary operator insertion,
number literals and string literals. Research shows poor results for all three. To use a
family, add it to the list:

```toml
operators = [
    "ArithmeticOperator", "AssignmentOperator", "BitwiseOperator",
    "BlockStatement", "BooleanLiteral", "ConditionalExpression",
    "EqualityOperator", "LogicalOperator", "MethodExpression",
    "StatementSwap",
    "StringLiteral",           # this project's faults live in its literals
]
```

If Angelo does not know a name, **the run stops**. If Angelo ignored the name, it would
remove a family and the score would increase incorrectly.

`arid` is a list of calls that write text for people, for example log calls. If a part of
the call name is in the list, Angelo does not mutate that statement. Angelo also does not
mutate `__repr__` and `__str__`. For example, Angelo does not mutate any part of
`self.logger.info("got %d", n + 1)`. To mutate all statements, set `arid = []`.

`per_line_cap` is the maximum number of mutants on one line. Angelo removes the other
mutants at random. Like `sample`, it makes the score an estimate.

Angelo tells you how many mutants each setting removed, because each removal can increase
the score:

```
enumerated 412 mutants across 9 files, 38 skipped by operators and 21 as arid
```

For the research behind these choices, see
[operators and sampling](06-operators-and-sampling.md).

### Exclude code that you did not write

`paths` tells Angelo what to mutate. `exclude` removes parts of it: generated code, code
from other projects, or one module that always hangs. Nobody fixes survivors in such code.

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
| `**` | zero or more directories |
| `*` | zero or more characters in one name, but not `/` |
| other text | that name exactly |

Patterns are **relative to the project root**. Use forward slashes. Backslashes also work,
because Angelo changes both to the same form.

Angelo does not open an excluded directory, so an exclusion also saves time. Angelo prints
the number of excluded paths, because an exclusion can increase the score:
`enumerated 812 mutants across 34 files (3 paths excluded by angelo.conf)`. If a pattern
matches nothing, Angelo gives a warning, because the pattern can contain a typing error.

!!! warning "`exclude` applies only when Angelo makes the mutants"
    If `.angelo/` exists, a new pattern has no effect. The mutants exist already. Delete
    `.angelo/` and run again. The same rule applies to `--diff`.

## Troubleshooting

| Message or symptom | Cause and fix |
| --- | --- |
| "Angelo could not run the baseline suite." | pytest did not test your code. Exit code 3 usually means that a plugin in `pyproject.toml` is not installed. Exit code 5 means that pytest found no tests. Test failures are exit code 1, and they give only a warning. |
| "The baseline suite is red and Angelo cannot work around that here." | Your suite has failing tests, and Angelo cannot select tests around them. Install `coverage`. Use the default `python -m pytest` test command (the Python of a virtual environment is permitted). Keep `test_selection = true`. |
| All mutants are `error`. | Your test command is probably wrong. Run it yourself first. |
| All mutants survived and the score is 0.0%. | A pytest plugin in `addopts` controls coverage, usually pytest-cov or pytest-xdist. Angelo gives a warning that its coverage data matched no mutant. To fix this, add one flag to `test_command`. See [Neutralising a plugin](ai-setup.md#neutralising-a-plugin-in-addopts). |
| The score changes between identical runs. | Set `warm_workers = false` and try again. If this fixes the problem, please report it. Also set `warm_workers = false` when `test_command` has extra pytest arguments, because the warm worker does not use them. |
| "No matching distribution found for angelo-mut." | There is no wheel for your platform. There is no source package, so pip stops immediately. Build from source, or look at the platform table above. |
