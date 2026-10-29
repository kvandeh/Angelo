# Angelo

**Fast mutation testing for Python.** Angelo is one Rust binary. It makes small changes to
your code on purpose, runs your pytest suite, and reports each change that no test detected.

Coverage tells you that a line ran. It does not tell you that a test checked the result.

```python
def is_adult(age):
    return age >= 18
```

Change `>=` to `>`. The function now rejects people who are 18. If your tests still pass,
no test checks the boundary, even at 100% coverage. Angelo finds this line and reports it.

## Install

```bash
pip install angelo-mut
```

The package is `angelo-mut`. The command is `angelo`.

The wheel contains only the compiled binary. It contains no Python code, so any Python
interpreter can install it. There are wheels for Windows x86-64, Linux x86-64 and macOS
Apple Silicon. There is **no source package**, on purpose. On a different platform, pip
stops immediately. It does not compile Rust for five minutes.

## Use

```bash
cd your-project
angelo init      # finds your layout and writes angelo.conf
angelo exec      # makes the mutants and runs them
```

You need **Python and pytest** on your PATH. We recommend `pip install coverage`. Batching
and test selection need it, and they give most of the speed.

Angelo runs on Windows, Linux and macOS. It does not need `fork()`.

## Documentation

The full documentation, with CI examples and the SonarQube integration, is at
**<https://angelo.kcvdh.com/>**.

Source code and issues: <https://github.com/kvandeh/angelo>
