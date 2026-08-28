# Corpus run remediation board

The 2026-08-28 corpus run scored **7 of 50** repos. This board holds every distinct
cause, grouped, and is updated as each one lands. One row per cause, not per repo —
36 "failures" collapse into eight causes, and most repos carry more than one.

Status: `todo` -> `doing` -> `done` (verified against a real repo) -> `wontfix`.

## The mandate

> Angelo must work on any repo where pytest works.

Causes A1 and A2 are Angelo bugs and break that promise for real users, not just for
this experiment. They come first. Everything under H is the harness around the
experiment and cannot affect a user of the tool.

## Angelo (ships to users)

| ID | Cause | Repos hit | Status |
| --- | --- | --- | --- |
| A1 | Bare `python` in `test_command` resolves to angelo's **own sibling** interpreter, not the one on PATH. Windows `CreateProcess` searches the calling exe's directory first, so an angelo installed in its own venv (pipx, uv tool, a shared tools venv) runs a python that has no pytest. | six, bottle, flake8, sqlalchemy, falcon, pyflakes | done |
| A2 | That failure reports as `Error: reading .angelo\baseline-junit.xml`. `run_baseline` treats exit 1 as a red suite, never captures stderr, and then reads the junit unconditionally. The real message (`No module named pytest`) is discarded. Baseline phase logs nothing even at `--verbosity trace`. | all of A1 | done |
| A3 | `angelo init` refuses when `angelo.conf` exists, with no way to regenerate. Callers that check only "does the file exist" silently proceed on a stale config. | joblib, + config drift across the corpus | done |

## Harness (experiment only)

| ID | Cause | Repos hit | Status |
| --- | --- | --- | --- |
| H1 | Test deps never installed. `pip install -e .[tests]` **exits 0 with a warning** when the extra does not exist, so the fallback chain always stopped at the first try. Compounded by PEP 735 `[dependency-groups]`, which `.[extra]` syntax cannot reach at all. | 24 | done |
| H2 | Collect gate uses `-x` and no path scope: one unimportable optional module discards a repo that collects thousands of tests, and it collects non-suite trees like `scripts/tests/`. | textual (2515 collected), rich (494), trio (342), redis-py (237), black (166), jsonschema (73), itsdangerous (66), pyparsing (31), fastapi, django | done |
| H3 | `addopts` requires plugins that were never installed. | uvicorn (`-n`), tqdm (`--asyncio-mode`), gunicorn (`--cov`), filelock, textual | done |
| H4 | Python 3.14 has no wheels for several targets, and `--depth 1` breaks setuptools_scm so every package versions as `0.1.dev1` — which fails pytest's own `minversion` check. | pyyaml, cryptography, pandas, pydantic, hypothesis, pytest, tornado | done |
| H5 | `status: ok` is set from the harness timeout alone, never from angelo's exit code, so 7 failed runs were filed as successes. | 7 | done |
| H6 | `Set-ConfigValue` writes UTF-8 **with BOM** (PS 5.1). A BOM ahead of `paths` makes that key parse as `﻿paths` and silently fall back to the default. | all | done |
| H7 | An `-Only <name>` re-run destroyed `results.jsonl`, 172 logs and 12 report dirs. No backup, and `experiment/` was untracked. | dataset | done |
| H8 | `$proc.Kill($true)` is .NET Core only. On Windows PowerShell 5.1 it throws into an empty `catch{}`, so the 45-minute timeout silently did nothing — one angelo ran an hour past its deadline with ~190 orphaned workers still multiplying. | joblib | done |
| H9 | `Start-Process -PassThru` leaves `ExitCode` `$null` unless the handle is held, so the new exit-code check read every finished run as a failure. | all | done |
| H10 | The `py` launcher lists interpreters from the registry, including installs that have been deleted. Asking for 3.12 failed and fell through to 3.14 with no warning — the version pin looked applied and was not. | all | done |

## Out of scope

| ID | Cause | Status |
| --- | --- | --- |
| X1 | django drives tests through its own runner, not pytest. Outside the mandate: pytest does not work on it either. | wontfix |

## Verification

Each fix is verified against a repo that actually exhibited the cause, not against a
fixture written to pass.

| Cause | Verified against | Result |
| --- | --- | --- |
| A1 | six, bottle, flake8, with angelo.exe beside a pytest-less python.exe | **pass** — all three score; before, all three died on the missing report |
| A2 | an interpreter that genuinely has no pytest | **pass** — names the interpreter and quotes `No module named pytest`; before, `reading .angelo\baseline-junit.xml` |
| A3 | a project with a stale conf | **pass** — `init` refuses, `init --force` regenerates |
| H1 | attrs, jinja, urllib3 (PEP 735 groups) | pending |
| H2 | textual, rich | pending |
| H3 | uvicorn, gunicorn | pending |
| H4 | pytest, pyyaml | pending |

### A2, before and after

```
before:  Error: reading .angelo\baseline-junit.xml
         Caused by: The system cannot find the file specified. (os error 2)

after:   Error: angelo ran the baseline suite but pytest wrote no report to
         .angelo\baseline-junit.xml. pytest exited 1, so the command most likely
         never reached pytest: check that test_command in angelo.conf names an
         interpreter with pytest installed. Angelo used <path>.
         <path>: No module named pytest
```
