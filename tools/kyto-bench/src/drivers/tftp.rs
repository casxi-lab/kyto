//! The delivery device of the kind `tftp-directory`: a directory of the
//! bench host from which a TFTP server serves the board. The board reads
//! every file through a symbolic link in it whose name is the path prefix
//! of the instance configuration without its final `/`, such as `example`
//! for `example/`.
//!
//! Each payload is staged as a new generation `releases/gNNNNNN` below the
//! directory, of files of the mode `0644` and directories of `0755`; a name
//! that `releases` holds already, in any letter case, or that the link
//! names, is never used again, and nothing in `releases` is changed or
//! removed, whoever made it. A switch makes a new link to the relative path
//! `releases/gNNNNNN`, which a server that serves from a changed root
//! resolves as the tool does, and renames it over the old link, so that the
//! link's name stays present and names the old generation or the new one;
//! on macOS a lookup through the link that meets the rename can fail with
//! `EINVAL`, which is why the tool switches only while readings confirm the
//! supply off. It switches only to a generation that
//! the same driver staged whole. From its first staging or switch until the
//! bench closes, the driver holds a lock on the directory, so that no other
//! run or publication stages or switches there meanwhile. Its errors name
//! no path and no link.

use std::collections::BTreeSet;
use std::fs::{self, DirBuilder, File, OpenOptions, Permissions, TryLockError};
use std::io::{self, ErrorKind, Write as _};
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use kyto_bench_core::instance::Instance;
use kyto_bench_core::names::{self, GenerationName, PayloadName};

use crate::device::{Delivery, DeviceError, Role};
use crate::gate::Checked;

/// The kind of the delivery device.
pub const KIND: &str = "tftp-directory";

/// The directory of the generations.
const RELEASES: &str = "releases";
/// The name of a new link before it is renamed over the served one. No
/// path prefix names it, as `+` is in no payload name.
const SWITCH: &str = ".kyto-bench+switch";

/// `[local.delivery]` of a `tftp-directory`: the absolute path of the
/// directory from which the TFTP server serves the board.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliverySettings {
    pub dir: String,
}

/// The directory of a TFTP directory and the name of the link through
/// which the board is served.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TftpSettings {
    pub dir: PathBuf,
    pub link: String,
}

const ABSOLUTE: &str = "[local.delivery] dir: an absolute path without a NUL byte";
const ONE_DIRECTORY: &str = "[tftp] path_prefix: a tftp-directory serves the board through one directory and a slash, such as example/";

/// The settings of the TFTP directory of `instance`, checked for their form
/// alone: nothing is opened, nor whether the directory exists.
pub fn settings(instance: &Instance) -> Result<TftpSettings, String> {
    let local: DeliverySettings = super::local(instance, Role::Delivery)?;
    if !local.dir.starts_with('/') || local.dir.contains('\0') {
        return Err(ABSOLUTE.into());
    }
    let prefix = &instance.parameters().tftp.path_prefix;
    let link = prefix.strip_suffix('/');
    let link = link.filter(|name| names::is_segment(name) && !name.eq_ignore_ascii_case(RELEASES));
    let link = link.ok_or(ONE_DIRECTORY)?.to_string();
    let dir = PathBuf::from(local.dir);
    Ok(TftpSettings { dir, link })
}

/// Whether a run or a publication holds the TFTP directory of `instance`,
/// when its delivery device is a `tftp-directory`: the lock is tried
/// without waiting, and released at once.
pub fn in_use(instance: &Instance) -> bool {
    if instance.devices().delivery.kind != KIND {
        return false;
    }
    let Ok(tftp) = settings(instance) else {
        return false;
    };
    let tried = File::open(tftp.dir).map(|dir| dir.try_lock());
    matches!(tried, Ok(Err(TryLockError::WouldBlock)))
}

/// The TFTP directory of its settings.
pub struct TftpDirectory {
    dir: PathBuf,
    link: String,
    /// The generations that this driver staged whole.
    staged: BTreeSet<GenerationName>,
    /// The lock on the directory, from the first staging or switch on.
    lock: Option<File>,
}

