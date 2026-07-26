# CLAUDE.md

angelo — fast mutation testing for Python; one Rust binary, shipped as a PyPI wheel.
Terminology (**batch mutating**, **mutation conflict**) is defined in README.md — keep
code and README consistent.

## Working with Kieran

- Python-fluent, learning Rust *through this project*. Idiomatic Rust — structs + impls,
  enums + match, derive macros, `?` — never tangled cleverness. When a construct is
  genuinely new territory (lifetimes beyond elision, async, trait objects, `Arc`), pause
  and explain it with a Python analogy before piling more on top.
- Comments only where the code is weird — say why it is weird, briefly what it does if
  needed. Otherwise make the code clear instead. No comments that restate code.
- Halfway-long beats short: no `unwrap()` in logic paths, `anyhow::Context` on fallible
  boundaries, handle the weird cases. Kieran's rule: shortcuts cost double later.
- Every dependency must defend itself. Current ten: clap/serde/toml/anyhow (plumbing),
  turso + tokio (DB, locked decision), ruff_python_parser/_ast/_text_size (parser),
  roxmltree (junit XML). walkdir/tempfile/serde_json were cut by hand-rolling small
  versions (recursive copy, Drop cleanup, newline-joined lists).
- Data files over string blobs: SQL lives in src/db/schema.sql (`include_str!`). The
  operator table in mutate.rs is a `macro_rules!` so the lookup and its test list are
  generated from the same lines and cannot drift.
- Docs in Kieran's register: short definition-style sentences, bold key terms. No
  academic wordiness.
- Neighbours worth checking before inventing: cargo-mutants (same approach — copy tree,
  patch, run), mutest-rs (batching via conflict graph, validates ours), mutagen
  (schemata; its const/pattern limits are why we skipped it).

## Locked decisions

- Config file is **`angelo.conf`**, TOML on the inside (renamed from angelo.toml
  2026-07-26). The `toml` crate does not care about the extension.
- CLI: `init` writes `angelo.conf`; `exec` enumerates mutants into the DB then runs them
  (`--workers N`, `--init-only`). Re-running `exec` resumes `pending` rows; fresh run =
  delete `.angelo/`. Config is `#[serde(default)]`, so an angelo.conf written by an
  older build still loads and new fields take their defaults — add fields freely.
- Batching (implemented): conflict = **same test case covers both mutants** — Kieran's
  correction, we measure the tests, not source structure. Per-test coverage comes from
  the baseline run wrapped in coverage.py (`dynamic_context = test_function`, rcfile +
  data file under `.angelo/`); turso reads coverage's SQLite file directly. Mutant
  classes: Tested (batchable by disjoint covering sets), ImportOnly (runs alone),
  Untested (survives with no run, `execution_id NULL`). Red batches attribute directly:
  failed test kills the one member it covers; unattributable outcomes (timeout, crash,
  unexplained failure) bisect. No coverage installed or non-default test command →
  batches of 1, everything still works. First-fit composer in batch.rs, `batch_size`
  in config (default 8).
- DB: turso 0.7 (async API) quarantined in src/db.rs behind a current-thread tokio
  runtime + `block_on`; everything else is sync. `mutant` and `execution` are separate
  tables; a green batch attaches many mutants to one execution row.
- Mutants: token-level byte splices via ruff_python_parser
  (`parse_module(source)?.tokens()`; token types in `ruff_python_ast::token`).
  Syntax-breaking splices are acceptable noise — they exit as `error`, outside the score.
- Operator set matches mutmut's (measured: **63 angelo vs 61 mutmut** on one file, was
  ~60%). 36 one-to-one token swaps in the `operators!` macro, plus numbers (`n+1`, any
  base), strings (XX-wrap + case flips, docstrings skipped), unary `not`/`~` removal, and
  string-method mirrors (only after a `Dot`). A `for` loop's `in` is skipped —
  `for x not in y` is always a syntax error. NOT implemented (needs AST rewriting, not
  splices): arg removal/None-ing, dict kwarg renames, lambda bodies, `a = None`,
  match-case dropping. `mutate::replacements` returns a Vec: one token can yield several.
