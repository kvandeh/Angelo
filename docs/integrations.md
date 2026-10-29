# Integrations

This page tells you how to send the result of a run to other places. These places are a
pipeline, a pull request, a dashboard, or a different tool. If you did not run Angelo yet,
start with [Run it locally](quick-start.md).

## Select your route

| You want to | Go to |
| --- | --- |
| Let an AI agent do the setup | [Setting it up with an AI agent](ai-setup.md) |
| Install Angelo on a CI runner | [Install it in a pipeline](#install-it-in-a-pipeline) |
| Copy a GitHub Actions job that works | [A complete job](#a-complete-github-actions-job) |
| Mutate only the changes in a pull request | [Scope a run to a pull request](#scope-a-run-to-a-pull-request) |
| Stop a merge when the score is low | [Gate the build](#gate-the-build) |
| Show survivors in SonarQube | [SonarQube](#sonarqube) |
| Send the result to a different tool | [Reports other tools read](#reports-other-tools-read) |

## Install it in a pipeline

```bash
pip install angelo-mut coverage
```

The wheel contains a compiled binary and **no Python code**. Thus the Python version of
the runner is not important. One installation step works for all Python versions of your
suite.

In a pipeline, also install `coverage`. Without it:

- Each mutant runs alone. This is approximately eight times slower.
- A suite with failing tests stops the run. Angelo cannot select tests around the failures.

There are wheels for Windows x86-64, Linux x86-64 and macOS Apple Silicon. There is **no
source package**, on purpose. On a different runner, pip stops with
`No matching distribution found for angelo-mut`. It does not compile Rust for five
minutes. The recipe for the wheels is in
[`integrations/pypi/`](https://github.com/kvandeh/angelo/tree/main/integrations/pypi).

!!! tip "Pin a version"
    For a stable pipeline, pin the version, for example `pip install "angelo-mut==0.3.1"`.
    A new version can find new survivors.

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
          pip install angelo-mut coverage

      - name: Mutate what this branch adds
        run: angelo exec --diff-base --fail-under 80 --html-report angelo.html

      - name: Keep the report either way
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: mutation-report
          path: angelo.html
```

The upload step needs `if: always()`. When the score is low, `--fail-under` exits with
code 1, and the job stops. Without `if: always()`, the report of the failed run is lost.
That is the report that you most need.

## Scope a run to a pull request

| Flag | Question | Comparison |
| --- | --- | --- |
| `--diff [REV]` | What changed on this computer? | `REV` against the working tree |
| `--diff-base [REV]` | What does this branch add to `REV`? | `REV` against the branch, from the merge base |

Use `--diff` while you edit code. Do not use it in CI. In CI, the branch has no
uncommitted changes, so `--diff` finds nothing.

Use `--diff-base` in CI. It compares from the **merge base**, the commit where your branch
started. It finds all the changes that the branch adds, in all its commits. Changes on the
base branch after that point do not count.

If you do not give a revision, `--diff-base` uses the first one that it finds:

1. The target branch of the pull request (`$GITHUB_BASE_REF`)
2. The default branch of `origin`
3. `origin/main`, `main`, then `master`

!!! warning "You must set `fetch-depth: 0`"
    By default, a checkout gets only one commit. Such a shallow clone has no merge base.
    Angelo then stops and tells you the cause. It does not compare a different thing,
    because that gives a score for lines that you did not write.

**A branch that changes no Python code has zero mutants.** Angelo tells you this and
prints no score. Zero mutants give no information, so this is not a pass.

## Gate the build

Without a threshold, `angelo exec` always exits with code 0. A pull request can decrease
the score from 80% to 30%, and the workflow stays green. Use `--fail-under` to prevent
this.

```bash
angelo exec --fail-under 80    # exit 1 if the score is less than 80%
```

Angelo exits with code 1 and prints the two numbers:

```
score 62.2% is below --fail-under 80.0%
```

`fail_under` in `angelo.conf` sets the same threshold for all runs. If you use the flag
and the key, the flag has priority. **0 means no threshold.** This is the default.

The run must **earn** a pass. Thus some runs fail even without a low score:

| Situation | Exit code | Reason |
| --- | --- | --- |
| The score is less than the threshold | 1 | This is the purpose of the flag. |
| All mutants are `error`, so there is no score | 1 | A tool that did not measure must not report success. A broken test command gives this result. |
| Some mutants are still `pending` | 1 | A partial score is not a score. All the pending mutants can survive. |
| There are zero mutants | 0 | Angelo measured nothing, so it has nothing to judge. It prints no score. |

Angelo compares the **exact ratio**, not the rounded percentage. Thus 4 kills from 5
mutants passes `--fail-under 80`.

### Exit codes

| Exit code | Meaning |
| --- | --- |
| 0 | The run is complete. The score is equal to or more than the threshold, or there is no threshold. |
| 1, with a line on stdout after the report | The score is less than `--fail-under`. |
| 1, with an error on stderr | pytest did not test the code: a plugin is missing, pytest found no tests, or Angelo could not select tests around a red suite. |

A low score is a verdict. A failed baseline is an error. Thus they go to different
streams, and a script can tell them apart without a parser.

## SonarQube

**SonarQube Cloud does not run third-party plugins.** This decides your route.

| Your SonarQube | Route | Result |
| --- | --- | --- |
| **Cloud** | `--sonar-report` only | Survivors show as issues, on the correct files and columns. **There is no mutation score**: no metric, no history, no gate on the number. |
| **Server or Community Build** | `--sonar-report`, and also the plugin | The same issues, **and** a mutation-score metric with history and a quality-gate condition. |

`--sonar-report` needs no installation on the server, and it works on both. Start with it:

```bash
angelo exec --sonar-report angelo-sonar.json --fail-under 80
sonar-scanner -Dsonar.externalIssuesReportPaths=angelo-sonar.json
```

The plugin adds to `--sonar-report`. It does not replace it. You must put the plugin jar on
a server that you control. See [the plugin](10-sonar-plugin.md).

The issue import cannot carry the score. The import makes **issues**, and a percentage is a
**measure**. SonarQube has no import for measures. For the full explanation and the format,
see [SonarQube](09-sonarqube.md).

!!! warning "Always use `--fail-under` with `--sonar-report`"
    Only survivors become issues. If all mutants are `error`, the file contains **no
    issues**, and SonarQube shows a clean result. The exit code stops a broken run. The
    dashboard only shows information.

## Reports other tools read

```bash
angelo exec --report angelo.json         # mutation-testing-report schema, version 2
angelo exec --html-report angelo.html    # one self-contained file, it needs no network
angelo exec --sonar-report angelo-sonar.json
```

| File | Read by |
| --- | --- |
| `--report` | The Stryker viewer and dashboard, and all tools that read the schema of StrykerJS, Stryker.NET, Stryker4s and muttest |
| `--html-report` | A person. Attach it to the pull request. It needs no network and no server. |
| `--sonar-report` | `sonar.externalIssuesReportPaths` |

To show the JSON in the Stryker HTML viewer:

```bash
npx mutation-testing-elements angelo.json
```

Angelo writes all three files **from the database**, not from the current run. Thus
`--init-only`, a continued `exec`, and a run with no pending mutants all write a report.
For the contents of each file, see [Reports](07-reports.md).

A pipeline usually sets four keys in `angelo.conf` and not on the command line:
`fail_under`, `report`, `html_report` and `sonar_report`. A flag always has priority over
the file. For the full list of keys, see
[Run it locally](quick-start.md#configuration).

## Log output in a pipeline

The report goes to stdout. Angelo always prints it, so a script can use it. The log lines
and the progress bar go to stderr.

When the `CI` environment variable is set, the default log level is **`warn`**. When the
output is not a terminal, Angelo shows no progress bar. When stdout is redirected, Angelo
uses no colours. Thus `grep` gets plain text.

## Troubleshooting in a pipeline

| Symptom | Cause and fix |
| --- | --- |
| `No matching distribution found for angelo-mut` | There is no wheel for that runner. Use a different runner, or build from source. |
| All mutants are `error` | The test command does not run on the runner, but it runs on your computer. Usually a dependency or a plugin is missing. Without `--fail-under`, this run exits with code 0. Look at the `error` count. |
| The score is 0.0% and all mutants survived without a run | A pytest plugin in `addopts` controlled the baseline: pytest-cov with `--cov`, or pytest-xdist with `-n auto`. Then the coverage data matches no mutant. Angelo gives a warning. For the fix, see [Neutralising a plugin](ai-setup.md#neutralising-a-plugin-in-addopts). |
| A branch with only documentation has no score | This is correct. Zero mutants exit with code 0 and print no score. |
| `--diff-base` stops with a message about the merge base | The checkout is shallow. Add `fetch-depth: 0`. |
| SonarQube imported no issues and gave no error | The file paths are usually the cause. They must be relative and use forward slashes. SonarQube ignores a path that does not match exactly. See [SonarQube](09-sonarqube.md#four-things-that-fail-silently-if-they-are-wrong). |