/// A failure of the TFTP directory.
fn failed(detail: impl Into<String>) -> DeviceError {
    DeviceError::new(Role::Delivery, detail)
}

/// The generation of a link's `target`: exactly `releases/gNNNNNN`.
fn generation_of(target: &Path) -> Option<GenerationName> {
    let name = target.strip_prefix(RELEASES).ok()?.to_str()?;
    GenerationName::parse(name).ok()
}

/// Makes the directory at `path`, of the mode `0755`.
fn make_dir(path: &Path) -> io::Result<()> {
    DirBuilder::new().create(path)?;
    fs::set_permissions(path, Permissions::from_mode(0o755))
}

impl TftpDirectory {
    /// The directory of `settings`; nothing is opened before the first
    /// staging or switch.
    pub fn new(settings: TftpSettings) -> Self {
        let TftpSettings { dir, link } = settings;
        let (staged, lock) = (BTreeSet::new(), None);
        Self {
            dir,
            link,
            staged,
            lock,
        }
    }

    /// Takes the lock on the directory, unless it holds it already.
    fn hold(&mut self) -> Result<(), String> {
        if self.lock.is_some() {
            return Ok(());
        }
        if !fs::metadata(&self.dir).is_ok_and(|dir| dir.is_dir()) {
            return Err("the TFTP directory is not a directory".into());
        }
        let dir = File::open(&self.dir).map_err(|error| format!("lock: {error}"))?;
        match dir.try_lock() {
            Ok(()) => {
                self.lock = Some(dir);
                Ok(())
            }
            Err(TryLockError::WouldBlock) => {
                Err("a run or a publication holds the TFTP directory".into())
            }
            Err(TryLockError::Error(error)) => Err(format!("lock: {error}")),
        }
    }

    /// Stages `files` as a new generation below `releases`, made first if
    /// it is not there, after every one that it holds or the link names.
    fn stage(&mut self, files: &[(PayloadName, Vec<u8>)]) -> Result<GenerationName, String> {
        self.hold()?;
        let releases = self.dir.join(RELEASES);
        match fs::symlink_metadata(&releases) {
            Ok(found) if found.is_dir() => {}
            Ok(_) => return Err("releases is not a directory of its own".into()),
            Err(error) if error.kind() == ErrorKind::NotFound => {
                make_dir(&releases).map_err(|error| format!("cannot make releases: {error}"))?;
            }
            Err(error) => return Err(format!("releases: {error}")),
        }
        names::check_distinct(files.iter().map(|(name, _)| name)).map_err(|e| e.to_string())?;
        let read = |error: io::Error| format!("cannot read releases: {error}");
        let mut taken = Vec::new();
        for entry in fs::read_dir(&releases).map_err(read)? {
            let name = entry
                .map_err(read)?
                .file_name()
                .to_string_lossy()
                .to_ascii_lowercase();
            taken.extend(GenerationName::parse(&name).ok());
        }
        let served = fs::read_link(self.dir.join(&self.link)).ok();
        taken.extend(served.as_deref().and_then(generation_of));
        let generation = GenerationName::next_after(taken).ok_or("no generation name is left")?;
        let root = releases.join(generation.to_string());
        DirBuilder::new()
            .create(&root)
            .map_err(|error| match error.kind() {
                ErrorKind::AlreadyExists => format!("{generation} exists already"),
                _ => format!("cannot make {generation}: {error}"),
            })?;
        let mut directories = Vec::new();
        for (name, bytes) in files {
            let written = write_file(&root, name, bytes, &mut directories);
            written.map_err(|error| format!("cannot write {generation}/{name}: {error}"))?;
        }
        for directory in directories.iter().rev().chain([&root]) {
            let mode = fs::set_permissions(directory, Permissions::from_mode(0o755));
            mode.map_err(|error| format!("cannot write {generation}: {error}"))?;
        }
        Ok(generation)
    }

