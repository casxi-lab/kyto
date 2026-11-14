//! The files that a run reads: the run plan, the instance configuration and
//! the source files of each payload. Each is read once, into memory; the
//! SHA-256 that a receipt records is that of the bytes read, and the
//! document is read from the same bytes, so the two cannot differ. The board
//! description is one that the tool holds, recorded with its own SHA-256.

use std::fs;
use std::path::{Path, PathBuf};

use kyto_bench_core::board::Board;
use kyto_bench_core::digest::Sha256;
use kyto_bench_core::instance::{Instance, PinnedFile};
use kyto_bench_core::plan::{Entry, RunPlan};

/// What a run reads before it opens the bench.
#[derive(Clone, Debug)]
pub struct Inputs {
    pub plan: RunPlan,
    pub plan_sha256: Sha256,
    pub instance: Instance,
    pub instance_sha256: Sha256,
    /// The directory of the instance configuration, which its local
    /// settings name paths relative to.
    pub instance_dir: PathBuf,
    /// The description of the board that the plan and the instance name.
    pub board: Board,
    /// The directory that holds the payload directory of each entry.
    pub payload_root: PathBuf,
}

/// The reason given when a run plan holds a value to hide.
const IN_PLAN: &str = "the run plan holds this value to hide; choose another";

impl Inputs {
    /// Reads the run plan at `plan` and the instance configuration at
    /// `instance`, takes the description of the plan's board from `boards`,
    /// and checks them together: the instance is for that board and passes
    /// its checks against the description, the plan passes its checks
    /// against the description and the pinned sets, and no value to hide of
    /// the instance occurs in the plan, which a receipt records verbatim.
    pub fn read(
        plan: &Path,
        instance: &Path,
        payload_root: &Path,
        boards: &[&str],
    ) -> Result<Self, String> {
        let (plan_sha256, text) = read_text(plan)?;
        let run_plan = RunPlan::parse(&text).map_err(|error| at(plan, error))?;
        let (instance_sha256, text) = read_text(instance)?;
        let config = Instance::parse(&text).map_err(|error| at(instance, error))?;
        let board = board(boards, &run_plan.board)?;
        config
            .check_board(&board)
            .map_err(|error| at(instance, error))?;
        let pins = &config.parameters().pins;
        run_plan
            .check(&board, pins)
            .map_err(|error| at(plan, error))?;
        config
            .check_text(run_plan.text(), IN_PLAN)
            .map_err(|error| at(instance, error))?;
        Ok(Self {
            plan: run_plan,
            plan_sha256,
            instance: config,
            instance_sha256,
            instance_dir: instance.parent().map(Path::to_path_buf).unwrap_or_default(),
            board,
            payload_root: payload_root.to_path_buf(),
        })
    }

    /// The payload files of `entry`: its own or those of its pinned set.
    pub fn payload<'a>(&'a self, entry: &'a Entry) -> &'a [PinnedFile] {
        let pins = &self.instance.parameters().pins;
        entry.files(pins).unwrap_or_default()
    }

    /// The bytes of each source file of the payload of `entry`, read once
    /// from its payload directory below the payload root, or `None` for a
    /// file that cannot be read.
    pub fn sources(&self, entry: &Entry) -> Vec<Option<Vec<u8>>> {
        let dir = self.payload_root.join(entry.payload_dir.as_str());
        let read = |file: &PinnedFile| fs::read(dir.join(file.name.as_str())).ok();
        self.payload(entry).iter().map(read).collect()
    }
}

/// An instance configuration read on its own, for a command without a run
/// plan.
#[derive(Clone, Debug)]
pub struct InstanceInputs {
    pub instance: Instance,
    pub instance_sha256: Sha256,
    /// The directory of the instance configuration.
    pub instance_dir: PathBuf,
    /// The description of the board that the instance names.
    pub board: Board,
}

/// Reads the instance configuration at `instance`, takes the description of
/// its board from `boards` and checks the instance against it, as
/// [`Inputs::read`] does.
pub fn read_instance(instance: &Path, boards: &[&str]) -> Result<InstanceInputs, String> {
    let (instance_sha256, text) = read_text(instance)?;
    let config = Instance::parse(&text).map_err(|error| at(instance, error))?;
    let board = board(boards, config.board())?;
    config
        .check_board(&board)
        .map_err(|error| at(instance, error))?;
    Ok(InstanceInputs {
        instance: config,
        instance_sha256,
        instance_dir: instance.parent().map(Path::to_path_buf).unwrap_or_default(),
        board,
    })
}

/// The description in `boards` of the board `id`.
fn board(boards: &[&str], id: &str) -> Result<Board, String> {
    let mut parsed = boards.iter().filter_map(|text| Board::parse(text).ok());
    let board = parsed.find(|board| board.id == id);
    board.ok_or_else(|| format!("no board description of this tool has the id {id}"))
}

/// `error` in the file at `path`.
fn at(path: &Path, error: impl std::fmt::Display) -> String {
    format!("{}: {error}", path.display())
}

/// The SHA-256 and the text of the file at `path`, read once.
fn read_text(path: &Path) -> Result<(Sha256, String), String> {
    let bytes =
        fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let sha256 = Sha256::of(&bytes);
    let text = String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8", path.display()))?;
    Ok((sha256, text))
}

#[cfg(test)]
mod tests {
    use kyto_bench_core::board::EMBEDDED;

    use super::*;

    #[test]
    fn an_instance_is_read_on_its_own_with_the_description_of_its_board() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/instance.toml");
        let read = read_instance(&path, &EMBEDDED).unwrap();
        assert_eq!(read.board.id, "raspberry-pi-5");
        assert_eq!(read.instance.board(), "raspberry-pi-5");
        assert_eq!(read.instance_sha256, Sha256::of(&fs::read(&path).unwrap()));
        assert_eq!(read.instance_dir, path.parent().unwrap());
        let unknown = read_instance(&path, &[]).unwrap_err();
        assert_eq!(
            unknown,
            "no board description of this tool has the id raspberry-pi-5"
        );
        let absent = read_instance(&path.with_file_name("absent.toml"), &EMBEDDED);
        assert!(absent.unwrap_err().starts_with("cannot read "));
    }
}
