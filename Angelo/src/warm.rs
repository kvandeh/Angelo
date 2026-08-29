use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::pytest::{self, Selection, SuiteResult};

const DRIVER: &str = include_str!("runner/worker.py");
const DRIVER_FILE: &str = "angelo-worker.py";
/// Must match REPLY_PREFIX in worker.py.
const REPLY_PREFIX: &str = "##angelo##";

/// A pytest process kept alive across mutants. Saves the ~300ms of interpreter
/// start plus pytest import that every fresh run repays, the cost fork()
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
    /// Set the first time a reply says so; a worker either forks or it does not.
    forks: bool,
}

pub struct WarmRun {
    pub result: SuiteResult,
    pub failed_tests: Vec<String>,
    /// The worker ran this mutant in a child of its own, so nothing it did can
    /// reach the next one.
    pub forked: bool,
}

impl WarmWorker {
    pub fn start(test_command: &[String], cwd: &Path, recycle_after: usize) -> Result<WarmWorker> {
        let driver = cwd.join(DRIVER_FILE);
        fs::write(&driver, DRIVER).with_context(|| format!("writing {}", driver.display()))?;

        let python = python_of(test_command)?;
        let mut child = Command::new(pytest::resolve_on_path(python))
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
            forks: false,
        })
    }

    /// Recycling exists to bound the state a purging worker accumulates. A
    /// forking worker accumulates none — every mutant ran in a child that has
    /// since died — so restarting it would only repay the warm-up for nothing.
    pub fn is_exhausted(&self) -> bool {
        !self.forks && self.runs >= self.recycle_after
    }

    /// `active` names the mutants the tree's schemata should switch on, empty
    /// when the mutants were spliced into the files instead. Splicing is what
    /// makes a re-import necessary, so it is also what decides `purge`.
    pub fn run(
        &mut self,
        selection: &Selection,
        timeout: Duration,
        active: &str,
    ) -> Result<WarmRun> {
        let request = format!(
            "{{\"tests\":[{}],\"stop_at_first_failure\":{},\"mutants\":\"{active}\",\"purge\":{},\"timeout_ms\":{}}}\n",
            selection
                .test_ids
                .iter()
                .map(|id| format!("\"{}\"", id.replace('\\', "/")))
                .collect::<Vec<_>>()
                .join(","),
            selection.stop_at_first_failure,
            active.is_empty(),
            timeout.as_millis(),
        );
        self.stdin
            .write_all(request.as_bytes())
            .context("sending work to the warm worker")?;
        self.stdin.flush().context("flushing the warm worker")?;
        self.runs += 1;

        // The worker enforces the same deadline itself and answers when it
        // expires, so this one only has to catch a worker that stopped
        // answering at all. Too tight and a live run is called a timeout, and a
        // timeout counts as detected.
        match self.replies.recv_timeout(timeout + Duration::from_secs(5)) {
            Ok(line) => {
                let run = parse_reply(&line)?;
                self.forks |= run.forked;
                Ok(run)
            }
            // A worker that never answered gives no guarantee about what it
            // left behind, whatever it usually does, so this run does not count
            // as forked and the caller retires it.
            Err(RecvTimeoutError::Timeout) => Ok(WarmRun {
                result: SuiteResult::TimedOut,
                failed_tests: Vec::new(),
                forked: false,
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
    // The fork path kills a child that outran its deadline and says so, rather
    // than going quiet and costing the whole worker.
    if line.contains("\"timed_out\": true") || line.contains("\"timed_out\":true") {
        return Ok(WarmRun {
            result: SuiteResult::TimedOut,
            failed_tests: Vec::new(),
            forked: line.contains("\"forked\""),
        });
    }
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
        forked: line.contains("\"forked\""),
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

    /// A child the worker killed on its own deadline. The verdict is a real
    /// timeout, and the worker is still alive to take the next mutant.
    #[test]
    fn reads_a_timed_out_reply() {
        let run = parse_reply(r#"{"exit_code": 0, "failed": [], "timed_out": true}"#).unwrap();
        assert!(matches!(run.result, SuiteResult::TimedOut));
    }

    /// The driver reads this variable to decide whether the tree holds schemata
    /// and, when it does, which mutants are live.
    #[test]
    fn the_driver_and_schemata_agree_on_the_variable() {
        assert!(DRIVER.contains(&format!("ACTIVE_VAR = \"{}\"", crate::schemata::ACTIVE_VAR)));
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
