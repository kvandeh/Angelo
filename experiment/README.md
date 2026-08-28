# Angelo 0.3.0 real-world corpus

50 real, openly-licensed Python projects run through `angelo exec --sample 1000`
(via `sample = 1000` in `angelo.conf`), collected as a dataset: what angelo can
run cleanly, what it can't, and what the mutation scores look like on code
nobody wrote to be mutation-tested.

Written for a future reader — human or LLM — who was not in the room for the
run. Nothing here should require re-deriving from the scripts to understand.

## What's in this directory

| Path | Holds |
| --- | --- |
| `manifest.json` | the 50 target repos: name, GitHub `owner/repo`, clone URL, detected license, whether it was pre-cloned |
| `results.jsonl` | one JSON object per repo, appended as each finishes — the primary dataset |
| `reports/<name>/` | `report.html` (self-contained), `stryker-report.json` (mutation-testing-report schema), `sonar-issues.json` (SonarQube generic issue import format) — for every repo that got far enough to produce one |
| `logs/<name>.*.log` | raw stdout/stderr from each stage (clone, venv setup, pytest --co sanity check, `angelo init`, `angelo exec`) — read these when a `results.jsonl` row needs explaining |
| `state/progress.json` | live progress marker, written continuously while the run is in flight; stale once the run finishes |
| `scripts/run-experiment.ps1` | the orchestrator. Run from PowerShell, never Git Bash — CLAUDE.md warns Git Bash mangles path-like arguments passed to the `angelo` binary, and this script passes plenty |
| `scripts/progress-watch.ps1` | read-only live dashboard, meant to sit in its own terminal window |
| `.venv/` | one shared venv with `angelo==0.3.0` pinned from TestPyPI — this is the *angelo binary*, not a target project's dependencies |

Cloned repos live in `../extra/<name>/` (gitignored, outside this directory),
each with its own `.venv/` holding that project's own test dependencies. This
follows the layout `scripts/setup-extra.sh` already established for this repo;
`extra/` was not duplicated into `experiment/` to avoid re-cloning repos that
were already there.

## Method

For each repo in `manifest.json`, in order:

1. **Clone** (`git clone --depth 1`) if not already present under `extra/`.
   Record the commit SHA actually checked out — a shallow clone still pins one.
2. **License check** happened once, up front, via the GitHub API
   (`license.spdx_id`, following redirects for renamed/transferred repos), not
   per-run. All 50 carry an OSI-style open license; the exact string angelo's
   maintainer's GitHub query returned is in `manifest.json`. A few show a
   compound value (`"MIT/Apache-2.0"`) because GitHub's detector reported
   `NOASSERTION` for a dual-licensed project and the actual license was
   confirmed by reading the project's own LICENSE file.
3. **venv setup**: create (or repair, if a pre-existing venv had no working
   `pip`) a virtualenv under `extra/<name>/.venv`, install `pytest` and
   `coverage`, then attempt `pip install -e .[tests]`, falling back to
   `.[test]`, then plain `-e .`, then a non-editable install. Whichever
   succeeded is recorded (`editable_install_ok`).
4. **Collect sanity check**: `python -m pytest -q -x --co`. If this fails,
   angelo is never invoked — the row is marked `collect_failed` and that's the
   whole story for that repo (a project that can't collect can't be scored,
   and running angelo against it would just reproduce the same failure with
   extra steps). This was expected going in for `django`, which drives its
   tests through its own runner rather than pytest.
5. **`angelo init`**, unmodified — whatever paths and defaults it detects for
   that project are what ran. Nothing here overrides angelo's own layout
   detection.
6. **Patch four keys** in the generated `angelo.conf`: `sample = 1000`,
   `report`, `html_report`, `sonar_report` (absolute paths into
   `experiment/reports/<name>/`). Nothing else in the generated config was
   touched — batching, test selection, warm workers, timeout_factor all run at
   angelo's own defaults.
7. **`angelo exec`**, capped at a **45-minute wall-clock harness timeout**
   (separate from angelo's own internal per-mutant timeout math). A repo that
   hits this is recorded as `timed_out_by_harness` with whatever partial
   `.angelo/db` state it left behind — this is a statement about that
   project's suite speed at 1000 mutants, not a crash.
8. **Parse the stdout summary** angelo prints (`killed`/`survived`/`timeout`/
   `error`/`untestable` counts, the score line) into the result row.

Every stage after clone runs inside the target repo's own venv — `angelo`
itself is one binary, shared across all 50 runs, version-pinned via
`pip install --index-url https://test.pypi.org/simple/ angelo==0.3.0`.

## Reading `results.jsonl`

One line per repo, in manifest order. Fields:

- `name`, `repo`, `url`, `license`, `commit_sha` — what was run
- `status` — one of `ok`, `clone_failed`, `collect_failed`, `init_failed`,
  `timed_out_by_harness`
- `editable_install_ok`, `collects_cleanly` — booleans for the two setup gates
- `killed`, `survived`, `timeout`, `error`, `untestable` — angelo's own verdict
  counts (see CLAUDE.md's **Statuses** section for what each means; `timeout`
  here counts as *detected*, same as `killed`)
- `score_percent`, `scored_total` — `(killed + timeout) / scored_total * 100`;
  absent when nothing was scorable
- `sample_requested` — always 1000; the *actual* pool sampled may be smaller
  when the project has fewer than 1000 mutable tokens, which `scored_total`
  will show
- `wall_seconds` — clock-on-the-wall time for the whole per-repo pipeline
  stage that ran `angelo exec`, not counting clone/venv setup
- `html_report_written` — whether `report.html` actually landed; a `status`
  of `ok` with this `false` means angelo exited zero but wrote nothing, which
  would itself be worth flagging as an angelo bug rather than a corpus fact

A repo missing from `results.jsonl` entirely means the run hadn't reached it
yet when this was read — check `state/progress.json`.

## Known shape of the data, going in

- `error` and `untestable` sit **outside** the score by design (see CLAUDE.md
  **Statuses**) — a high `error` count on a repo is not a low score, it's a
  sign that many splices didn't parse or the harness couldn't judge them
  fairly, and it's printed separately for exactly that reason.
- Several repos were picked as deliberately hard cases and are expected to
  struggle: `django` (no pytest runner), `pandas` and `cryptography` (heavy
  compiled-extension surface, likely to hit the 45-minute cap), `pytest`
  itself (angelo mutating the tool it depends on to run).
- `workers` in every generated `angelo.conf` reflects this machine's logical
  core count at the time `angelo init` ran, not a fixed number chosen for the
  experiment.
- Two sampled runs of the same repo are not comparable to each other (see
  CLAUDE.md **Sampling** — the 1000-mutant draw is reseeded from the clock
  every run). This corpus holds exactly one draw per repo.
