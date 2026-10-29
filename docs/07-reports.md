# Reports

!!! abstract "Summary"
    Before, the result of a run was only on the terminal. Now Angelo writes two files: one
    self-contained HTML page for people, and one JSON document in an existing schema for
    other tools. Neither file calculates its own score. Both get the score from `Summary`,
    the same source as stdout.

## Background

Three types of reader use a mutation run. stdout helps only the first.

| Reader | Needs | stdout gives |
| --- | --- | --- |
| The developer who ran it | The survivors, now | Yes |
| The reviewer of a pull request | A file to open | Nothing |
| A dashboard, a gate, a different tool | Structured data | Nothing |

A survivor list that is longer than one screen is difficult to read in a terminal. Also,
stdout cannot group survivors by file and sort the files.

## The JSON format is a standard

A standard for all languages already exists. Thus Angelo does not make a new format.

The format is **`mutation-testing-report-schema`** version 2, at
`http://stryker-mutator.io/report.schema.json`. Its source is
[stryker-mutator/mutation-testing-elements](https://github.com/stryker-mutator/mutation-testing-elements/blob/master/packages/report-schema/src/mutation-testing-report-schema.json).
StrykerJS, Stryker.NET, Stryker4s and muttest all write it. The
[mutation-testing-elements](https://stryker-mutator.io/docs/mutation-testing-elements/)
viewer and the Stryker dashboard both read it.

```bash
angelo exec --report angelo.json
```

### The statuses match

Each Angelo status has an equivalent in the schema.

| Angelo | Schema | Reason |
| --- | --- | --- |
| `killed` | `Killed` | |
| `survived`, a test ran it | `Survived` | A test executes the line but checks nothing about it. |
| `survived`, no test ran it | `NoCoverage` | No test executes the line. |
| `timeout` | `Timeout` | Detected in both: the behaviour changed. |
| `error` | `RuntimeError` | |
| `untestable` | `Ignored` | Its only tests failed already in the baseline. |
| `pending` | `Pending` | Angelo made it, but did not run it yet. |

**The schema splits `survived` in two**, and this split is useful. For a mutant that no
test covers, write a test. For a mutant that a test ran but did not detect, add an
assertion. These are different jobs, and the terminal did not show the difference.

The score also matches. The schema defines the score as `detected / valid`. Detected is
killed plus timeout. Valid does not include errors and ignored mutants. This is exactly
[`Summary::score`](04-architecture.md). Thus there is one calculation, and the file and the
terminal always agree.

### The cost

The schema needs **the full original text of each mutated file** in the report. Without it,
the viewer cannot show a mutant in its context. Thus a large repository gives a large file.
The demo project, with 74 mutants in 3 files, gives 29 KB.

## The HTML format is ours

One file. It needs no network and no framework. The model is Lighthouse: a score at the
top, and the details below it.

```bash
angelo exec --html-report angelo.html
```

| Section | Contents |
| --- | --- |
| **Score** | The number, in a red, amber or green band |
| **Worth checking** | All the problems that the run found, before the numbers |
| **Counts** | Each verdict, and whether it is in the score |
| **Per file** | Mutants, survivors and score for each file, **worst first** |
| **Survivors** | Grouped by file, `original → replacement`. Uncovered survivors have a mark. |
| **Run facts** | Baseline, workers, batch size, coverage, selection, warm workers |

The CSS is in the file. The document has no external references. Thus it opens from a CI
artifact and on a computer with no network. It uses the light or dark theme of the reader.

**"Worth checking" is the most important section.** A run can show a score that looks
correct but is not reliable. Examples: a red baseline, a sample, an `exclude` pattern that
matched nothing, or coverage that matched no mutant. Before, these warnings went past in the
terminal. Now they are above the score, in the file that a person attaches to a pull
request.

## Reports are output, never a verdict

A report cannot change the verdicts of a run.

- Angelo writes the reports **from the database**, not from the run. Thus `--init-only`, a
  continued `exec` and a run with no pending mutants all write a report.
- `scripts/verdict-matrix.sh` must still agree with itself when the report flags are on.
- A report is **not a gate**. Use it with `--fail-under`, never as a replacement. The exit
  code stops a broken run. A report only shows information.

This last rule is important. **A run where all mutants have the status `error` has no
score.** A tool that reads only the survivors sees an empty list and shows "no problems".
This is the characteristic failure of this tool in the opposite direction. For this
reason, `error` becomes `RuntimeError` and is not removed. For the same reason, the HTML
says "no score" in words and does not show a number.

## Configuration

You can put each flag in `angelo.conf`. The flag has priority over the file.

```toml
report = "angelo.json"        # empty means off, which is the default
html_report = "angelo.html"   # empty means off, which is the default
```

## Limits

- **The HTML has no date and time.** A calendar date needs a dependency or fifteen lines of
  calendar calculation. The modification time of the file already gives this information.
- **The HTML does not show the source code.** It lists the survivors without their context.
  The JSON contains the source of each file, and the Stryker viewer shows it.
- **The JSON gets larger with the repository**, because the schema needs the sources.
- **No mutation score goes to SonarQube Cloud.** `--sonar-report` shows survivors in all
  SonarQube editions. See [SonarQube](09-sonarqube.md). But the import makes issues, not
  measures. The percentage needs [the plugin](10-sonar-plugin.md), and third-party plugins
  run only on a self-hosted server.
