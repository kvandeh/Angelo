# Angelo

Fast mutation testing for Python: one Rust binary that drives an ordinary pytest suite,
shipped as a PyPI wheel.

This file is the working contract for anyone changing Angelo, human or agent. The process
for landing a change is [CONTRIBUTING.md](CONTRIBUTING.md); this is how the code and the
prose are expected to look once it lands. Terminology — **batch mutating**, **mutation
conflict** — is defined in [README.md](README.md); keep the code and the README saying the
same thing.

## Quick Reference

* Speed features must never change a verdict; see [The rule everything obeys](#the-rule-everything-obeys).
* Write plain Rust: structs and impls, enums and match, derive, `?`; see [Write idiomatic Rust, not clever Rust](#write-idiomatic-rust-not-clever-rust).
* No `unwrap()` in a logic path, `anyhow::Context` on every fallible boundary; see [Handle the weird case](#handle-the-weird-case).
* If a function takes an `X` and branches on it, it belongs on `X`; see [Put behaviour on the type that owns the data](#put-behaviour-on-the-type-that-owns-the-data).
* Clean up with `Drop`, never with a call the caller has to remember; see [Clean up in Drop](#clean-up-in-drop).
* Every dependency must defend itself; see [Make each dependency defend itself](#make-each-dependency-defend-itself).
* SQL, templates and tables live in files, not in string literals; see [Keep data in data files](#keep-data-in-data-files).
* Comment the weird, not the obvious; see [Comment the weird, not the obvious](#comment-the-weird-not-the-obvious).
* Pure logic gets a unit test in the same file; see [Test the arithmetic, not just the workflow](#test-the-arithmetic-not-just-the-workflow).
* A run where every mutant dies instantly is a broken test command, and no score is not a pass; see [Never trust a fast run](#never-trust-a-fast-run).
* Bytecode caching silently fakes survivors; see [Never let Python reuse bytecode](#never-let-python-reuse-bytecode).
* Windows has no `fork()` and is the first-class platform; see [Windows first](#windows-first).
* Do not purge `__main__`, and do not revisit subinterpreters; see [Two traps that already cost a day](#two-traps-that-already-cost-a-day).
* Settled decisions, with the reasoning: [Settled decisions](#settled-decisions).
* Module map: [Where the code lives](#where-the-code-lives).

## The rule everything obeys

Batching, test selection and warm workers are **speed features only**. None of them may
change a verdict.

This is enforced, not promised. `scripts/verdict-matrix.sh` runs eight configurations
against the same project and fails the build when any of them disagrees about the score.
A speedup that changes a result is a bug, and it is reported as a bug even when the
speedup is large.

This tool's characteristic failure is **inventing a test gap that does not exist**. Every
other rule here exists downstream of that one.

## Write idiomatic Rust, not clever Rust

Structs with `impl` blocks, enums with `match`, derive macros, `?`. Nothing tangled.

```rust
pub enum Status {
    Killed,
    Survived,
    Timeout,
    Error,
}

impl Status {
    /// A timeout counts as detected: the mutant observably changed behaviour.
    pub fn is_detected(self) -> bool {
        matches!(self, Status::Killed | Status::Timeout)
    }
}
```

The reader may be fluent in Python and new to Rust. When a change genuinely needs a
construct beyond the basics — a lifetime past elision, async, a trait object, `Arc` —
explain it briefly in the pull request in terms of its Python equivalent, and do not stack
a second new construct on top of it in the same change.

## Handle the weird case

Halfway-long beats short. Shortcuts cost double later.

Do this:

```rust
let source = fs::read_to_string(&path)
    .with_context(|| format!("reading {}", path.display()))?;
```

Instead of:

```rust
// DO NOT DO THIS
let source = fs::read_to_string(&path).unwrap();
```

- **No `unwrap()` in a logic path.** In a `#[cfg(test)]` block it is fine.
- **`anyhow::Context` on every fallible boundary**: files, processes, git, the database.
  An error the user reads must say what Angelo was trying to do.
- Prefer `let ... else` and early return over nesting.

## Put behaviour on the type that owns the data

If a function takes an `X` and branches on it, it probably belongs on `X`.

Do this:

```rust
impl SuiteResult {
    pub fn status(&self) -> Status {
        match self {
            SuiteResult::TimedOut => Status::Timeout,
            SuiteResult::Finished(0) => Status::Survived,
            SuiteResult::Finished(1) => Status::Killed,
            SuiteResult::Finished(_) => Status::Error,
        }
    }
}
```

Instead of a free `fn status_of(result: &SuiteResult) -> Status` parked in another module.

There is no `utils.rs` and there will not be one.

## Clean up in Drop

Temporary state is released by the type that created it, so an early return or a panic
cannot leak it.

```rust
impl Drop for WorkerCopy {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
```

`WorkerCopy`, `PatchedFiles` and `WarmWorker` all work this way. `Drop` cannot return an
error, so when a failed cleanup would corrupt a later run, the later run must detect it:
`PatchedFiles` restores the original bytes on drop, and the next batch's original-bytes
check turns a failed restore into an `error` verdict rather than a wrong one.

## Make each dependency defend itself

Ten, and each one earns its place:

| Dependency | For |
| --- | --- |
| clap, serde, toml, anyhow | plumbing |
| turso, tokio | the database, a settled decision |
| ruff_python_parser, ruff_python_ast, ruff_text_size | the Python parser |
| roxmltree | junit XML |

`walkdir`, `tempfile` and `serde_json` were removed and hand-rolled instead: a recursive
copy, `Drop` cleanup, and newline-joined lists. Reach for that outcome first. A new
dependency needs a sentence in the pull request explaining what it does that a few lines
here could not.

Dev-only scripts under `scripts/` are exempt. They are not in the binary and not in the
wheel.

## Keep data in data files

Do this:

```rust
const SCHEMA: &str = include_str!("db/schema.sql");
```

Instead of a multi-line SQL literal in the middle of a Rust function.

The same rule generates the operator table: `mutate.rs` declares it through a
`macro_rules!` so the lookup and its test list are produced from the same lines and cannot
drift apart. `src/runner/worker.py` is `include_str!`'d and written into the worker copy
for the same reason — it is Python, so it lives in a `.py` file where an editor can lint it.

## Comment the weird, not the obvious

A comment explains **why** something is strange. If the code needs a comment to say what it
does, make the code clearer instead.

Do this:

```rust
// Splice back to front so earlier byte offsets stay valid.
ordered.sort_by_key(|m| std::cmp::Reverse(m.byte_start));
```

Instead of:

```rust
// DO NOT DO THIS
// Sort the mutants by byte_start in reverse order.
ordered.sort_by_key(|m| std::cmp::Reverse(m.byte_start));
```

## Test the arithmetic, not just the workflow

Unit tests live in `#[cfg(test)] mod tests` at the bottom of the module they test.
`tests/end_to_end.rs` drives the real binary (`env!("CARGO_BIN_EXE_angelo")`) against
throwaway Python projects: init, scoring, resume, `--init-only`, a red baseline, no sources.
It needs python and pytest on PATH.

**Pure logic gets a unit test, not an end-to-end one.** This was learned the hard way:
breaking `detected = killed + timeout` down to `killed` passed every integration test,
because the fixture happens to produce no timeouts. `Summary::of` exists so that the
arithmetic can be tested directly.

`tests/docs_are_true.rs` guards documentation rot: every documented flag must exist, every
config key must be documented, and the nav must match `docs/`.

## Never trust a fast run

A run where every mutant dies almost instantly means a **broken test command**, not an
excellent suite. Check the `error` count before believing any score. This was learned
during the cosmic-ray study and it is the reason the summary prints a warning about error
counts at all.

The same lesson decides what `--fail-under` does with a run it could not score: it fails
it. A tool that could not measure must never report success, and an all-`error` run is
exactly what a broken test command produces.

Related: pytest exit codes are 0 passed, 1 failures, 2 to 4 usage or internal error, 5
nothing collected. `pytest::diagnose_baseline` turns each into a sentence, because a
collection error buries its real cause under hundreds of tracebacks — only the last 40
lines are shown.

## Never let Python reuse bytecode

A `.pyc` is reused when the source's `(mtime_seconds, size)` still match. A same-size
splice — `+` to `-`, `*` to `/` — written in the same second as the previous one therefore
runs the **old bytecode**, and the mutant survives for free.

- Fixed by `PYTHONDONTWRITEBYTECODE=1` in `pytest.rs`. Worker copies never carry a
  `__pycache__` in, so never writing one is airtight.
- The environment variable **alone is not enough** where a `.pyc` already exists. Python
  still reads it.
- Symptom: scores drift between identical runs. Worse on a fast suite and a fast
  filesystem. Invisible on Windows with a slow suite, where runs land more than a second
  apart; Linux with a 0.2 s suite gave 77, 73, 69 and 78 kills for identical work.

**Any future warm-process or schemata work must re-check this.**

## Windows first

- **No `fork()`.** Warm workers exist because of this.
- Kill a runaway pytest with `child.kill()`.
- Drive the CLI from PowerShell. Git Bash mangles path-like arguments.
- An editable install shadows a worker copy, so pytest runs with the copy root and its
  `src/` on `PYTHONPATH`; the mutated copy has to win over a `pip install -e` original.

## Two traps that already cost a day

**Do not purge `__main__`.** The warm worker's driver *is* `__main__` and it lives inside
the copy it purges. Dropping it breaks any stdlib that does `import __main__` — pdb does,
through rlcompleter — and the symptom is an INTERNALERROR far from the cause.

**Do not revisit subinterpreters** without new evidence. PEP 734
(`concurrent.interpreters`) was measured as a `fork()` substitute and rejected: creating
one costs 15 ms, but importing pytest inside it costs 131 ms, the same as a cold start,
because each interpreter gets its own `sys.modules`.

## Settled decisions

**Config.** `angelo.conf`, TOML on the inside. The `toml` crate does not care about the
extension. `Config` is `#[serde(default)]`, so a file written by an older build still
loads and new fields take their defaults — add fields freely.

**CLI.** `init` writes `angelo.conf`. `exec` enumerates mutants into the database and then
runs them (`--workers N`, `--init-only`, `--diff`, `--diff-base`, `--sample`,
`--fail-under`). Re-running `exec` resumes `pending` rows; a fresh run means deleting
`.angelo/`. `main` returns `ExitCode` rather than `()`, because the threshold needs an exit
code that is not an error.

**Database.** turso 0.7. Its API is async, so it is quarantined inside `src/db.rs` behind a
current-thread tokio runtime and `block_on`; everything else in the codebase is sync.
`mutant` and `execution` are separate tables, because one green batch attaches many mutants
to a single execution row. `src/db.rs` also reads coverage.py's own SQLite file directly.

**Mutants.** Token-level byte splices via `ruff_python_parser`: `parse_module(source)?
.tokens()`, token kinds from `ruff_python_ast::token`. Splices that break the syntax are
accepted noise — they exit as `error` and sit outside the score.

**Operators.** Measured at **63 Angelo against 61 mutmut** on one file, up from about 60%.
36 one-to-one token swaps in the `operators!` macro, plus numbers (`n+1`, any base),
strings (XX-wrapping and case flips, docstrings skipped), unary `not` and `~` removal, and
string-method mirrors, which only apply after a `Dot`. A `for` loop's `in` is skipped
because `for x not in y` is always a syntax error. `mutate::replacements` returns a `Vec`:
one token can yield several mutants. Not implemented, because they need AST rewriting
rather than splices: argument removal or None-ing, dict kwarg renames, lambda bodies,
`a = None`, match-case dropping.

**Batching.** A conflict is **the same test case covering both mutants**. The measurement
is over tests, not over source structure. Per-test coverage comes from the baseline run
wrapped in coverage.py (`dynamic_context = test_function`, rcfile and data file under
`.angelo/`). Mutants classify as Tested (batchable when their covering sets are disjoint),
ImportOnly (runs alone) or Untested (survives with no run at all, `execution_id NULL`). A
red batch attributes directly: a failed test kills the one member it covers. Anything
unattributable — a timeout, a crash, a failure no member explains — bisects. With no
coverage installed, or a non-default test command, batches are size 1 and everything still
works. First-fit composer in `batch.rs`, `batch_size` in config, default 8.

**Test selection.** A run executes only the pytest node ids covering its batch, and a
single-mutant run adds `-x`. Node ids come from the baseline junit report
(`classname`/`name`), resolved against disk because a dotted classname does not say where
the module ends and the classes begin. Anything unresolvable, and any ImportOnly member,
falls back to the whole suite: too many tests is merely slow, too few invents survivors.
Selection and batching compete — a batch selects the union of its members' tests — but they
stack to 7.7x on a 2 s suite.

**Timeouts.** `Budget` in `pytest.rs` owns the arithmetic. A run that selected its tests is
charged `their baseline time * timeout_factor + 5s`; a whole-suite run is charged the whole
suite's duration on the same formula, which is what every run used to pay. Per-test times
come from the baseline junit report's `time` attribute, summed across a node id so
parametrised cases add up. The 5 s floor is a constant and must stay one: it covers
interpreter start, imports and collection, none of which appear in a junit time, and the
cold subprocess that any warm failure falls back to. **A timeout counts as detected, so a
budget that is too tight invents kills rather than merely running slowly.** The verdict
matrix fixture carries a deliberately hanging mutant for exactly this reason.

**Warm workers.** Angelo's stand-in for `fork()`, default on. Each worker keeps one pytest
process alive (`src/runner/worker.py`) and feeds it JSON lines. Between runs the driver
drops every module whose `__file__` sits under the copy root, so mutated source is
re-imported while pytest stays loaded. Replies carry a `##angelo##` prefix because pytest
writes its own progress to the same stdout. Any timeout, crash or unparseable reply retires
the worker and the mutant falls back to a fresh subprocess — warm running may change the
clock and nothing else. Recycles every `warm_recycle_after` runs, default 50. Only for
`python -m pytest ...` commands (`warm::hostable`). Measured 2.5x unbatched; the full stack
takes 48.1 s to 4.5 s on a 2 s suite.

**Execution.** Per-worker temporary copies of the project, hand-rolled and cleaned by
`Drop`. Multi-file patching through `PatchedFiles`, restored on `Drop`. pytest is spawned
with junit XML and killed on timeout. `std::thread::scope` with an `AtomicUsize` work index
and an mpsc channel; the main thread owns every database write.

**Statuses.** `killed` (exit 1), `survived` (exit 0), `timeout`, `error` (any other exit,
including 5, nothing collected), `untestable` (no run could judge it fairly). Score is
`(killed + timeout) / (killed + timeout + survived)`, so `error` and `untestable` sit
outside it. `STATUSES` in `mutate.rs` is the one list `parse` and its test both read.

**Red baselines.** A red baseline warns and keeps going. Exit 1 means pytest judged the
code and some of it failed, which is a normal state for a real suite; exits 2 to 5 mean it
never judged anything, so there is no duration to measure and no junit report to read, and
those still bail. The reason the old rule existed is kept: an already-failing test fails
again under a mutant, pytest exits 1, and exit 1 is `killed`. So a mutant is judgeable only
when `Coverage::gets_a_fair_trial` says its selection can name its own tests and avoid
every already-red node id. The rest are `untestable`. This **needs coverage and
`test_selection`** — without them every run is the whole red suite and every mutant scores
`killed`, so that combination bails with a message naming the missing piece rather than the
failing tests. Untested mutants are split off *first*, because a mutant no test executes
survives without a run whether the baseline is green or red.

**Progress.** Over 1000 mutants, or with `show_loading = true`, the line-per-mutant output
collapses into one `\r`-redrawn bar. No dependency: a progress-bar crate buys nothing over
a carriage return. `error` lines still print on their own line and the bar redraws
underneath, because a broken test command is the loudest thing this tool has to say. The
remaining-time estimate is a linear extrapolation and says `~`, since batching settles
mutants in clumps and a red batch bisects into more runs. `bar_line` is pure and
unit-tested; the drawing is not.

**Output.** The survivor list prints **above** the report, so the score is the last line
on screen rather than five hundred survivors up. Verdicts carry colour through `Paint` in
`report.rs`: detected green, survived yellow, error red, untestable dim, the score bold,
the bar's filled run green. Hand-rolled ANSI, because `std::io::IsTerminal` makes the
check free and four escape codes are cheaper than a dependency. **Colour reaches a
terminal and nothing else** — `colour_wanted` is off when stdout is redirected or
`NO_COLOR` is set to any value, empty included, because `verdict-matrix.sh` greps those
very lines and so does anyone piping a run into `grep`. The decision is unit-tested; the
escape codes are not. Two things that bite: a label is padded *before* it is painted, or
the column counts escapes, and the bar is measured before it is painted, or `erase`
overruns the line.

**Allocation.** Four hot spots were fixed and two famous suggestions were rejected. Fixed:
`Lines` in `mutate.rs` carries a cursor, so a file is scanned once rather than once per
mutant; `Coverage::build` looks a file up once per row, not once per covered line;
`Coverage::covering` borrows the covering set, and only `batch.rs` clones one;
`Mutant::splice_into` uses `String::replace_range` rather than rebuilding the whole file
once per batch member. Rejected: **rayon**, because `TestRunner::run_all` already fans out
with `std::thread::scope` and the work is a subprocess, not arithmetic; **`swap_remove`**,
because there is no order-preserving `Vec::remove` anywhere to apply it to. Measured, and
the number is small on purpose: see
[benchmarks](docs/05-benchmarks.md#the-allocation-pass).

**Thresholds.** `--fail-under PERCENT`, or `fail_under` in config, with the flag winning. 0
is off and is the default. `Summary::gate` owns the decision and returns a `Gate`, so the
arithmetic is unit-tested rather than inferred from an exit code. A threshold has to be
**earned**: an all-`error` run has no score and fails it, and a run with `pending` rows
fails it too, because a partial score is not a score. A **zero-mutant pool passes** and
prints nothing — a docs-only `--diff-base` branch lands there, and there is no measurement
to judge. The comparison is `detected * 100 >= threshold * scored`, the raw ratio rather
than the rounded percentage, so 4 of 5 clears `--fail-under 80` instead of tripping over
how it prints. Both a threshold failure and a red baseline exit 1, and they stay
distinguishable: the threshold prints its one line on stdout with the report, while the red
baseline comes out of `anyhow` on stderr.

**Sampling.** `--sample N`, or `sample` in config. It **deletes the overflow rows**,
because the sample *is* the study: the score is an estimate over a random draw from the
whole pool, not a census of the first N. Fisher-Yates over the ids with a hand-rolled
xorshift seeded by the pool size, so a re-run samples identically — reproducibility beats
unpredictability here. Fresh enumeration only; resuming keeps the existing pool.

**Diff mode.** Two flags answering two questions, held by `Scope` in `diff.rs`. `--diff
[REV]`, default HEAD, is `git diff REV`: the revision against the working tree, which is
what you want while editing. `--diff-base [REV]` is `git diff REV...HEAD`, the **three-dot**
form, which is merge-base semantics: what this branch adds on top of REV, collapsed across
however many commits it took, ignoring whatever the base gained meanwhile. Only that one
suits a pull request, because a pushed branch has nothing uncommitted for two dots to find.
Given no revision it works one out: `$GITHUB_BASE_REF`, then origin's own HEAD, then
`origin/main`, `main`, `master`. clap refuses both flags at once, because silently
preferring one is a way to score the wrong lines. A **shallow clone** has no merge base, so
`--diff-base` stops and names `fetch-depth: 0` rather than falling back to two dots and
reporting a confident score for the wrong lines. `git diff --unified=0 --relative` is parsed
for added-line ranges, and mutants off those lines are never inserted. Filtering happens at
enumeration, so `.angelo/` has to be deleted before the scope can widen again. An empty pool
prints no score at all: zero mutants is zero information, not a pass.

**Name.** Angelo, renamed from magneto on 2026-07-26. `angelo` on crates.io is free; on
PyPI it is a dead 2021 turtle-graphics toy with one release. On **TestPyPI** the plain name
is free, so the playground publishes as `angelo` and the real PyPI suffix still gets
settled at publish time, not before.

**Packaging.** `pyproject.toml`, maturin with `bindings = "bin"`. The wheel carries
`angelo.exe` in its scripts directory and no Python whatsoever, so the tag is
`py3-none-<platform>` and the interpreter that installed it is irrelevant. The version is
`dynamic` and read from Cargo.toml, which stays the one place a release number lives.
Wheels only, no sdist: an sdist would make an unsupported platform quietly compile Rust
for five minutes instead of saying it has no wheel. PyPI metadata lives in
`pyproject.toml`, not in Cargo.toml, so the two files do not compete.

**Publishing.** TestPyPI, over OIDC **trusted publishing**, so no token exists in the
repository at all. The match is on three names together — repository, workflow file name,
environment — and renaming `release.yml` breaks the upload until the publisher on TestPyPI
is renamed to match.

It lives **inside `release.yml`** rather than in a workflow of its own, and that is not a
tidiness preference. A GitHub Release cut with the default `GITHUB_TOKEN` never starts
another workflow, so a separate publishing workflow could not be triggered by the release
it exists to publish; it would sit silent while looking wired up. One workflow, one
trigger, one version.

`version` reads Cargo.toml and decides two things: whether to cut a release (no, if the tag
exists) and whether to build wheels (yes on a manual run either way, so a broken publisher
can be retried without a version bump). `release` and `testpypi` then run **beside** each
other, so a sandbox being down never blocks shipping a binary that already built. The
wheels job uploads `target/release/angelo.exe` too, because maturin ran a plain
`cargo build --release` and the release should not build the same binary twice. No
`skip-existing`; a version TestPyPI already holds should fail loudly and be bumped.

**Roadmap.** More operators, aarch64 and Intel-macOS wheels, real PyPI, conda.

## Where the code lives

Flat modules with one nested directory, the same shape as cargo-mutants.

| File | Holds |
| --- | --- |
| `src/main.rs` | clap definitions and dispatch, nothing else |
| `src/exec.rs` | the exec workflow: enumerate, baseline, split, compose, run |
| `src/config.rs` | angelo.conf, file discovery, `SKIP_DIRS` |
| `src/mutate.rs` | `Mutant` and `Status`, the operator table, enumeration |
| `src/coverage.rs` | `Coverage`: coverage.py wrapping, numbits, classify/attribute/select |
| `src/batch.rs` | `Batch` (`accepts`/`add`) and the first-fit composer |
| `src/diff.rs` | `Scope` (which lines are in play) and `ChangedLines`: git hunk parsing and the changed-line filter |
| `src/pytest.rs` | `SuiteResult`, `Selection`, `TestCase` node ids, the pytest process |
| `src/runner.rs` | `TestRunner` spawns a `Worker` per thread; `WorkerCopy`, `PatchedFiles`, `WarmWorker` |
| `src/warm.rs` + `src/runner/worker.py` | the long-lived pytest host and its driver |
| `src/db.rs` + `src/db/schema.sql` | turso, the only async file, and the schema |
| `src/report.rs` | `Progress` (live lines or one redrawn bar) and `Summary` (scoring, unit-tested) |
| `tests/end_to_end.rs` | the real binary against throwaway Python projects |
| `pyproject.toml` | maturin's wheel recipe and the PyPI metadata |
| `demo/` | a pytest project for manual runs |

## Write documentation in the project's register

Short definition-style sentences. **Bold key terms.** Tables instead of paragraphs. No
academic wordiness. A big paragraph reads as intimidating; a diagram with short explainers
reads as clear.

- `docs/` holds one note per idea, each shaped as abstract, background, method, result,
  limits. Update the relevant note in the same pull request when a feature or a number
  changes. The README stays a front page and links out.
- Mermaid diagrams render through the `pymdownx.superfences` custom fence.
- The docs site is **Zensical** (`zensical.toml`), the Material for MkDocs team's
  Rust-core successor; Material itself is end-of-life on 2026-11-05. Build with
  `zensical build --clean` into `site/`. `docs/index.md` is the landing page, and every
  page must appear in the nav — a test enforces it.
- Report the disappointing measurements too. The benchmarks page records where the
  optimisations bought nothing, and that is the reason to trust the rest of it.

## Three workflows, and no more

| Workflow | Runs |
| --- | --- |
| **lint-and-test** | lint on every push; tests and the verdict matrix on a pull request or on master/main/develop, ubuntu and windows |
| **docs** | on push to master: builds and deploys to Pages |
| **release** | on push to master, or by hand: version from Cargo.toml, skipped if the tag exists, builds a wheel per platform, ships `angelo.exe` and `src.zip`, uploads to TestPyPI, body is the merge commit message |

`scripts/bench-repo.sh` stays a local tool and has no workflow.

## Commands

```bash
cargo build
cargo clippy --all-targets
cargo fmt
cargo test
```

Manual run, from `demo/`:

```powershell
..\target\debug\angelo.exe init
..\target\debug\angelo.exe exec
```

Real-world runs use `extra/`, which holds gitignored shallow clones of click, flask, httpx,
requests, fastapi and django. **Each needs its own virtualenv** (`scripts/setup-extra.sh`):
against a global Python these projects hit pytest exit 3, because their `pyproject.toml`
configures plugins that are not installed. django uses its own test runner rather than
pytest, so Angelo cannot drive it at all.

## Neighbours worth checking before inventing

- **cargo-mutants** — the same approach: copy the tree, patch, run.
- **mutest-rs** — batching through a conflict graph, which validates ours.
- **mutagen** — schemata; its const and pattern limits are why that path was skipped.
- **mutmut** — the operator set to match, and the `fork()` plus schemata design Angelo
  competes with at about 1.2x per mutant. It cannot run on Windows.