    /// Makes `generation` the served one; the one served before, when the
    /// old link pointed to a generation.
    fn switch(&mut self, generation: &GenerationName) -> Result<Option<GenerationName>, String> {
        if !self.staged.contains(generation) {
            return Err(format!("{generation} was not staged whole on this bench"));
        }
        self.hold()?;
        let link = self.dir.join(&self.link);
        let previous = match fs::symlink_metadata(&link) {
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Ok(found) if found.is_symlink() => fs::read_link(&link)
                .ok()
                .and_then(|target| generation_of(&target)),
            Ok(_) => return Err("the served path is not a symbolic link".into()),
            Err(error) => return Err(format!("the served path: {error}")),
        };
        let new = self.dir.join(SWITCH);
        match fs::symlink_metadata(&new) {
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Ok(found) if found.is_symlink() => {
                fs::remove_file(&new).map_err(|error| format!("new link: {error}"))?;
            }
            Ok(_) => return Err(format!("the name of the new link, {SWITCH}, is taken")),
            Err(error) => return Err(format!("new link: {error}")),
        }
        let target = Path::new(RELEASES).join(generation.to_string());
        symlink(target, &new).map_err(|error| format!("new link: {error}"))?;
        fs::rename(&new, &link).map_err(|error| format!("switch: {error}"))?;
        Ok(previous)
    }
}

