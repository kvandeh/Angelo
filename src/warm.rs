use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::pytest::{Selection, SuiteResult};

const DRIVER: &str = include_str!("runner/worker.py");
const DRIVER_FILE: &str = "angelo-worker.py";
/// Must match REPLY_PREFIX in worker.py.
const REPLY_PREFIX: &str = "##angelo##";

/// A pytest process kept alive across mutants. Saves the ~300ms of interpreter
/// start plus pytest import that every fresh run repays — the cost fork()
/// avoids on Unix and Windows cannot.
///
/// Everything here is best-effort: any timeout, crash, or unparseable reply
/// kills the worker and the caller falls back to a fresh subprocess, so a warm
/// run can only ever be faster, never a different verdict.
pub struct WarmWorker {
    child: Child,
    stdin: ChildStdin,
    replies: Receiver<String>,
    runs: usize,
    recycle_after: usize,
}

pub struct WarmRun {
    pub result: SuiteResult,
    pub failed_tests: Vec<String>,
}

impl WarmWorker {
    pub fn start(test_command: &[String], cwd: &Path, recycle_after: usize) -> Result<WarmWorker> {
        let driver = cwd.join(DRIVER_FILE);
        fs::write(&driver, DRIVER).with_context(|| format!("writing {}", driver.display()))?;

        let python = python_of(test_command)?;
        let mut child = Command::new(python)
            .arg(DRIVER_FILE)
            .current_dir(cwd)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env(
                "PYTHONPATH",
                std::env::join_paths([cwd.to_path_buf(), cwd.join("src")])
                    .context("building PYTHONPATH")?,
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("starting the warm pytest worker")?;

        let stdin = child.stdin.take().context("worker stdin")?;
        let stdout = child.stdout.take().context("worker stdout")?;
        let (sender, replies) = mpsc::channel();
        // A reader thread turns the blocking pipe into something we can wait on
        // with a timeout; it ends when the worker closes stdout. pytest prints
        // its own progress to the same stream, so only marked lines are replies.
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Some(reply) = line.strip_prefix(REPLY_PREFIX) else {
                    continue;
                };
                if sender.send(reply.to_string()).is_err() {
                    return;
                }
            }
        });

        Ok(WarmWorker {
            child,
            stdin,
            replies,
            runs: 0,
            recycle_after,
        })
    }

    pub fn is_exhausted(&self) -> bool {
        self.runs >= self.recycle_after
    }

    pub fn run(&mut self, selection: &Selection, timeout: Duration) -> Result<WarmRun> {
        let request = format!(
            "{{\"tests\":[{}],\"stop_at_first_failure\":{}}}\n",
            selection
                .test_ids
                .iter()
                .map(|id| format!("\"{}\"", id.replace('\\', "/")))
                .collect::<Vec<_>>()
                .join(","),
            selection.stop_at_first_failure
        );
        self.stdin
            .write_all(request.as_bytes())
            .context("sending work to the warm worker")?;
        self.stdin.flush().context("flushing the warm worker")?;
        self.runs += 1;

        match self.replies.recv_timeout(timeout) {
            Ok(line) => parse_reply(&line),
            Err(RecvTimeoutError::Timeout) => Ok(WarmRun {
                result: SuiteResult::TimedOut,
                failed_tests: Vec::new(),
            }),
            Err(RecvTimeoutError::Disconnected) => bail!("the warm worker died"),
        }
    }
}

impl Drop for WarmWorker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Hand-rolled rather than pulling in serde_json for two fields.
fn parse_reply(line: &str) -> Result<WarmRun> {
    let exit_code = field(line, "\"exit_code\":")
        .and_then(|rest| {
            let digits: String = rest
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '-')
                .collect();
            digits.parse::<i64>().ok()
        })
        .with_context(|| format!("no exit_code in worker reply: {line}"))?;

    let failed = field(line, "\"failed\": [")
        .or_else(|| field(line, "\"failed\":["))
        .map(|rest| {
            rest.split(']')
                .next()
                .unwrap_or("")
                .split(',')
                .filter_map(|item| {
                    let trimmed = item.trim().trim_matches('"');
                    (!trimmed.is_empty()).then(|| trimmed.to_string())
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(WarmRun {
        result: SuiteResult::Finished(exit_code),
        failed_tests: failed,
    })
}

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.find(key).map(|at| &line[at + key.len()..])
}

/// The worker replaces the pytest invocation, so only the interpreter carries
/// over from test_command. A command that is not `python -m pytest ...` cannot
/// be hosted and must keep using subprocesses.
pub fn hostable(test_command: &[String]) -> bool {
    matches!(test_command, [python, dash_m, pytest, ..]
        if python.contains("python") && dash_m == "-m" && pytest == "pytest")
}

fn python_of(test_command: &[String]) -> Result<&String> {
    test_command
        .first()
        .filter(|program| program.contains("python"))
        .context("the warm worker needs a python interpreter as test_command[0]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_worker_reply() {
        let run =
            parse_reply(r#"{"exit_code": 1, "failed": ["a.py::test_x", "b.py::test_y"]}"#).unwrap();
        assert_eq!(run.result.exit_code(), Some(1));
        assert_eq!(run.failed_tests, ["a.py::test_x", "b.py::test_y"]);
    }

    #[test]
    fn reads_a_green_reply() {
        let run = parse_reply(r#"{"exit_code": 0, "failed": []}"#).unwrap();
        assert_eq!(run.result.exit_code(), Some(0));
        assert!(run.failed_tests.is_empty());
    }

    #[test]
    fn rejects_a_reply_with_no_exit_code() {
        assert!(parse_reply("not json at all").is_err());
    }

    /// pytest shares stdout with the protocol, so the prefix is load-bearing.
    #[test]
    fn the_prefix_matches_the_driver() {
        assert!(DRIVER.contains(&format!("REPLY_PREFIX = \"{REPLY_PREFIX}\"")));
    }

    #[test]
    fn only_plain_pytest_commands_can_be_hosted() {
        let default = ["python", "-m", "pytest"].map(String::from);
        assert!(hostable(&default));
        let venv = ["/x/.venv/bin/python", "-m", "pytest", "-q"].map(String::from);
        assert!(hostable(&venv));
        let tox = ["tox".to_string()];
        assert!(!hostable(&tox));
        let unittest = ["python", "-m", "unittest"].map(String::from);
        assert!(!hostable(&unittest));
    }
}
