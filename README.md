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
  <b><a href="https://angelo.kcvdh.com/quick-start/">Quick Start</a></b> ·
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
enumerated 74 mutants across 3 files
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

`exec` is resumable. Interrupt it and run it again to pick up where it stopped, or delete
`.angelo/` to start fresh. Results live in `.angelo/angelo.db`, a plain SQLite file you can
open with anything.

## Why it is fast

Most of a mutation run is not testing. On one selected test, roughly 95% of the 327 ms
goes to starting Python and importing pytest. Angelo attacks that from four directions:

| Technique | What it skips |
| --- | --- |
| [Batch mutating](https://angelo.kcvdh.com/01-batch-mutating/) | Runs. Several mutants share one test run. |
| [Test selection](https://angelo.kcvdh.com/02-test-selection/) | Tests. Only the ones that reach the mutant. |
| [Warm workers](https://angelo.kcvdh.com/03-warm-workers/) | Startup. One pytest process stays alive. |
| `--diff`, `--diff-base`, `--sample` and `exclude` | Mutants. Only changed lines, only what a branch adds, a random sample, or everything but the code you did not write. |

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

## Building

Not yet published to PyPI. For now:

```bash
git clone https://github.com/kvandeh/angelo.git
cd angelo
cargo build --release   # target/release/angelo
```

## Documentation

Full documentation at **[angelo.kcvdh.com](https://angelo.kcvdh.com/)**.
Each page is written as a short paper: abstract, background, method, result, limits.

| Page | Covers |
| --- | --- |
| [Quick Start](https://angelo.kcvdh.com/quick-start/) | First run, reading the report, configuration, troubleshooting |
| [Batch mutating](https://angelo.kcvdh.com/01-batch-mutating/) | The conflict rule and why it is sound |
| [Test selection](https://angelo.kcvdh.com/02-test-selection/) | Coverage into pytest node ids |
| [Warm workers](https://angelo.kcvdh.com/03-warm-workers/) | The `fork()` substitute, and why subinterpreters are not one |
| [Architecture](https://angelo.kcvdh.com/04-architecture/) | Module map and design rules |
| [Benchmarks](https://angelo.kcvdh.com/05-benchmarks/) | Every measurement, including the disappointing ones |
| [Operators and sampling](https://angelo.kcvdh.com/06-operators-and-sampling/) | What gets mutated, and how to cap the pool |

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