/// Writes the file `name` of `bytes` below `root`, of the mode `0644`,
/// after the directories of its name that `made` does not hold, which it
/// makes and adds to `made`.
fn write_file(
    root: &Path,
    name: &PayloadName,
    bytes: &[u8],
    made: &mut Vec<PathBuf>,
) -> io::Result<()> {
    let path = root.join(name.as_str());
    let parents: Vec<&Path> = path
        .ancestors()
        .skip(1)
        .take_while(|p| *p != root)
        .collect();
    for directory in parents.into_iter().rev() {
        if !made.iter().any(|made| made == directory) {
            DirBuilder::new().create(directory)?;
            made.push(directory.to_path_buf());
        }
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::set_permissions(&path, Permissions::from_mode(0o644))
}

impl Delivery for TftpDirectory {
    fn write_generation(&mut self, files: Checked<'_>) -> Result<GenerationName, DeviceError> {
        let generation = self.stage(files.files()).map_err(failed)?;
        self.staged.insert(generation);
        Ok(generation)
    }

    fn read_staged(
        &mut self,
        generation: &GenerationName,
        name: &PayloadName,
    ) -> Result<Vec<u8>, DeviceError> {
        let path = self.dir.join(RELEASES).join(generation.to_string());
        let read = fs::read(path.join(name.as_str()));
        read.map_err(|error| failed(format!("cannot read {generation}/{name}: {error}")))
    }

    fn switch_to(
        &mut self,
        generation: &GenerationName,
    ) -> Result<Option<GenerationName>, DeviceError> {
        self.switch(generation).map_err(failed)
    }

    fn read_served(&mut self, name: &PayloadName) -> Result<Vec<u8>, DeviceError> {
        let read = fs::read(self.dir.join(&self.link).join(name.as_str()));
        read.map_err(|error| failed(format!("cannot read the served {name}: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    use kyto_bench_core::board::{Board, RASPBERRY_PI_5};
    use kyto_bench_core::write::WriteEnables;

    use super::*;
    use crate::device::StageError;
    use crate::drivers::ina228;
    use crate::drivers::tests::instance;
    use crate::gate::WriteGate;

    const REAL: [&str; 4] = ["lcus-1", ina228::KIND, "serial", KIND];

    /// An empty directory of its own for `test`, made again by each run of
    /// the tests.
    fn scratch(test: &str) -> PathBuf {
        let name = format!("tftp-{test}-{}", std::process::id());
        let dir = std::env::current_exe().unwrap().with_file_name(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        dir
    }

    /// The TFTP directory `dir`, which serves through `example`.
    fn tftp(dir: &Path) -> TftpDirectory {
        let (dir, link) = (dir.to_path_buf(), "example".to_string());
        TftpDirectory::new(TftpSettings { dir, link })
    }

    fn name(text: &str) -> PayloadName {
        PayloadName::parse(text).unwrap()
    }

    fn generation(number: u32) -> GenerationName {
        GenerationName::new(number).unwrap()
    }

    /// Stages `files` on `tftp` through the gate of an entry that enables
    /// no write.
    fn stage(tftp: &mut TftpDirectory, files: &[(&str, &[u8])]) -> Result<GenerationName, String> {
        let board = Board::parse(RASPBERRY_PI_5).unwrap();
        let files: Vec<_> = (files.iter())
            .map(|(file, bytes)| (name(file), bytes.to_vec()))
            .collect();
        match WriteGate::new(&WriteEnables::default(), &board).stage(tftp, &files) {
            Ok(generation) => Ok(generation),
            Err(StageError::Device(error)) => Err(error.detail),
            Err(refused) => panic!("{refused}"),
        }
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn the_tftp_directory_stages_whole_generations_and_switches_atomically() {
        let dir = scratch("switch");
        let releases = dir.join(RELEASES);
        fs::create_dir_all(releases.join("legacy-abc")).unwrap();
        fs::write(releases.join("legacy-abc/config.txt"), "old").unwrap();
        fs::create_dir(releases.join("g000002")).unwrap();
        fs::write(releases.join("g000002/config.txt"), "two").unwrap();
        symlink("releases/legacy-abc", dir.join("example")).unwrap();
        let mut tftp = tftp(&dir);
        // A new generation after every one present, whole, with the modes
        // of a served one; staging serves nothing.
        let first = [("config.txt", &b"a"[..]), ("overlays/x.dtbo", b"x")];
        assert_eq!(stage(&mut tftp, &first), Ok(generation(3)));
        for (path, expected) in [
            ("g000003", 0o755),
            ("g000003/overlays", 0o755),
            ("g000003/config.txt", 0o644),
            ("g000003/overlays/x.dtbo", 0o644),
        ] {
            assert_eq!(mode(&releases.join(path)), expected, "{path}");
        }
        let x = tftp.read_staged(&generation(3), &name("overlays/x.dtbo"));
        assert_eq!(x, Ok(b"x".to_vec()));
        assert_eq!(tftp.read_served(&name("config.txt")), Ok(b"old".to_vec()));
        // The switch replaces the link that another publisher left by a
        // relative one, and gives the generation served before.
        assert_eq!(tftp.switch_to(&generation(3)), Ok(None));
        let target = fs::read_link(dir.join("example")).unwrap();
        assert_eq!(target, Path::new("releases/g000003"));
        assert_eq!(tftp.read_served(&name("config.txt")), Ok(b"a".to_vec()));
        assert!(fs::symlink_metadata(dir.join(SWITCH)).is_err());
        assert_eq!(stage(&mut tftp, &[("config.txt", b"b")]), Ok(generation(4)));
        assert_eq!(tftp.switch_to(&generation(4)), Ok(Some(generation(3))));
        // A reader of the served file finds the one generation or the
        // other at every switch: the name is never missing and the bytes
        // are never mixed. On macOS a lookup that meets the rename of the
        // link at that instant can fail with EINVAL instead, which is why
        // the tool switches only while the board is off.
        let (start, done) = (Barrier::new(2), AtomicBool::new(false));
        let served = dir.join("example/config.txt");
        thread::scope(|scope| {
            let reader = scope.spawn(|| {
                start.wait();
                let reads: Vec<_> = (0..1000).map(|_| fs::read(&served)).collect();
                done.store(true, Ordering::SeqCst);
                reads
            });
            start.wait();
            loop {
                for number in [3, 4] {
                    assert!(tftp.switch_to(&generation(number)).is_ok());
                }
                if done.load(Ordering::SeqCst) {
                    break;
                }
            }
            let reads = reader.join().unwrap();
            for read in &reads {
                match read {
                    Ok(read) => assert!(read == b"a" || read == b"b", "{read:?}"),
                    Err(error) => {
                        let lookup = error.kind() == ErrorKind::InvalidInput;
                        assert!(cfg!(target_os = "macos") && lookup, "{error}");
                    }
                }
            }
            assert!(reads.iter().any(Result::is_ok));
        });
        // Every generation keeps its bytes, the other publisher's too.
        for (path, bytes) in [
            ("legacy-abc/config.txt", "old"),
            ("g000002/config.txt", "two"),
            ("g000003/config.txt", "a"),
            ("g000004/config.txt", "b"),
        ] {
            assert_eq!(fs::read_to_string(releases.join(path)).unwrap(), bytes);
        }
        drop(tftp);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// The variable under which [`staging_gives_its_modes_under_any_umask`]
    /// takes the part of its child process: the directory to stage into.
    const STAGE_INTO: &str = "KYTO_BENCH_TEST_STAGE_INTO";

    #[test]
    #[allow(
        clippy::disallowed_types,
        reason = "the umask is a state of the whole process, so sh sets it for this test binary run again"
    )]
    fn staging_gives_its_modes_under_any_umask() {
        if let Some(dir) = std::env::var_os(STAGE_INTO) {
            let dir = PathBuf::from(dir);
            fs::write(dir.join("unmoded"), "").unwrap();
            let files = [("config.txt", &b"a"[..]), ("overlays/x.dtbo", b"x")];
            assert_eq!(stage(&mut tftp(&dir), &files), Ok(generation(1)));
            return;
        }
        let dir = scratch("umask");
        let test = "drivers::tftp::tests::staging_gives_its_modes_under_any_umask";
        let child = std::process::Command::new("sh")
            .args(["-c", "umask 077 && exec \"$0\" --exact \"$1\" --quiet"])
            .arg(std::env::current_exe().unwrap())
            .arg(test)
            .env(STAGE_INTO, &dir)
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&child.stdout) + String::from_utf8_lossy(&child.stderr);
        assert!(
            child.status.success() && said.contains("1 passed"),
            "{said}"
        );
        // The child staged under the umask 077, which a file made without a
        // mode of its own shows.
        assert_eq!(mode(&dir.join("unmoded")), 0o600);
        for (path, expected) in [
            ("releases", 0o755),
            ("releases/g000001", 0o755),
            ("releases/g000001/overlays", 0o755),
            ("releases/g000001/config.txt", 0o644),
            ("releases/g000001/overlays/x.dtbo", 0o644),
        ] {
            assert_eq!(mode(&dir.join(path)), expected, "{path}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_tftp_directory_overwrites_nothing_and_names_no_local_value() {
        let dir = scratch("refusals");
        let files: [(&str, &[u8]); 1] = [("config.txt", b"1")];
        let mut details = Vec::new();
        let mut refused = |detail: String, expected: &str| {
            assert_eq!(detail, expected);
            details.push(detail);
        };
        // While one holds the directory, no other stages or switches there,
        // and the lock shows the bench in use.
        let delivery = format!("[local.delivery]\ndir = \"{}\"\n", dir.display());
        let model = ina228::MODELS[0].0;
        let bench = instance(REAL, model, "example/", &delivery);
        assert!(!in_use(&bench));
        let mut first = tftp(&dir);
        let staged = stage(&mut first, &files).unwrap();
        assert!(in_use(&bench));
        let mut second = tftp(&dir);
        let held = "a run or a publication holds the TFTP directory";
        refused(stage(&mut second, &files).unwrap_err(), held);
        // A generation that the driver did not stage is not switched to.
        let not_staged = "g000001 was not staged whole on this bench";
        refused(second.switch_to(&staged).unwrap_err().detail, not_staged);
        drop(first);
        assert!(!in_use(&bench));
        assert_eq!(stage(&mut second, &files), Ok(generation(2)));
        refused(second.switch_to(&staged).unwrap_err().detail, not_staged);
        // A served path that is no link is left as it is.
        fs::create_dir(dir.join("example")).unwrap();
        fs::write(dir.join("example/config.txt"), "mine").unwrap();
        let no_link = "the served path is not a symbolic link";
        refused(
            second.switch_to(&generation(2)).unwrap_err().detail,
            no_link,
        );
        assert_eq!(fs::read(dir.join("example/config.txt")).unwrap(), b"mine");
        // A file where the new link goes is not removed.
        fs::remove_dir_all(dir.join("example")).unwrap();
        fs::write(dir.join(SWITCH), "mine").unwrap();
        let taken = format!("the name of the new link, {SWITCH}, is taken");
        refused(second.switch_to(&generation(2)).unwrap_err().detail, &taken);
        assert_eq!(fs::read(dir.join(SWITCH)).unwrap(), b"mine");
        // A link left there by a switch that stopped before its rename is
        // replaced.
        fs::remove_file(dir.join(SWITCH)).unwrap();
        symlink("releases/g000001", dir.join(SWITCH)).unwrap();
        assert_eq!(second.switch_to(&generation(2)), Ok(None));
        let target = fs::read_link(dir.join("example")).unwrap();
        assert_eq!(target, Path::new("releases/g000002"));
        assert!(fs::symlink_metadata(dir.join(SWITCH)).is_err());
        // The generation that the link names is not staged again, also when
        // it is gone.
        fs::remove_file(dir.join("example")).unwrap();
        symlink("releases/g000003", dir.join("example")).unwrap();
        assert_eq!(stage(&mut second, &files), Ok(generation(4)));
        // No name is used again, in any letter case, nor one after the last.
        fs::create_dir(dir.join("releases/G999999")).unwrap();
        refused(
            stage(&mut second, &files).unwrap_err(),
            "no generation name is left",
        );
        // A releases that is a link, and a directory that is a file or is
        // not there, are refused.
        let other = scratch("refusals-other");
        symlink(dir.join(RELEASES), other.join(RELEASES)).unwrap();
        let own = "releases is not a directory of its own";
        refused(stage(&mut tftp(&other), &files).unwrap_err(), own);
        fs::write(other.join("file"), "").unwrap();
        for path in [other.join("file"), other.join("absent")] {
            let not_a_directory = "the TFTP directory is not a directory";
            refused(
                stage(&mut tftp(&path), &files).unwrap_err(),
                not_a_directory,
            );
        }
        // Nothing named a path or the link.
        for detail in details {
            let lower = detail.to_ascii_lowercase();
            assert!(
                !lower.contains("example") && !lower.contains("tftp-refusals"),
                "{detail}"
            );
            assert!(!detail.contains('/'), "{detail}");
        }
        drop(second);
        fs::remove_dir_all(&dir).unwrap();
        fs::remove_dir_all(&other).unwrap();
    }

    #[test]
    fn the_tftp_directory_settings_need_an_absolute_directory_and_a_one_directory_prefix() {
        let model = ina228::MODELS[0].0;
        let checked = |prefix: &str, local: &str| settings(&instance(REAL, model, prefix, local));
        let dir = "[local.delivery]\ndir = \"/srv/tftp/board\"\n";
        let served = |link: &str| TftpSettings {
            dir: "/srv/tftp/board".into(),
            link: link.into(),
        };
        assert_eq!(checked("example/", dir), Ok(served("example")));
        assert_eq!(checked("boot.d/", dir), Ok(served("boot.d")));
        for (local, error) in [
            ("", "[local.delivery]: missing field `dir`"),
            (
                "[local.delivery]\ndir = \"/srv\"\nport = \"/srv\"\n",
                "[local.delivery]: unknown field `port`, expected `dir`",
            ),
            ("[local.delivery]\ndir = \"srv/tftp\"\n", ABSOLUTE),
            ("[local.delivery]\ndir = \"/srv/\\u0000tftp\"\n", ABSOLUTE),
        ] {
            assert_eq!(checked("example/", local).unwrap_err(), error, "{local}");
        }
        for prefix in [
            "",
            "example",
            "a/b/",
            "/srv/",
            "releases/",
            "RELEASES/",
            "cur+r/",
            "..../",
        ] {
            assert_eq!(checked(prefix, dir).unwrap_err(), ONE_DIRECTORY, "{prefix}");
        }
    }
}
