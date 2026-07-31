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

## Getting Started

First download the Angelo package.

```bash
pip install -i https://test.pypi.org/simple/ angelo
```

Then run it from a project with **python and pytest on your PATH** and a **passing test
suite**:

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

=== mutation report ===
    killed: 46
  survived: 28
     score: 62.2% (46/74 detected)
```

The survivors are the output worth reading. `exec` is resumable: interrupt it and run it
again to carry on, or delete `.angelo/` to start fresh.

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

## What mutation testing is

Coverage tells you a line ran. It does not tell you whether anything checked the result.

```python
def is_adult(age):
    return age >= 18
```

Change `>=` to `>` and the function now rejects eighteen year olds. If your tests still
pass, the boundary was never tested, even at 100% coverage.

Each deliberate change is a **mutant**. A mutant is **killed** when a test fails, and it
**survived** when every test passed. Every survivor is a bug you could ship without a
single test objecting.

For everything else, see the [documentation](https://angelo.kcvdh.com/).
