#!/usr/bin/env python3
"""Measure Angelo against mutmut and cosmic-ray on the same code, the same way.

The comparison rules matter more than the code:

* **Per-mutant seconds is the only fair time column.** The operator sets differ,
  so wall time compares two different jobs.
* **Scores are not comparable across tools.** Different pools. Printed side by
  side, labelled, never subtracted.
* **Same module, same tests, same worker count, same virtualenv.** Each tool is
  pointed at one source file and one test selection, so the job is identical.
* **Failures and timeouts are recorded per tool.** A tool that errors on 90% of
  its mutants looks fast and means nothing.

mutmut needs `fork()`, so this is Linux and macOS only.
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import statistics
import subprocess
import sys
import time
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path

REPOS = {
    "fastapi": {
        "package": "fastapi",
        "module": "fastapi/security/api_key.py",
        "tests": [
            "tests/test_security_api_key_query.py",
            "tests/test_security_api_key_header.py",
            "tests/test_security_api_key_cookie.py",
        ],
    },
    "thefuck": {
        "package": "thefuck",
        "module": "thefuck/types.py",
        "tests": ["tests/test_types.py"],
    },
    "graphify": {
        "package": "graphify",
        "module": "graphify/cluster.py",
        "tests": ["tests/test_cluster.py"],
    },
}

TOOLS = ("angelo", "mutmut", "cosmic-ray")


@dataclass
class Run:
    """One tool against one repository."""

    repo: str
    tool: str
    seconds: float | None = None
    mutants: int = 0
    killed: int = 0
    survived: int = 0
    errored: int = 0
    note: str = ""
    samples: list[float] = field(default_factory=list)

    @property
    def per_mutant(self) -> float | None:
        """Seconds per mutant, the only column that compares two tools honestly."""
        if self.seconds is None or self.mutants <= 0:
            return None
        return self.seconds / self.mutants

    @property
    def score(self) -> float | None:
        """Kill rate over this tool's own pool. Never comparable across tools."""
        judged = self.killed + self.survived
        return 100.0 * self.killed / judged if judged else None

    @property
    def ok(self) -> bool:
        return self.per_mutant is not None


@contextmanager
def restored(path: Path, content: str):
    """Write a file for the length of a run, then put back whatever was there."""
    original = path.read_text() if path.exists() else None
    path.write_text(content)
    try:
        yield
    finally:
        if original is None:
            path.unlink(missing_ok=True)
        else:
            path.write_text(original)


def tail(output: str, lines: int = 3) -> str:
    """The last few lines of a tool's own output, for a note the reader can act on."""
    kept = [l.strip() for l in output.strip().splitlines() if l.strip()][-lines:]
    return " / ".join(kept)[:300]


def shell(command: list[str], cwd: Path, timeout: float) -> tuple[int, str, float]:
    """Run a command, returning its exit code, merged output and wall time."""
    start = time.perf_counter()
    try:
        done = subprocess.run(
            command,
            cwd=cwd,
            capture_output=True,
            text=True,
            timeout=timeout,
            errors="replace",
        )
    except subprocess.TimeoutExpired as expired:
        output = (expired.output or b"")
        if isinstance(output, bytes):
            output = output.decode("utf-8", "replace")
        return 124, output, time.perf_counter() - start
    return done.returncode, done.stdout + done.stderr, time.perf_counter() - start


class Repo:
    """A checkout under the benchmark root, with its own virtualenv."""

    def __init__(self, root: Path, name: str, spec: dict) -> None:
        self.name = name
        self.path = root / name
        self.module = spec["module"]
        self.package = spec["package"]
        self.tests = spec["tests"]

    @property
    def python(self) -> Path:
        return self.path / ".venv" / "bin" / "python"

    @property
    def bin(self) -> Path:
        return self.path / ".venv" / "bin"

    def test_command(self) -> str:
        return f"{self.python} -m pytest -q " + " ".join(self.tests)

    def exists(self) -> bool:
        return self.python.exists() and (self.path / self.module).exists()

    def baseline_is_green(self, timeout: float) -> tuple[bool, str]:
        code, output, _ = shell(
            [str(self.python), "-m", "pytest", "-q", *self.tests], self.path, timeout
        )
        return code == 0, output.strip().splitlines()[-1] if output.strip() else ""

    def clean(self) -> None:
        """Delete every tool's cache so no run inherits another's answers."""
        for junk in (
            ".angelo",
            ".mutmut-cache",
            "mutants",
            "mutmut-stats.json",
            "cosmic-ray.sqlite",
            "cosmic-ray.toml",
            "angelo.conf",
            ".pytest_cache",
        ):
            target = self.path / junk
            if target.is_dir():
                shutil.rmtree(target, ignore_errors=True)
            elif target.exists():
                target.unlink()


