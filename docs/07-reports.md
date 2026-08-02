# Reports

**Abstract.** A run's result used to die with the terminal. Angelo now writes two files: one
self-contained HTML page for a person, and one JSON document in an existing cross-language
schema for everything else. Neither invents a number — both ask `Summary` for the same score
stdout prints.

## Background

Three audiences read a mutation run, and stdout only serves the first.

| Reader | Wants | stdout gives |
| --- | --- | --- |
| The developer who ran it | The survivors, now | Yes |
| The reviewer on a pull request | A file to open | Nothing |
| A dashboard, a gate, another tool | Structured data | Nothing |

A survivor list longer than a screen is also unreadable in a terminal, and the one thing
stdout genuinely cannot do is group survivors per file and rank the files.

## The JSON is not ours

There is already a cross-language standard, so Angelo does not define one.

**`mutation-testing-report-schema`** version 2, published at
`http://stryker-mutator.io/report.schema.json` and maintained in
[stryker-mutator/mutation-testing-elements](https://github.com/stryker-mutator/mutation-testing-elements/blob/master/packages/report-schema/src/mutation-testing-report-schema.json).
StrykerJS, Stryker.NET, Stryker4s and muttest all emit it, and the
[mutation-testing-elements](https://stryker-mutator.io/docs/mutation-testing-elements/) viewer
and the Stryker dashboard both read it.

```bash
angelo exec --report angelo.json
```

### It already agreed with us

The schema's statuses map onto Angelo's without inventing anything.

| Angelo | Schema | Why |
| --- | --- | --- |
| `killed` | `Killed` | |
| `survived`, a test ran it | `Survived` | A test executes the line and asserts nothing about it |
| `survived`, nothing ran it | `NoCoverage` | No test executes the line at all |
| `timeout` | `Timeout` | Detected in both: behaviour observably changed |
| `error` | `RuntimeError` | |
| `untestable` | `Ignored` | Its only tests were already red |
| `pending` | `Pending` | Enumerated, not yet run |

**`survived` splits in two**, and that split is the useful part. A mutant no test covers wants
a test written; a mutant a test ran and missed wants an assertion added. They are different
jobs and the terminal never distinguished them.

So does the arithmetic. The schema defines the score as `detected / valid`, where detected is
killed plus timeout and valid excludes errors and ignored mutants. That is
[`Summary::score`](04-architecture.md) exactly, so there is one implementation and the file
cannot disagree with the terminal.

### What it costs

The schema requires **the full original text of every mutated file**, inlined. The viewer
cannot show a mutant in context without it. Expect a large file on a large repository: the
demo project's 74 mutants across 3 files come to 29 KB.

## The HTML is ours

One file, no network, no framework. Lighthouse is the model: a score at the top, the detail
underneath.

```bash
angelo exec --html-report angelo.html
```

| Section | Holds |
| --- | --- |
| **Score** | The number, in a red, amber or green band |
| **Worth checking** | Everything the run found wrong, before the numbers |
| **Counts** | Each verdict, and whether it counts toward the score |
| **Per file** | Mutants, survivors and score per file, **worst first** |
| **Survivors** | Grouped by file, `original → replacement`, uncovered ones marked |
| **Run facts** | Baseline, workers, batch size, coverage, selection, warm workers |

The CSS is inline and there is not one external reference in the document, so a CI artifact
and an aeroplane both render it. It follows the reader's light or dark theme.

**"Worth checking" is the section that matters.** A run can produce a confident-looking score
and still be untrustworthy — a red baseline, a sample, an `exclude` pattern that matched
nothing, coverage that matched no mutant. Those warnings used to scroll past in a terminal.
Now they sit above the score in the file someone attaches to a pull request.

## Both are output, never a verdict

Writing a report cannot change what a run decided.

- Reports are written **from the database**, not from the run, so `--init-only`, a resumed
  `exec` and a run with nothing pending all produce one.
- `scripts/verdict-matrix.sh` must still agree with itself with the flags on.
- A report is **not a gate**. Use it alongside `--fail-under`, never instead of it: the exit
  code is what stops a broken run, and a report is only visibility.

That last point is not a style preference. **A run where every mutant errored has no score**,
and a consumer that only reads survivors sees an empty list and renders it as "no problems
found". That is this tool's characteristic failure — inventing a test gap that does not exist
— running in reverse. It is why `error` maps to `RuntimeError` rather than being dropped, and
why the HTML says "no score" in words rather than showing a number.

## Configuration

Either flag can live in `angelo.conf` instead, and the flag wins.

```toml
report = "angelo.json"        # empty means off, which is the default
html_report = "angelo.html"   # empty means off, which is the default
```

## Limits

- **No wall-clock timestamp in the HTML.** Formatting a civil date needs either a dependency
  or fifteen lines of calendar arithmetic, and neither earns its place for a line the file's
  own modification time already carries.
- **No source view in the HTML.** Survivors are listed, not shown in context. The JSON carries
  every file's source, so Stryker's viewer already does this properly for anyone who wants it.
- **The JSON grows with the repository**, because the schema requires inlined sources.
- **No mutation score reaches SonarQube Cloud.** `--sonar-report` puts survivors on any
  Sonar dashboard — see [SonarQube](09-sonarqube.md) — but generic import creates issues,
  not measures. The percentage needs [the plugin](10-sonar-plugin.md), and third-party
  plugins run only on a self-hosted server.
