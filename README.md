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

Angelo measures how good your tests actually are. It breaks your code on purpose, one
small change at a time, and reports which changes your test suite failed to notice. It is
a single Rust binary that drives your ordinary pytest suite.

## Getting Started

First download the Angelo package. *Currently only on test.pypi.org.*

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
enumerated 74 mutants across 3 files
running 17 batches on 8 workers, covering tests only

   killed: 46
 survived: 28
    score: 62.2% (46/74 detected)

survivors (changes your tests never noticed):
  calculator.py:31 >= -> >
  calculator.py:35 and -> or
  text.py:22 lower -> upper
```

The survivors are the output worth reading. `exec` is resumable: interrupt it and run it
again to carry on, or delete `.angelo/` to start fresh.

`pip install coverage` is optional and strongly recommended. It is what makes runs fast.

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
