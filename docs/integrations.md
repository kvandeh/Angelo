# Integrations

Angelo already ran on your machine. This page puts its verdict somewhere else: a pipeline, a
pull request, a dashboard, another tool's viewer. If you have not run it yet, start with
[Run it locally](quick-start.md).

## Pick your route

| You want to | Go to |
| --- | --- |
| Have an AI agent wire this up for you | [Setting it up with an AI agent](ai-setup.md) |
| Install Angelo on a CI runner | [Install it in a pipeline](#install-it-in-a-pipeline) |
| Copy a working GitHub Actions job | [A complete job](#a-complete-github-actions-job) |
| Mutate only what a pull request changed | [Scope a run to a pull request](#scope-a-run-to-a-pull-request) |
| Block a merge on a bad score | [Gate the build](#gate-the-build) |
| Get survivors onto a SonarQube dashboard | [SonarQube](#sonarqube) |
| Feed the result to some other tool | [Reports other tools read](#reports-other-tools-read) |

## Install it in a pipeline

```bash
pip install --index-url https://test.pypi.org/simple/ angelo
```

The wheel holds a compiled binary and **no Python at all**, so the interpreter that installs
it is irrelevant and there is no build matrix to maintain. One install step covers every
Python version your suite runs on.

!!! warning "TestPyPI is a sandbox, not a mirror"
    That flag points pip at TestPyPI **instead of** PyPI, so anything else in the same
    command fails to resolve. Install Angelo on its own, or keep PyPI in the search:

    ```bash
    pip install --index-url https://test.pypi.org/simple/ --extra-index-url https://pypi.org/simple/ angelo
    ```

    TestPyPI makes no retention promises, so **pin a version** in anything that matters.

`pip install coverage` is close to required here even though it is optional locally. Without
it every mutant runs alone, which is roughly eight times slower, **and** a red baseline stops
the run outright instead of routing around itself. A job that installs Angelo and forgets
coverage is the one that mysteriously times out.

There are wheels for Windows x86-64, Linux x86-64 and Apple Silicon, and deliberately **no
sdist**. Any other runner fails with `No matching distribution found for angelo` rather than
compiling Rust for five minutes. The recipe that builds those wheels is in
[`integrations/pypi/`](https://github.com/kvandeh/angelo/tree/main/integrations/pypi).

## A complete GitHub Actions job

```yaml
name: Mutation testing
on: pull_request

jobs:
  mutants:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          # A shallow clone has no merge base, and --diff-base needs one.
          fetch-depth: 0

      - uses: actions/setup-python@v5
        with:
          python-version: "3.13"

      - name: Install the suite and Angelo
        run: |
          pip install -e ".[test]"
          pip install coverage
          pip install --index-url https://test.pypi.org/simple/ angelo

      - name: Mutate what this branch adds
        run: angelo exec --diff-base --fail-under 80 --html-report angelo.html

      - name: Keep the report either way
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: mutation-report
          path: angelo.html
```

`if: always()` on the upload is the load-bearing part. `--fail-under` exits 1 on a bad score,
and the report is most worth reading on exactly the run that failed.

## Scope a run to a pull request

`--diff` compares a revision against **your working tree**. That is the right question while
you are editing and the wrong one in CI, where a pushed branch has nothing uncommitted and
the answer is therefore nothing at all.

`--diff-base` compares against the **merge base** instead: the point where your branch left
the base. That is what the branch adds, however many commits it took, and whether or not the
base branch moved on since.

| Flag | Question it answers | What it compares |
| --- | --- | --- |
| `--diff [REV]` | What is different on this machine right now? | `REV` against the working tree |
| `--diff-base [REV]` | What does this branch add on top of `REV`? | `REV` against the branch, from where they last met |

Given no revision, `--diff-base` works one out: the branch the pull request targets, then
origin's own default branch, then `main` or `master`.

!!! warning "`fetch-depth: 0` is not optional"
    Checkouts fetch a single commit by default, and a shallow clone has no merge base to
    diff from. Angelo stops and tells you rather than quietly comparing something else,
    because the alternative is a confident score for lines you never wrote.

**A branch that changes no Python enumerates zero mutants.** Angelo says so and prints no
score, because zero mutants is zero information rather than a pass.

## Gate the build

Without a threshold `angelo exec` always exits 0, so a pull request can take the score from
80% to 30% and the workflow stays green. `--fail-under` is the gate.

```bash
angelo exec --fail-under 80    # exit 1 if the score comes in under 80%
```

It exits 1 and says which two numbers disagreed:

```
score 62.2% is below --fail-under 80.0%
```

`fail_under` in `angelo.conf` sets the same threshold for every run. The flag wins when both
are given. **0 means no threshold**, and that is the default.

A threshold has to be **earned**, so three runs fail it rather than passing by default.

| Situation | Exit | Why |
| --- | --- | --- |
| Score below the threshold | 1 | The point of the flag. |
| Every mutant `error`, so no score at all | 1 | A tool that could not measure must never report success, and this is what a broken test command looks like. |
| Mutants still `pending` | 1 | A partial score is not a score. The pending ones could all survive. |
| Zero mutants in scope | 0 | Nothing was measured, so there is nothing to judge. Angelo prints no score. |

The comparison is against the **raw ratio**, not the rounded percentage, so 4 kills out of 5
clears `--fail-under 80` rather than tripping over a rounding artefact.

### What an exit code means

| Exit | Means |
| --- | --- |
| 0 | The run finished, and either cleared its threshold or had none |
| 1, on stdout with the report | The score came in under `--fail-under` |
| 1, as an error on stderr | pytest never judged the code: a missing plugin, no tests collected, a red suite Angelo could not route around |

A threshold failure is a verdict rather than a crash, which is why the two land on different
streams. A script can tell them apart without parsing anything.

## SonarQube

**Third-party plugins do not run on SonarQube Cloud at all.** That one fact decides which
route you get, so settle it before reading a command.

| Your SonarQube | Route | What you get |
| --- | --- | --- |
| **Cloud** | `--sonar-report` only | Survivors as issues, on the right files and columns. **No mutation score** — no metric, no history, no gate on the number |
| **Server, or Community Build** | `--sonar-report`, and the plugin jar on top | The same issues, **plus** a real mutation-score metric with history and a quality-gate condition |

`--sonar-report` installs nothing on the server and works on both, so start there whichever
you run:

```bash
angelo exec --sonar-report angelo-sonar.json --fail-under 80
sonar-scanner -Dsonar.externalIssuesReportPaths=angelo-sonar.json
```

The plugin is an addition and never a replacement: it needs a jar dropped into a server you
administer, and there is no version of it Cloud will load. See
[the plugin](10-sonar-plugin.md).

Why the score cannot cross on its own: generic import creates **issues**, and a percentage is
a **measure**. There is no import path for measures. The full argument, the format, and the
four fields that fail silently are in [SonarQube](09-sonarqube.md).

!!! warning "Use `--sonar-report` alongside `--fail-under`, never instead of it"
    Only survivors become issues, so a run where every mutant errored uploads **nothing** and
    Sonar renders that as a clean bill of health. The exit code is what stops a broken run.
    The dashboard is only visibility.

## Reports other tools read

```bash
angelo exec --report angelo.json         # mutation-testing-report schema, version 2
angelo exec --html-report angelo.html    # one self-contained file, no network
angelo exec --sonar-report angelo-sonar.json
```

| File | Read by |
| --- | --- |
| `--report` | Stryker's viewer and dashboard, and anything else that speaks the schema StrykerJS, Stryker.NET, Stryker4s and muttest share |
| `--html-report` | A person. Attach it to the pull request; it needs no network and no server |
| `--sonar-report` | `sonar.externalIssuesReportPaths` |

To render the JSON as Stryker's own HTML viewer:

```bash
npx mutation-testing-elements angelo.json
```

All three are written **from the database rather than from the run**, so `--init-only`, a
resumed `exec` and a run with nothing pending all produce one. What each holds is in
[reports](07-reports.md).

The four keys a pipeline usually sets in `angelo.conf` instead of on the command line are
`fail_under`, `report`, `html_report` and `sonar_report`. A flag always wins over the file.
The full list of keys is on [Run it locally](quick-start.md#configuration).

## Verbosity in a pipeline

**The report is output; everything else is commentary.** The report goes to stdout and prints
at every verbosity, so a script can rely on it. Timestamped lines and the progress bar go to
stderr.

The default drops to **`warn` when the `CI` environment variable is set**, since in CI nobody
is watching it scroll past. Off a terminal the bar disappears by itself, and colour is off
whenever stdout is redirected, so a piped run stays byte-clean and greps correctly.

## When a pipeline goes wrong

**`No matching distribution found for angelo`.** No wheel for that runner, and no sdist on
purpose. Check the platform, or build from source.

**Every mutant is `error`.** The test command cannot run in the pipeline even though it runs
locally — usually a missing dependency or a plugin the runner does not have. This scores
nothing and, without `--fail-under`, exits 0 while looking fine. Read the `error` count.

**The score is 0.0% and every mutant survived without a run.** A pytest plugin in the
project's `addopts` took over the baseline: pytest-cov through `--cov`, or pytest-xdist
through `-n auto`. Both leave the per-test coverage map matching nothing, so every mutant
looks untested. Angelo warns, and the fix is in
[Setting it up with an AI agent](ai-setup.md#neutralising-a-plugin-in-addopts) — it is the
same fix whether a person or an agent is doing it.

**The run scores nothing on a docs-only branch.** Expected. Zero mutants in scope exits 0 and
prints no score, because there was nothing to measure.

**`--diff-base` stops with a message about the merge base.** The checkout was shallow. Add
`fetch-depth: 0`.

**SonarQube imported no issues and reported no error.** Almost always the file paths. Keys
must be relative and forward-slashed, and the scanner resolves them against its base
directory by literal match, so a miss is silently dropped. See
[SonarQube](09-sonarqube.md#four-things-that-fail-silently-if-they-are-wrong).
