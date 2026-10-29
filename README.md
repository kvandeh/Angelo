<h1 align="center">Angelo</h1>

<p align="center">
  <em>Fast mutation testing for Python.</em>
</p>

<p align="center">
  <a href="https://pypi.org/project/angelo-mut/">
    <img alt="PyPI" src="https://img.shields.io/pypi/v/angelo-mut?color=orange">
  </a>
  <a href="https://angelo.kcvdh.com/">
    <img alt="Documentation" src="https://img.shields.io/badge/docs-angelo.kcvdh.com-blue">
  </a>
  <a href="https://github.com/sponsors/kvandeh">
    <img alt="Sponsor" src="https://img.shields.io/badge/sponsor-%E2%9D%A4-db61a2">
  </a>
</p>

<p align="center">
  <b><a href="https://angelo.kcvdh.com/quick-start/">Run it locally</a></b> ·
  <a href="https://angelo.kcvdh.com/integrations/">Integrations</a> ·
  <a href="https://angelo.kcvdh.com/">Documentation</a> ·
  <a href="https://angelo.kcvdh.com/05-benchmarks/">Benchmarks</a>
</p>

---

Angelo measures the quality of your tests. It makes small changes to your code on purpose.
Then it runs your pytest suite and reports each change that no test detected. Angelo is
one Rust binary. It does not replace pytest. It uses your existing suite.

## The problem

Coverage tells you that a line ran. It does not tell you that a test checked the result.

```python
def is_adult(age):
    return age >= 18
```

Change `>=` to `>`. The function now rejects people who are 18. If all your tests still
pass, no test checks this boundary. This is true even at 100% coverage. Angelo finds this
line and reports it.

| Term | Meaning |
| --- | --- |
| **Mutant** | One deliberate change to your code |
| **Killed** | At least one test failed. Your suite detected the change. |
| **Survived** | All tests passed. Your suite did not detect the change. |
| **Score** | Detected mutants divided by scored mutants |

Read the survivors first. Each survivor is a bug that you can ship and no test will fail.

## Install

```bash
pip install angelo-mut
```

The package is `angelo-mut` because a different project already uses the name `angelo` on
PyPI. The command is `angelo`.

The wheel contains only the compiled binary. It contains no Python code, so any Python
interpreter can install it.

| Platform | Wheel |
| --- | --- |
| Windows x86-64 | yes |
| Linux x86-64 | yes (manylinux) |
| macOS Apple Silicon | yes |
| Other platforms | build from source |

To build from source, you need a Rust toolchain:

```bash
git clone https://github.com/kvandeh/angelo.git
cd angelo/Angelo
cargo build --release   # writes target/release/angelo
```

## Quick start

```bash
cd your-project
angelo init      # finds your layout and writes angelo.conf
angelo exec      # makes the mutants and runs them
```

```
16:20:04 INFO   enumerated 74 mutants across 3 files
16:20:09 INFO   baseline green in 4.8s, timeout 14.5s for a whole-suite run, from its own tests for a selected one
16:20:09 INFO   9 mutants sit on lines no test executes, survived without a single run
16:20:09 INFO   running 17 batches on 16 workers, covering tests only

survivors (changes your tests never noticed):
  calculator.py:31 >= -> >
  calculator.py:35 and -> or

=== mutation report ===
    killed: 46
  survived: 28
     score: 62.2% (46/74 detected)
```

The report goes to stdout. The log lines and the progress bar go to stderr. A script can
read the report at all verbosity levels. To show fewer log lines, use `--verbosity warn`.

