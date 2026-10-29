<h1 align="center">Angelo</h1>

<p align="center">
  <em>Python mutation testing is slow and expensive, but what if it wasn't?</em>
</p>

<p align="center">
  <a href="https://github.com/kvandeh/angelo/releases">
    <img alt="Release" src="https://img.shields.io/github/v/release/kvandeh/angelo?display_name=tag&sort=semver&color=orange">
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

Angelo measures how good your tests actually are. It breaks your code on purpose, one
small change at a time, and reports which changes your test suite failed to notice. It is
a single Rust binary that drives your ordinary pytest suite.

## The problem it solves

Coverage tells you a line ran. It does not tell you whether anything checked the result.

```python
def is_adult(age):
    return age >= 18
```

Change `>=` to `>` and the function now rejects eighteen year olds. If your tests still
pass, the boundary was never tested, even at 100% coverage. Angelo finds that line and
tells you about it.

Each deliberate change is a **mutant**. A mutant is **killed** when a test fails, and it
**survived** when every test passed. Survivors are the output worth reading: each one is a
bug you could ship without a single test objecting.

## Quick start

```bash
cd your-project
angelo init      # detect the layout, write angelo.conf
angelo exec      # enumerate mutants, then run them
```

```
16:20:04 INFO   enumerated 74 mutants across 3 files
16:20:09 INFO   baseline green in 4.8s, timeout 14.5s for a whole-suite run, from its own tests for a selected one
16:20:09 INFO   9 mutants sit on lines no test executes, survived without a single run
16:20:09 INFO   running 17 batches on 16 workers, covering tests only
  mutants   [############################--------]  78%  51/65  detected 34  survived 17  ~11s left

survivors (changes your tests never noticed):
  calculator.py:4 0.21 -> 1.21
  calculator.py:24 < -> <=

Mutate what the branch added, and fail if too much of it survived.

```yaml
- uses: actions/checkout@v4
  with:
    fetch-depth: 0