- Sampling: `--sample N` / `sample` in config. **DELETES the overflow rows** — the sample
  IS the study, so the score is an estimate over a random draw from the whole pool, not
  the first N. Fisher-Yates over ids with a hand-rolled xorshift seeded by pool size, so
  re-runs sample identically (reproducibility over unpredictability). Fresh enumeration
  only; resuming keeps the existing pool.
- Execution: per-worker temp copies (hand-rolled, cleaned by Drop); multi-file patching
  via PatchedFiles (restore-on-Drop); pytest spawned with junit XML, killed on timeout.
  `std::thread::scope` + AtomicUsize work index + mpsc; the main thread owns DB writes.
- Statuses: `killed` (exit 1) / `survived` (exit 0) / `timeout` / `error` (other exits,
  incl. 5 = no tests). Score = (killed + timeout) / (killed + timeout + survived).
- Test selection (implemented): a run executes only the pytest node ids covering its
  batch, and a single-mutant run adds `-x`. Node ids come from the baseline junit report
  (`classname`/`name`), resolved against disk because a dotted classname does not say
  where the module ends and classes begin. Anything unresolvable, or any ImportOnly
  member, falls back to the whole suite — too many tests is slow, too few invents
  survivors. `test_selection` in config. Measured: selection and batching compete (a
  batch selects the union of its members' tests) but stack to 7.7x on a 2s suite.
- Diff mode (implemented): `--diff [REV]`, default HEAD. `git diff --unified=0 --relative`
  parsed for added-line ranges; mutants off those lines are never inserted. Filtering
  happens at enumeration, so `.angelo/` must be deleted to widen scope afterwards.
- Name: **angelo** (renamed from magneto 2026-07-26). `angelo` on PyPI is a dead 2021
  turtle-graphics toy (v0.0.1, one release); `angelo` on crates.io is FREE. Kieran:
  "name isn't that important" — settle the PyPI suffix at publish time.
- Warm workers (implemented, default on): angelo's stand-in for `fork()`. Each worker
  keeps one pytest process alive (src/runner/worker.py, `include_str!`ed and written into
  the copy) and feeds it JSON lines; between runs the driver drops every module whose
  `__file__` is under the copy root, so mutated source is re-imported while pytest stays
  loaded. Replies are prefixed `##angelo##` because pytest writes progress to the same
  stdout. Any timeout, crash, or unparseable reply retires the worker and the mutant
  falls back to a fresh subprocess — warm running can only change the clock. Recycles
  every `warm_recycle_after` runs (default 50). Only for `python -m pytest ...` commands
  (`warm::hostable`). Measured 2.5x unbatched; full stack 48.1s -> 4.5s on a 2s suite.
  **Do NOT purge `__main__`** — the driver is `__main__` and lives in the copy; pdb's
  `import __main__` breaks (cost an INTERNALERROR debugging session).
- Subinterpreters (PEP 734, `concurrent.interpreters`) were measured and REJECTED as a
  fork substitute: creating one is 15ms but importing pytest inside it is 131ms, same as
  cold, because each gets its own `sys.modules`. Do not revisit without new evidence.
- Roadmap: more operators, maturin wiring, conda.

## Docs

- `docs/` — one note per idea, each as abstract → background → method → result → limits.
  Short sentences, mermaid diagrams, tables over paragraphs. Kieran's rule: "big paragraph
  = scary, diagram + clear short explainers = good". Update the relevant note when a
  feature or number changes; README stays a front page and links out.
- Docs site is **Zensical** (`zensical.toml`, the Material for MkDocs team's Rust-core
  successor; Material is EOL 2026-11-05). `zensical build --clean` → `site/`. Mermaid
  renders via the `pymdownx.superfences` custom fence. `docs/index.md` is the landing
  page, and every page must appear in the `nav` — a test enforces that.
- Three workflows only: **lint-and-test** (lint on every push, tests + verdict-matrix on
  PR or master/main/develop, ubuntu + windows), **docs** (push to master, builds and
  deploys to Pages), **release** (push to master; version from Cargo.toml, skips if the
  tag exists, ships `angelo.exe` + `src.zip`, body = the merge commit message).
  `scripts/bench-repo.sh` stays for local runs; it has no workflow.
- `tests/docs_are_true.rs` guards doc rot: documented flags must exist, config keys must
  be documented, nav must match `docs/`.
- `extra/` — gitignored shallow clones (click, flask, httpx, requests, fastapi, django)
  for real-world runs. **Each needs its own venv** (`scripts/setup-extra.sh`): against a
  global Python these projects hit pytest exit 3 because their pyproject.toml configures
  plugins that are not installed. django uses its own runner, not pytest, so angelo cannot
  drive it at all.
- Baseline failures are diagnosed by exit code in `pytest::diagnose_baseline` — exit 1 is
  "your tests fail", 3 is "plugin/config problem", 5 is "nothing collected". Only the
  last 40 lines of pytest output are shown; collection errors bury the real cause.

## Structure

Flat modules, one nested dir, like cargo-mutants. Types own their own behaviour —
if a `fn` takes an X and branches on it, it probably belongs on X.

- src/main.rs — clap definitions and dispatch, nothing else
- src/exec.rs — the exec workflow (enumerate → baseline → split → compose → run)
- src/config.rs — angelo.conf, file discovery, SKIP_DIRS
- src/mutate.rs — Mutant + Status, operator table, enumeration
- src/coverage.rs — Coverage: coverage.py wrapping, numbits, classify/attribute/select
- src/batch.rs — Batch (accepts/add) + first-fit composer
- src/diff.rs — ChangedLines: git hunk parsing + the changed-line filter
- src/pytest.rs — SuiteResult, Selection, TestCase (node ids) + the pytest process
- src/runner.rs — TestRunner spawns Worker per thread; WorkerCopy, PatchedFiles and
  WarmWorker all clean up via Drop
- src/warm.rs + src/runner/worker.py — the long-lived pytest host and its driver
- src/db.rs + src/db/schema.sql — turso, the only async file (also reads coverage.db)
- src/report.rs — Progress (live lines) + Summary (scoring, unit-tested)
- tests/end_to_end.rs — drives the real binary against throwaway Python projects
- demo/ — pytest project for manual runs

## Testing

- Unit tests live in `#[cfg(test)] mod tests` at the bottom of each module.
- `tests/end_to_end.rs` runs the built binary (`env!("CARGO_BIN_EXE_angelo")`) in a
  temp project: init, scoring, resume, --init-only, red baseline, no-sources. Needs
  python + pytest on PATH.
- Pure logic gets a unit test, not an end-to-end one. Learned the hard way: breaking
  `detected = killed + timeout` to `killed` passed every integration test, because the
  fixture has no timeouts. Summary::of exists so that arithmetic is directly testable.

## Commands

- `cargo build` / `cargo clippy --all-targets` / `cargo fmt` / `cargo test`
- Manual run: from `demo/`, `..\target\debug\angelo.exe init` then `exec`.

## Gotchas

- Windows first: no `fork()`; kill runaway pytest with `child.kill()`; drive the CLI from
  PowerShell (Git Bash mangles path-like args).
- pytest exit codes: 0 passed, 1 failures, 2–4 usage/internal error, 5 no tests.
- Editable installs shadow worker copies: pytest runs with the copy root and `src/` on
  PYTHONPATH so the mutated copy wins over `pip install -e` originals.
- A run where every mutant dies near-instantly means a broken test command, not a great
  suite (learned in the cosmic-ray study). Check `error` counts before trusting a score.
- **Stale bytecode fakes survivors.** A `.pyc` is reused when the source's
  `(mtime_seconds, size)` still match. Same-size splices (`+`->`-`, `*`->`/`) written in
  the same second as the previous one run the OLD bytecode, so the mutant survives for
  free. Fixed by `PYTHONDONTWRITEBYTECODE=1` in pytest.rs (worker copies never carry a
  `__pycache__` in, so never writing one is airtight). `PYTHONDONTWRITEBYTECODE` alone is
  NOT enough if a `.pyc` already exists — Python still reads it. Symptom: scores drift
  between runs, worse on fast suites and fast filesystems. Invisible on Windows with a
  slow suite (runs land >1s apart); Linux with a 0.2s suite gave 77/73/69/78 killed for
  identical work. Any future "warm process" or schemata work must re-check this.
