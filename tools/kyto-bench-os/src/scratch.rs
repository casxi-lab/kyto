//! A directory for the files of one unit test, below the build directory and
//! removed with its value.

use crate::TEST_WAIT;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Distinguishes the directories of one test process.
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// A fresh, empty directory that is removed when the value is dropped.
pub struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// Creates the directory, named after `label`, the process and a counter,
    /// in `scratch` below the profile directory of the build, whose `deps`
    /// directory holds the test executable. Test files thus never fill the
    /// system temporary directory.
    pub fn new(label: &str) -> Scratch {
        let executable = std::env::current_exe().expect("locate the test executable");
        let profile = executable
            .parent()
            .and_then(|deps| deps.parent())
            .expect("the test executable lies in the deps directory of a profile");
        let root = profile.join("scratch");
        std::fs::create_dir_all(&root).expect("create the scratch root");
        let path = root.join(format!(
            "krepi-tools-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        // A directory of the same name can only be left over from an earlier
        // process with the same identifier.
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir(&path).expect("create the scratch directory");
        Scratch { path }
    }

    /// The path of the directory.
    pub fn root(&self) -> &Path {
        &self.path
    }

    /// The path of `name` inside the directory.
    pub fn path(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    /// Writes `data` to `name` inside the directory and returns its path.
    pub fn write(&self, name: &str, data: &[u8]) -> PathBuf {
        let path = self.path(name);
        std::fs::write(&path, data).expect("write a scratch file");
        path
    }

    /// Writes the shell script `text` to the new file `name` inside the
    /// directory, executable by everyone, and returns its path once it can be
    /// run. The script must end at once when its first argument is
    /// `--version`, as a fake QEMU does: it is run so until its start no
    /// longer fails with `ETXTBSY`. A child process that another test forks
    /// while the file is open for writing holds a copy of the descriptor until
    /// its own exec, and Linux refuses to execute a file that is open for
    /// writing. A test fails when the script cannot be run within
    /// [`TEST_WAIT`].
    #[track_caller]
    pub fn script(&self, name: &str, text: &str) -> PathBuf {
        let path = self.path(name);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o755)
            .open(&path)
            .unwrap_or_else(|error| panic!("cannot create {path:?}: {error}"));
        file.write_all(text.as_bytes())
            .unwrap_or_else(|error| panic!("cannot write {path:?}: {error}"));
        drop(file);
        let deadline = Instant::now() + TEST_WAIT;
        loop {
            let started = Command::new(&path)
                .arg("--version")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            match started {
                Err(error)
                    if error.raw_os_error() == Some(libc::ETXTBSY) && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("cannot run {path:?}: {error}"),
                Ok(_) => return path,
            }
        }
    }

    /// The bytes of `name` inside the directory.
    #[track_caller]
    pub fn read(&self, name: &str) -> Vec<u8> {
        let path = self.path(name);
        std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path:?}: {error}"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