- run: angelo exec --diff-base $GITHUB_BASE_REF --fail-under 80
```

`--fail-under` exits 1 with one line: `score 62.2% is below --fail-under 80.0%`. A run that
could not be scored fails it too, because a tool that could not measure must never report
success. See the [Quick Start](https://angelo.kcvdh.com/quick-start/).

**The report is output; everything above it is commentary.** The report goes to stdout and
prints at every verbosity, so a script can read it. The timestamped lines and the bar go to
stderr, and `--verbosity` turns them down.

```bash
angelo exec --verbosity warn      # error | warn | info | debug | trace
```

The default is `info`, or `warn` when the `CI` environment variable is set, because in CI
nobody is watching it scroll past. `RUST_LOG` works too. Off a terminal the bar disappears
by itself, so a piped run stays byte-clean.

## Reports you can keep

A run that only ever reached a terminal cannot be attached to a pull request.

```bash
angelo exec --html-report angelo.html   # one self-contained file, no network
angelo exec --report angelo.json        # the mutation-testing-report schema
```

The JSON is not a shape Angelo invented. It is
[`mutation-testing-report-schema`](https://github.com/stryker-mutator/mutation-testing-elements/tree/master/packages/report-schema)
version 2, the format StrykerJS, Stryker.NET, Stryker4s and muttest all write, so Stryker's
existing viewers and dashboards read an Angelo run without a converter. See
[reports](https://angelo.kcvdh.com/07-reports/).

Survivors also go straight to **SonarQube**, so they land on the dashboard your team already
reads:

```bash
angelo exec --sonar-report angelo-sonar.json   # sonar.externalIssuesReportPaths
```

No plugin, no Java, nothing installed on the server, and it works on SonarQube Cloud. See
[SonarQube](https://angelo.kcvdh.com/09-sonarqube/).

A self-hosted SonarQube can go further. The
[plugin](https://angelo.kcvdh.com/10-sonar-plugin/) in `integrations/sonarqube/` publishes the
mutation score as a **real SonarQube metric**, with history and a quality-gate condition on
the number. Third-party plugins do not run on SonarQube Cloud, so that half is Server and
Community Build only.

## What mutation testing is

Most of a mutation run is not testing. On one selected test, roughly 95% of the 327 ms
goes to starting Python and importing pytest. Angelo attacks that from four directions:

| Technique | What it skips |
| --- | --- |
| [Batch mutating](https://angelo.kcvdh.com/01-batch-mutating/) | Runs. Several mutants share one test run. |
| [Test selection](https://angelo.kcvdh.com/02-test-selection/) | Tests. Only the ones that reach the mutant. |
| [Warm workers](https://angelo.kcvdh.com/03-warm-workers/) | Startup. One pytest process stays alive. |
| `--diff`, `--diff-base` and `--sample` | Mutants. Only changed lines, only what a branch adds, or a random sample. |

On a synthetic 200 mutant project with a two second suite, these take **48.1s down to
4.5s** with an identical score.

> [!WARNING]
> That figure is an upper bound. It comes from a project with independent functions and no
> timeouts, which is the best case for all three techniques. Measured against the real
> [click](https://github.com/pallets/click) codebase, where about 7% of mutants hang until
> the timeout, the same techniques bought almost nothing, because waiting for timeouts
> dominates. The [benchmarks](https://angelo.kcvdh.com/05-benchmarks/) page reports
> both results.

## The rule everything obeys

Batching, test selection and warm workers are **speed features only**. None may change a
verdict.

This is enforced rather than promised. [`scripts/verdict-matrix.sh`](scripts/verdict-matrix.sh)
runs eight configurations against the same project and fails the build if any of them
disagrees about the score. It runs in CI on every push. A speedup that changes a result is
treated as a bug.

## Requirements

- **python and pytest** on your PATH.
- **A passing test suite.** Angelo refuses to run against a red one, because a failing test
  cannot tell you anything about a planted fault.
- **`pip install coverage`**, strongly recommended. Coverage unlocks batching and test
  selection, which provide most of the speed. Without it Angelo still works, one mutant per
  run.

Runs natively on Windows, Linux and macOS. Unlike mutmut, it does not need `fork()`.

## Installing

Not on PyPI proper yet. Releases go to **TestPyPI** while the pipeline is being proven:

```bash
pip install --index-url https://test.pypi.org/simple/ angelo
```

The wheel carries the compiled binary rather than any Python, so `pip` puts `angelo` on
your PATH and nothing gets imported. Wheels are built for Windows x86-64, Linux x86-64 and
Apple Silicon; anything else builds from source:

```bash
git clone https://github.com/kvandeh/angelo.git
cd angelo/Angelo
cargo build --release   # Angelo/target/release/angelo
```

## Documentation

Full documentation at **[angelo.kcvdh.com](https://angelo.kcvdh.com/)**.
Each page is written as a short paper: abstract, background, method, result, limits.

| Page | Covers |
| --- | --- |
| [Run it locally](https://angelo.kcvdh.com/quick-start/) | First run, reading the report, configuration, troubleshooting |
| [Integrations](https://angelo.kcvdh.com/integrations/) | CI, pull requests, SonarQube, PyPI |
| [Batch mutating](https://angelo.kcvdh.com/01-batch-mutating/) | The conflict rule and why it is sound |
| [Test selection](https://angelo.kcvdh.com/02-test-selection/) | Coverage into pytest node ids |
| [Warm workers](https://angelo.kcvdh.com/03-warm-workers/) | The `fork()` substitute, and why subinterpreters are not one |
| [Architecture](https://angelo.kcvdh.com/04-architecture/) | Module map and design rules |
| [Benchmarks](https://angelo.kcvdh.com/05-benchmarks/) | Every measurement, including the disappointing ones |
| [Operators and sampling](https://angelo.kcvdh.com/06-operators-and-sampling/) | Which operators the evidence supports, where not to apply them, and how to cap the pool |

## Status

In development. The core works and is tested, but expect rough edges and no stability
guarantee yet.

## Contributing and support

Pull requests are welcome. [CONTRIBUTING.md](CONTRIBUTING.md) is one paragraph.

If Angelo is useful to you, [sponsorship](https://github.com/sponsors/kvandeh) pays for the
time it takes. Reporting a wrong verdict is worth just as much: see
[Support](https://angelo.kcvdh.com/support/).

## Author

Written and owned by **Kieran van der Heijde**.
[LinkedIn](https://www.linkedin.com/in/kcvdh) · [GitHub](https://github.com/kvandeh)