def run_angelo(repo: Repo, angelo: Path, workers: int, timeout: float) -> Run:
    result = Run(repo.name, "angelo")
    config = "\n".join(
        [
            f'paths = ["{repo.module}"]',
            f'test_command = "{repo.test_command()}"',
            f"workers = {workers}",
            "batch_size = 8",
            "test_selection = true",
            "warm_workers = true",
            "timeout_factor = 2.0",
            "",
        ]
    )
    (repo.path / "angelo.conf").write_text(config)
    code, output, seconds = shell(
        [str(angelo), "exec", "--workers", str(workers)], repo.path, timeout
    )
    if code == 124:
        result.note = f"timed out after {timeout:.0f}s"
        return result
    for line in output.splitlines():
        found = re.match(r"\s*(killed|survived|timeout|error|untestable):\s*(\d+)", line)
        if not found:
            continue
        label, count = found.group(1), int(found.group(2))
        if label in ("killed", "timeout"):
            result.killed += count
        elif label == "survived":
            result.survived += count
        else:
            result.errored += count
    result.mutants = result.killed + result.survived + result.errored
    if not result.mutants:
        result.note = f"no mutants reported (exit {code}): {tail(output)}"
        return result
    result.seconds = seconds
    return result


def run_mutmut(repo: Repo, workers: int, timeout: float) -> Run:
    result = Run(repo.name, "mutmut")
    mutmut = repo.bin / "mutmut"
    if not mutmut.exists():
        result.note = "mutmut not installed"
        return result
    settings = "\n".join(
        [
            "[mutmut]",
            # The whole package, or mutmut's mutants/ copy is missing the modules
            # conftest imports; only_mutate then narrows it back to one module.
            f"source_paths={repo.package}",
            f"only_mutate={repo.module}",
            # mutmut runs its own stats pass, so left at the whole directory it
            # drags in every unrelated (and sometimes broken) test.
            "pytest_add_cli_args_test_selection=" + " ".join(repo.tests),
            f"runner={repo.python} -m pytest -x -q " + " ".join(repo.tests),
            "",
        ]
    )
    # Several of these repos keep their own setup.cfg, and pytest reads it too.
    # Overwriting it turns a green suite red for every tool that runs after.
    with restored(repo.path / "setup.cfg", settings):
        code, output, seconds = shell(
            [str(mutmut), "run", "--max-children", str(workers)], repo.path, timeout
        )
    if code == 124:
        result.note = f"timed out after {timeout:.0f}s"
        return result
    with restored(repo.path / "setup.cfg", settings):
        _, results, _ = shell([str(mutmut), "results"], repo.path, 120)
    unchecked = 0
    for line in results.splitlines():
        if ":" not in line:
            continue
        status = line.rsplit(":", 1)[1].strip().lower()
        if status in ("killed", "timeout"):
            result.killed += 1
        elif status in ("survived", "no tests"):
            # "no tests" is mutmut's untested, which angelo also scores as survived.
            result.survived += 1
        elif status in ("suspicious", "skipped", "caught by type check"):
            result.errored += 1
        elif status == "not checked":
            unchecked += 1
    result.mutants = result.killed + result.survived + result.errored
    if not result.mutants:
        reason = (
            f"{unchecked} mutants generated, none run"
            if unchecked
            else f"no mutants reported (exit {code})"
        )
        result.note = f"{reason}: {tail(output)}"
        return result
    result.seconds = seconds
    if unchecked:
        result.note = f"{unchecked} of {result.mutants + unchecked} mutants never ran"
    return result


def run_cosmic_ray(repo: Repo, workers: int, timeout: float) -> Run:
    result = Run(repo.name, "cosmic-ray")
    cosmic = repo.bin / "cosmic-ray"
    if not cosmic.exists():
        result.note = "cosmic-ray not installed"
        return result
    config = "\n".join(
        [
            "[cosmic-ray]",
            f'module-path = "{repo.module}"',
            "timeout = 60.0",
            "excluded-modules = []",
            f'test-command = "{repo.test_command()}"',
            "",
            "[cosmic-ray.distributor]",
            'name = "local"',
            "",
            "[cosmic-ray.distributor.local]",
            f"worker-count = {workers}",
            "",
        ]
    )
    (repo.path / "cosmic-ray.toml").write_text(config)
    session = repo.path / "cosmic-ray.sqlite"
    code, output, _ = shell(
        [str(cosmic), "init", "cosmic-ray.toml", session.name], repo.path, 300
    )
    if code != 0:
        result.note = f"init failed (exit {code}): {tail(output)}"
        return result
    code, output, seconds = shell(
        [str(cosmic), "exec", "cosmic-ray.toml", session.name], repo.path, timeout
    )
    total, done, killed, survived, errored = read_session(session)
    result.mutants, result.killed, result.survived, result.errored = (
        done, killed, survived, errored
    )
    if not done:
        result.note = f"no mutants completed (exit {code}): {tail(output)}"
        return result
    result.seconds = seconds
    if code == 124:
        result.note = f"timed out after {timeout:.0f}s, {done} of {total} mutants done"
    return result


