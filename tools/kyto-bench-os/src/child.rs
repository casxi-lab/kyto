//! One unit test run alone in a child process.
//!
//! A test that delivers a real signal, that changes a state of the whole
//! process, such as a signal disposition, a limit, a standard stream or the
//! environment, or that runs at another scheduling priority, runs its own
//! part in a child process: this test binary again, running only that test.
//! A signal then reaches the child alone, so the test runner keeps its own
//! signal dispositions. A thread reads the child's standard output line by
//! line and another its standard error; a child still running when the
//! parent fails is killed.

use crate::TEST_WAIT;
use std::io::{BufRead, BufReader, Read};
use std::process::Stdio;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The variable under which a test that [`ChildTest::spawn`] runs takes the
/// child's part; its value is what the parent passes.
pub const CHILD: &str = "KREPI_TOOLS_TEST_CHILD";

/// What the parent passed this process, if it is such a child.
pub fn child_part() -> Option<String> {
    std::env::var(CHILD).ok()
}

/// One test of this binary running alone in a child process.
pub struct ChildTest {
    name: String,
    child: std::process::Child,
    lines: mpsc::Receiver<String>,
    seen: Vec<String>,
    stderr: Option<JoinHandle<Vec<u8>>>,
}

/// How a child ended: its status, the lines of its standard output, the last
/// one without its newline if it had none, and its standard error.
#[derive(Debug)]
pub struct Ended {
    pub status: std::process::ExitStatus,
    pub lines: Vec<String>,
    pub stderr: String,
}

impl Ended {
    /// The lines that the code under test printed: all but the test
    /// runner's own lines of a run of one test in the terse format.
    pub fn printed(&self) -> Vec<&str> {
        self.lines
            .iter()
            .map(String::as_str)
            .filter(|line| {
                !matches!(*line, "" | "." | "running 1 test") && !line.starts_with("test result: ")
            })
            .collect()
    }

    /// Whether the test runner reports the one test as passed.
    pub fn passed(&self) -> bool {
        self.status.success()
            && self
                .lines
                .iter()
                .any(|line| line.starts_with("test result: ok. 1 passed;"))
    }
}

impl ChildTest {
    /// Runs the test `test` of `module`, a `module_path!()`, alone with
    /// [`CHILD`] set to `part`. The test runner does not capture the child's
    /// output, so what the code under test prints, and a panic's message,
    /// reach the child's standard streams.
    #[track_caller]
    pub fn spawn(module: &str, test: &str, part: &str) -> Self {
        Self::start(&[], module, test, part, &[])
    }

    /// [`ChildTest::spawn`] with the child's environment changed, for a
    /// variable that must be in place when the child starts: each one is set
    /// to its value, or removed without one, so that what the parent
    /// inherited cannot stand in for it.
    #[track_caller]
    pub fn spawn_with(
        module: &str,
        test: &str,
        part: &str,
        environment: &[(&str, Option<&str>)],
    ) -> Self {
        Self::start(&[], module, test, part, environment)
    }

    /// [`ChildTest::spawn`] with the test binary started by the command
    /// `prefix`, such as `nice -n 19`, which runs it with its arguments.
    #[track_caller]
    pub fn spawn_under(prefix: &[&str], module: &str, test: &str, part: &str) -> Self {
        Self::start(prefix, module, test, part, &[])
    }

    #[track_caller]
    fn start(
        prefix: &[&str],
        module: &str,
        test: &str,
        part: &str,
        environment: &[(&str, Option<&str>)],
    ) -> Self {
        // The test runner names a test by its path below the crate.
        let (_, path) = module
            .split_once("::")
            .expect("a module path below the crate");
        let name = format!("{path}::{test}");
        let executable = std::env::current_exe().expect("locate the test executable");
        let mut command = match prefix.split_first() {
            Some((program, args)) => {
                let mut command = std::process::Command::new(program);
                command.args(args).arg(executable);
                command
            }
            None => std::process::Command::new(executable),
        };
        command
            .args([
                name.as_str(),
                "--exact",
                "--test-threads=1",
                "--quiet",
                "--nocapture",
            ])
            .env(CHILD, part)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (variable, value) in environment {
            match value {
                Some(value) => command.env(variable, value),
                None => command.env_remove(variable),
            };
        }
        let mut child = command.spawn().expect("start the test executable");
        let stdout = child.stdout.take().expect("a piped standard output");
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).split(b'\n') {
                let Ok(line) = line else { break };
                let line = String::from_utf8_lossy(&line).into_owned();
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let mut stderr = child.stderr.take().expect("a piped standard error");
        let stderr = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes);
            bytes
        });
        ChildTest {
            name,
            child,
            lines,
            seen: Vec::new(),
            stderr: Some(stderr),
        }
    }

    /// Waits until `reached` holds of something that the child brings about.
    /// A child that ends first, or that has not brought it about within
    /// [`TEST_WAIT`], fails the test with what it printed.
    #[track_caller]
    pub fn await_state(&mut self, state: &str, mut reached: impl FnMut() -> bool) {
        let deadline = Instant::now() + TEST_WAIT;
        while !reached() {
            let ended = matches!(self.child.try_wait(), Ok(Some(_)));
            if ended || Instant::now() >= deadline {
                let ended = self.stop();
                panic!("{}: not {state}: {ended:?}", self.name);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Sends `signal` to the child, never to this process.
    #[track_caller]
    pub fn signal(&self, signal: libc::c_int) {
        let pid = libc::pid_t::try_from(self.child.id()).expect("a process number");
        // SAFETY: kill takes a process number and a signal number and
        // touches no memory of this process; the child is not reaped yet, so
        // the number is still its own.
        assert_eq!(unsafe { libc::kill(pid, signal) }, 0, "signal {signal}");
    }

    /// Waits for the child's end. A child that has not ended within
    /// [`TEST_WAIT`] is killed, and the test fails with its name.
    #[track_caller]
    pub fn finish(mut self) -> Ended {
        let deadline = Instant::now() + TEST_WAIT;
        while self
            .child
            .try_wait()
            .expect("look at the child's status")
            .is_none()
        {
            if Instant::now() >= deadline {
                let ended = self.stop();
                panic!(
                    "{}: the child did not end within {TEST_WAIT:?}: {ended:?}",
                    self.name
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        self.stop()
    }

    /// Kills the child unless it has ended, reaps it and collects its
    /// output, which ends with its pipes.
    fn stop(&mut self) -> Ended {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            // Not yet reaped, so the number is still the child's.
            let _ = self.child.kill();
        }
        let status = self.child.wait().expect("reap the child");
        let deadline = Instant::now() + TEST_WAIT;
        while let Ok(line) = self
            .lines
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            self.seen.push(line);
        }
        let stderr = self
            .stderr
            .take()
            .map_or_else(Vec::new, |stderr| stderr.join().unwrap_or_default());
        Ended {
            status,
            lines: std::mem::take(&mut self.seen),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        }
    }
}

impl Drop for ChildTest {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