For the full first run, see [Run it locally](https://angelo.kcvdh.com/quick-start/).

## Use it in CI

This step mutates only the lines that a pull request adds. It fails when the score is
below 80%.

```yaml
- uses: actions/checkout@v4
  with:
    fetch-depth: 0
- run: pip install angelo-mut coverage
- run: angelo exec --diff-base $GITHUB_BASE_REF --fail-under 80
```

A run that Angelo cannot score also fails `--fail-under`. A tool that did not measure must
not report success.

## Reports

| Flag | Writes |
| --- | --- |
| `--html-report angelo.html` | One self-contained HTML file. It needs no network. |
| `--report angelo.json` | The [mutation-testing-report schema](https://github.com/stryker-mutator/mutation-testing-elements/tree/master/packages/report-schema), version 2. Stryker viewers can read it. |
| `--sonar-report angelo-sonar.json` | SonarQube's generic issue format. You do not install anything on the server. |

A self-hosted SonarQube can also show the score as a metric, with history and a quality
gate. For this, install the [plugin](https://angelo.kcvdh.com/10-sonar-plugin/). SonarQube
Cloud does not run third-party plugins.

## Why it is fast

Most of the time in a mutation run is not test time. For one selected test, approximately
95% of 327 ms is Python startup and the pytest import. Angelo removes work in four ways:

| Technique | What it removes |
| --- | --- |
| [Batch mutating](https://angelo.kcvdh.com/01-batch-mutating/) | Runs. Some mutants share one test run. |
| [Test selection](https://angelo.kcvdh.com/02-test-selection/) | Tests. A run executes only the tests that reach its mutants. |
| [Warm workers](https://angelo.kcvdh.com/03-warm-workers/) | Startup. One pytest process stays alive between runs. |
| `--diff`, `--diff-base`, `--sample` | Mutants. Only changed lines, or a random sample. |

On a synthetic project with 200 mutants and a 2-second suite, the run time decreases from
**48.1 s to 4.5 s**. The score does not change.

> [!WARNING]
> This number is a best case. The project has independent functions and no timeouts. On
> the real [click](https://github.com/pallets/click) codebase, approximately 7% of mutants
> hang until the timeout. There, the wait for timeouts is most of the run time. The
> [benchmarks](https://angelo.kcvdh.com/05-benchmarks/) page shows both results.

## The rule everything obeys

Batching, test selection and warm workers make a run faster. **They must not change a
verdict.**

[`scripts/verdict-matrix.sh`](scripts/verdict-matrix.sh) enforces this rule. It runs eight
configurations on the same project. If two configurations give different scores, the build
fails. CI runs it on each pull request. A speed-up that changes a result is a bug.

## Requirements

- **Python and pytest** on your PATH.
- **A test suite that runs.** Some failing tests are permitted. Angelo gives a warning and
  does not score the mutants that those tests cover.
- **`pip install coverage`**. This is optional, but recommended. Batching and test
  selection need coverage. Without it, Angelo runs one mutant at a time.

Angelo runs on Windows, Linux and macOS. It does not need `fork()`.

## Documentation

The full documentation is at **[angelo.kcvdh.com](https://angelo.kcvdh.com/)**.

| Page | Contents |
| --- | --- |
| [Run it locally](https://angelo.kcvdh.com/quick-start/) | First run, the report, configuration, troubleshooting |
| [Integrations](https://angelo.kcvdh.com/integrations/) | CI, pull requests, SonarQube, PyPI |
| [Batch mutating](https://angelo.kcvdh.com/01-batch-mutating/) | The conflict rule, and why it is safe |
| [Test selection](https://angelo.kcvdh.com/02-test-selection/) | From coverage data to pytest node ids |
| [Warm workers](https://angelo.kcvdh.com/03-warm-workers/) | The replacement for `fork()` on Windows |
| [Architecture](https://angelo.kcvdh.com/04-architecture/) | Module map and design rules |
| [Benchmarks](https://angelo.kcvdh.com/05-benchmarks/) | All measurements, also the bad results |
| [Operators and sampling](https://angelo.kcvdh.com/06-operators-and-sampling/) | Which mutations Angelo makes, and why |

## Status

Angelo is in development. The core works and has tests. The interface can change before
version 1.0.

## Contributing and support

Pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md).

If Angelo is useful to you, [sponsorship](https://github.com/sponsors/kvandeh) pays for the
work. A report of a wrong verdict is also very useful. See
[Support](https://angelo.kcvdh.com/support/).

## Author

Written and owned by **Kieran van der Heijde**.
[LinkedIn](https://www.linkedin.com/in/kcvdh) · [GitHub](https://github.com/kvandeh)
