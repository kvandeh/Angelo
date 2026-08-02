# Angelo

**Fast mutation testing for Python.** One Rust binary that drives your ordinary pytest suite,
breaks your code on purpose one small change at a time, and reports which changes your tests
failed to notice.

Coverage tells you a line ran. It does not tell you whether anything checked the result.

```python
def is_adult(age):
    return age >= 18
```

Change `>=` to `>` and the function rejects eighteen year olds. If your tests still pass, the
boundary was never tested — at 100% coverage. Angelo finds that line and tells you about it.

## Install

```bash
pip install --index-url https://test.pypi.org/simple/ angelo
```

Releases go to **TestPyPI** while the pipeline is being proven. TestPyPI makes no retention
promises, so pin a version in anything that matters.

The wheel carries the compiled binary and no Python at all, so `pip` puts `angelo` on your
PATH and nothing here is ever imported. The interpreter that installs it is irrelevant.
Wheels are built for Windows x86-64, Linux x86-64 and Apple Silicon; there is deliberately
**no sdist**, so an unsupported platform fails immediately rather than compiling Rust for
five minutes.

## Use

```bash
cd your-project
angelo init      # detect the layout, write angelo.conf
angelo exec      # enumerate mutants, then run them
```

Needs **python and pytest** on your PATH. `pip install coverage` is strongly recommended: it
unlocks batching and test selection, which provide most of the speed.

Runs natively on Windows, Linux and macOS. Unlike mutmut, it does not need `fork()`.

## Documentation

Full documentation, including CI recipes and the SonarQube integration, at
**<https://angelo.kcvdh.com/>**.

Source and issues: <https://github.com/kvandeh/angelo>