def read_session(session: Path) -> tuple[int, int, int, int, int]:
    """cosmic-ray's own session database, which survives a timeout intact."""
    import sqlite3

    if not session.exists():
        return 0, 0, 0, 0, 0
    connection = sqlite3.connect(f"file:{session}?mode=ro", uri=True)
    try:
        total = connection.execute("select count(*) from work_items").fetchone()[0]
        outcomes = dict(
            connection.execute(
                "select test_outcome, count(*) from work_results group by test_outcome"
            ).fetchall()
        )
    except sqlite3.Error:
        return 0, 0, 0, 0, 0
    finally:
        connection.close()
    killed = survived = 0
    for label, count in outcomes.items():
        # Recorded as the enum rather than its value on some versions, so this
        # matches "killed", "KILLED" and "TestOutcome.KILLED" alike.
        name = str(label).lower()
        if "killed" in name:
            killed += count
        elif "survived" in name:
            survived += count
    done = sum(outcomes.values())
    return total, done, killed, survived, done - killed - survived


RUNNERS = {"angelo": run_angelo, "mutmut": run_mutmut, "cosmic-ray": run_cosmic_ray}


def measure(repo: Repo, tool: str, angelo: Path, workers: int, timeout: float,
            repeats: int, warmup: bool) -> Run:
    """Run one tool, discarding a warm-up and reporting the median of the rest."""
    attempts: list[Run] = []
    total = repeats + (1 if warmup else 0)
    for index in range(total):
        repo.clean()
        if tool == "angelo":
            attempt = run_angelo(repo, angelo, workers, timeout)
        else:
            attempt = RUNNERS[tool](repo, workers, timeout)
        if warmup and index == 0:
            continue
        attempts.append(attempt)
    good = [a for a in attempts if a.ok]
    if not good:
        return attempts[-1] if attempts else Run(repo.name, tool, note="no runs")
    best = good[0]
    best.samples = [a.seconds for a in good if a.seconds is not None]
    best.seconds = statistics.median(best.samples)
    return best


def write_markdown(runs: list[Run], path: Path) -> None:
    lines = [
        "# Benchmark results",
        "",
        "Per-mutant seconds is the only fair time column: the operator sets differ, so",
        "wall time compares two different jobs. Scores come from different pools and are",
        "printed side by side, never subtracted.",
        "",
    ]
    for repo in REPOS:
        rows = [r for r in runs if r.repo == repo]
        if not rows:
            continue
        spec = REPOS[repo]
        lines += [
            f"## {repo}",
            "",
            f"Module `{spec['module']}`, tests `{' '.join(spec['tests'])}`.",
            "",
            "| Tool | Mutants | Wall time | Per mutant | Killed | Survived | Error | Score | Note |",
            "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |",
        ]
        for row in rows:
            wall = f"{row.seconds:.1f}s" if row.seconds is not None else "—"
            per = f"{row.per_mutant:.3f}s" if row.per_mutant is not None else "—"
            score = f"{row.score:.1f}%" if row.score is not None else "—"
            lines.append(
                f"| {row.tool} | {row.mutants or '—'} | {wall} | {per} | {row.killed} "
                f"| {row.survived} | {row.errored} | {score} | {row.note or ''} |"
            )
        lines.append("")
    path.write_text("\n".join(lines))


def write_chart(runs: list[Run], path: Path) -> None:
    try:
        import matplotlib

        matplotlib.use("Agg")
        import matplotlib.pyplot as plt
    except ImportError:
        print("matplotlib missing, skipping the chart "
              "(pip install -r scripts/requirements-bench.txt)")
        return

    repos = list(REPOS)
    colours = {"fastapi": "#4C72B0", "thefuck": "#DD8452", "graphify": "#55A868"}
    fig, axes = plt.subplots(figsize=(11, 6.2))

    ticks, labels, bars = [], [], []
    position = 0.0
    for tool in TOOLS:
        for repo in repos:
            row = next((r for r in runs if r.repo == repo and r.tool == tool), None)
            value = row.per_mutant if row and row.per_mutant is not None else 0.0
            bars.append((position, value, repo, row))
            ticks.append(position)
            labels.append(repo)
            position += 1
        position += 0.9

    widest = max((value for _, value, _, _ in bars), default=1.0) or 1.0
    for y, value, repo, row in bars:
        axes.barh(y, value, height=0.75, color=colours[repo], zorder=3)
        if row is None or row.per_mutant is None:
            # The full note belongs in the table; a bar chart only has room for
            # the headline, and a long one stretches the whole figure.
            note = (row.note if row and row.note else "no result").split(":")[0]
            axes.text(widest * 0.01, y, f"  {note[:46]}", va="center", fontsize=8,
                      color="#B03030", zorder=4)
            continue
        score = f"{row.score:.0f}%" if row.score is not None else "n/a"
        axes.text(value + widest * 0.012, y,
                  f"{value:.2f}s  ·  {score} caught",
                  va="center", fontsize=8.5, color="#333333", zorder=4)

    axes.set_yticks(ticks)
    axes.set_yticklabels(labels, fontsize=9)
    axes.tick_params(axis="y", length=0, pad=6)
    axes.invert_yaxis()
    axes.set_xlabel("Seconds per mutant (lower is faster)", fontsize=10)
    axes.set_xlim(0, widest * 1.38)
    axes.set_title(
        "Mutation testing speed: seconds per mutant, same module and same tests",
        fontsize=12.5, pad=32,
    )
    axes.grid(axis="x", color="#DDDDDD", zorder=0)
    axes.set_axisbelow(True)
    for spine in ("top", "right", "left"):
        axes.spines[spine].set_visible(False)

    for index, tool in enumerate(TOOLS):
        middle = index * (len(repos) + 0.9) + (len(repos) - 1) / 2
        axes.text(-widest * 0.175, middle, tool, ha="center", va="center",
                  fontsize=11, fontweight="bold", rotation=90)

    handles = [plt.Rectangle((0, 0), 1, 1, color=colours[r]) for r in repos]
    axes.legend(handles, repos, title="repository", loc="upper right",
                frameon=False, fontsize=9, title_fontsize=9)
    fig.text(0.5, 0.925,
             "Percentages are each tool's own kill rate over its own pool — "
             "different operator sets, so never subtract them.",
             ha="center", fontsize=8.5, color="#666666")
    fig.tight_layout()
    fig.savefig(path, dpi=170, bbox_inches="tight")
    print(f"wrote {path}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True,
                        help="directory holding the cloned repositories")
    parser.add_argument("--angelo", type=Path, required=True, help="the angelo binary")
    parser.add_argument("--tools", default=",".join(TOOLS),
                        help="comma separated subset of tools to run")
    parser.add_argument("--repos", default=",".join(REPOS),
                        help="comma separated subset of repositories to run")
    parser.add_argument("--workers", type=int, default=8)
    parser.add_argument("--timeout", type=float, default=600.0,
                        help="wall clock budget per tool per repository")
    parser.add_argument("--repeats", type=int, default=1,
                        help="measured runs per pair, reported as a median")
    parser.add_argument("--warmup", action="store_true",
                        help="discard one run per pair before measuring")
    parser.add_argument("--out", type=Path, default=Path("."))
    options = parser.parse_args()

    if sys.platform == "win32":
        print("mutmut needs fork(), so this benchmark is Linux and macOS only.")
        return 2

    tools = [t for t in options.tools.split(",") if t]
    unknown = set(tools) - set(TOOLS)
    if unknown:
        print(f"unknown tools: {', '.join(sorted(unknown))}")
        return 2

    runs: list[Run] = []
    for name in options.repos.split(","):
        if name not in REPOS:
            print(f"unknown repo {name}, skipping")
            continue
        repo = Repo(options.root, name, REPOS[name])
        if not repo.exists():
            print(f"=== {name}: not set up, skipping")
            continue
        green, summary = repo.baseline_is_green(300)
        if not green:
            print(f"=== {name}: RED baseline, skipping — {summary}")
            for tool in tools:
                runs.append(Run(name, tool, note="skipped, red baseline"))
            continue
        print(f"=== {name}: green baseline — {summary}")
        for tool in tools:
            print(f"  {tool} ...", flush=True)
            run = measure(repo, tool, options.angelo, options.workers,
                          options.timeout, options.repeats, options.warmup)
            runs.append(run)
            if run.ok:
                print(f"    {run.mutants} mutants in {run.seconds:.1f}s "
                      f"= {run.per_mutant:.3f}s each, "
                      f"{run.killed} killed / {run.survived} survived / {run.errored} error")
            else:
                print(f"    no result: {run.note}")
            repo.clean()

    options.out.mkdir(parents=True, exist_ok=True)
    write_markdown(runs, options.out / "bench-results.md")
    (options.out / "bench-results.json").write_text(
        json.dumps([r.__dict__ for r in runs], indent=2, default=str)
    )
    write_chart(runs, options.out / "bench-results.png")
    print(f"wrote {options.out / 'bench-results.md'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
