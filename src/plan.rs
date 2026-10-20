use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use crate::guard::{Guard, Refusal};
use crate::terminal::{escape, escape_path};

/// The wording of each kind of target. The dry run, the question and the run all read these, so
/// they cannot describe one target differently.
pub const VERB_FILE: &str = "Overwriting";
pub const VERB_REMOVAL: &str = "Removing";
pub const VERB_DIRECTORY: &str = "Deleting directory";

/// Past this many the report stops being readable.
const FAILURES_NAMED: usize = 10;

/// How many of the largest files a plan keeps for the operator to see.
const LARGEST_KEPT: usize = 5;

const SIZE_UNITS: [&str; 6] = ["KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];

/// Everything a run would destroy, in one bucket per way of destroying a target. A run acts on
/// nothing else, so what the operator is shown before it starts is what it works from.
#[derive(Default)]
pub struct Plan {
    roots: Vec<PathBuf>,
    /// The same roots, for a lookup that does not grow with the number of arguments a glob
    /// expands to.
    root_set: HashSet<PathBuf>,
    pub files: Vec<PathBuf>,
    /// Symlinks and special files. Each is unlinked and never opened.
    pub removals: Vec<PathBuf>,
    /// In walk order, each directory before what it holds.
    pub directories: Vec<PathBuf>,
    failures: Vec<Failure>,
    found: HashMap<PathBuf, Found>,
    /// The total size of the files, the largest of them, biggest first, and the directories
    /// nobody could list, all counted as the walk goes so the plan reads its statistics in one
    /// place.
    bytes: u64,
    largest: Vec<(PathBuf, u64)>,
    unreadable: Vec<PathBuf>,
}

/// One target as the walk found it.
#[derive(Clone, Copy)]
struct Found {
    /// The run checks the device and inode again before it acts, so a name later pointed at
    /// another file is refused. A file deleted and created again under the same name can
    /// receive the inode just freed, and passes.
    identity: Identity,
    size: u64,
}

/// What a run of a plan would destroy, and what it would leave because the walk could not read
/// it.
pub struct Statistics<'p> {
    pub files: usize,
    pub removals: usize,
    pub directories: usize,
    /// The total size of the files.
    pub bytes: u64,
    /// The largest files, biggest first.
    pub largest: &'p [(PathBuf, u64)],
    /// The directories the walk could not list. Each survives the run with every directory
    /// above it.
    pub unreadable: &'p [PathBuf],
}

impl Statistics<'_> {
    /// What the covers write: the size of every file, once for each random pass and once for
    /// the zero pass. No time is given, because the speed belongs to the device and a duration
    /// would be a guess, where the byte count is exact.
    pub fn cover_bytes(&self, passes: u32) -> u64 {
        self.bytes.saturating_mul(u64::from(passes) + 1)
    }

    /// The totals the dry run and the question both print, so the two read alike.
    pub fn write_totals(&self, out: &mut impl Write, passes: u32) -> io::Result<()> {
        let covers = passes.saturating_add(1);
        let noun = if covers == 1 { "pass" } else { "passes" };
        writeln!(
            out,
            "The files hold {}, and the covers will write {} in {covers} {noun}.",
            size(self.bytes),
            size(self.cover_bytes(passes)),
        )?;
        match self.unreadable.len() {
            0 => Ok(()),
            1 => writeln!(
                out,
                "1 directory cannot be read, and it will survive the run."
            ),
            count => writeln!(
                out,
                "{count} directories cannot be read, and they will survive the run."
            ),
        }
    }
}

/// A byte count in a binary unit, with the exact count beside it once the unit rounds it.
fn size(bytes: u64) -> String {
    if bytes < 1024 {
        return counted(
            usize::try_from(bytes).unwrap_or(usize::MAX),
            "byte",
            "bytes",
        );
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    // Rounded to one decimal, anything from 1023.95 up would read as 1024.0 of the smaller unit.
    while value >= 1023.95 && unit + 1 < SIZE_UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {} ({bytes} bytes)", SIZE_UNITS[unit])
}

pub struct Target {
    pub kind: Kind,
    pub path: PathBuf,
}

impl Target {
    pub fn verb(&self) -> &'static str {
        self.kind.verb()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    device: u64,
    inode: u64,
}

impl Identity {
    pub(crate) fn new(device: u64, inode: u64) -> Identity {
        Identity { device, inode }
    }
}

#[derive(Clone)]
pub struct Failure {
    path: PathBuf,
    reason: String,
    /// A cover landed before the run stopped, so the entry is still on the disk and the
    /// contents it held are gone. A caller reads an undestroyed target as an intact one,
    /// and this is the case where that is wrong.
    damaged: bool,
}

impl Failure {
    pub(crate) fn new(path: &Path, reason: String, damaged: bool) -> Failure {
        Failure {
            path: path.to_path_buf(),
            reason,
            damaged,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    pub fn damaged(&self) -> bool {
        self.damaged
    }
}

/// The targets a run did not destroy. The caller believes they are gone, so the report names
/// them rather than only counting them.
pub struct Incomplete {
    failures: Vec<Failure>,
}

impl Incomplete {
    pub(crate) fn new(failures: Vec<Failure>) -> Incomplete {
        Incomplete { failures }
    }

    /// Every target the run did not destroy, where the report names at most a few.
    pub fn survivors(&self) -> &[Failure] {
        &self.failures
    }
}

impl fmt::Display for Incomplete {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self.failures.len();
        let (noun, verb) = if count == 1 {
            ("target", "was")
        } else {
            ("targets", "were")
        };
        write!(f, "{count} {noun} {verb} not destroyed")?;
        for failure in self.failures.iter().take(FAILURES_NAMED) {
            let line = format!("{}: {}", failure.path.to_string_lossy(), failure.reason);
            let state = if failure.damaged {
                "damaged, the contents are gone"
            } else {
                "intact"
            };
            write!(f, "\n  {} ({state})", escape(&line))?;
        }
        if count > FAILURES_NAMED {
            write!(f, "\n  and {} more", count - FAILURES_NAMED)?;
        }
        Ok(())
    }
}

/// How a run destroys a target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Directory,
    /// A symlink or a special file. It is unlinked and never opened.
    Removal,
}

impl Kind {
    pub fn verb(self) -> &'static str {
        match self {
            Kind::File => VERB_FILE,
            Kind::Directory => VERB_DIRECTORY,
            Kind::Removal => VERB_REMOVAL,
        }
    }
}

impl Plan {
    /// Expands the arguments into everything a run would destroy. It reads the filesystem and
    /// changes nothing. Every argument passes the guard before any of them is expanded, so a
    /// command line holding one protected path plans nothing at all.
    pub fn enumerate(args: &[PathBuf], guard: &Guard) -> Result<Plan, Refusal> {
        let mut plan = Plan::default();
        let mut targets = Vec::with_capacity(args.len());
        for arg in args {
            let path = match absolute(arg) {
                Ok(path) => path,
                Err(error) => {
                    plan.fail(arg, format!("resolve path: {error}"));
                    continue;
                }
            };
            guard.check(&path)?;
            if plan.root_set.insert(path.clone()) {
                plan.roots.push(path.clone());
            }
            targets.push(path);
        }
        let mut seen = HashSet::new();
        for target in targets {
            plan.walk(target, &mut seen);
        }
        Ok(plan)
    }

    /// Collects the tree under `root` depth first. The stack holds the paths still to visit,
    /// with the next one on top, which gives the order a recursive walk gives without growing
    /// the call stack on a deep tree.
    ///
    /// Arguments overlap whenever a glob expands beside the directory holding it, so `seen`
    /// keeps a target from being planned twice.
    fn walk(&mut self, root: PathBuf, seen: &mut HashSet<PathBuf>) {
        let mut pending = vec![root];
        while let Some(path) = pending.pop() {
            if !seen.insert(path.clone()) {
                continue;
            }
            let kind = match classify(&path) {
                Ok((kind, found)) => {
                    self.found.insert(path.clone(), found);
                    kind
                }
                Err(error) => {
                    self.fail(&path, format!("read path: {error}"));
                    continue;
                }
            };
            match kind {
                Kind::File => {
                    self.count_file(&path);
                    self.files.push(path);
                }
                Kind::Removal => self.removals.push(path),
                // A directory nobody could list is not a target, because what it holds was never
                // shown.
                Kind::Directory => match sorted_names(&path) {
                    Ok(names) => {
                        pending.extend(names.into_iter().rev().map(|name| path.join(name)));
                        self.directories.push(path);
                    }
                    Err(error) => {
                        self.fail(&path, format!("read directory: {error}"));
                        self.unreadable.push(path);
                    }
                },
            }
        }
    }

    fn count_file(&mut self, path: &Path) {
        let size = self.size(path);
        self.bytes = self.bytes.saturating_add(size);
        if self.largest.len() == LARGEST_KEPT
            && self.largest.last().is_some_and(|&(_, least)| least >= size)
        {
            return;
        }
        let at = self.largest.partition_point(|&(_, kept)| kept >= size);
        self.largest.insert(at, (path.to_path_buf(), size));
        self.largest.truncate(LARGEST_KEPT);
    }

    pub fn statistics(&self) -> Statistics<'_> {
        Statistics {
            files: self.files.len(),
            removals: self.removals.len(),
            directories: self.directories.len(),
            bytes: self.bytes,
            largest: &self.largest,
            unreadable: &self.unreadable,
        }
    }

    fn fail(&mut self, path: &Path, reason: String) {
        self.failures.push(Failure::new(path, reason, false));
    }

    /// The arguments as absolute paths, each once, in the order they were named.
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    pub fn total(&self) -> usize {
        self.files.len() + self.removals.len() + self.directories.len()
    }

    /// Every target in the order the run reaches it: files, then removals, then directories
    /// deepest first.
    pub fn targets(&self) -> Vec<Target> {
        let files = self.files.iter().map(|path| (Kind::File, path));
        let removals = self.removals.iter().map(|path| (Kind::Removal, path));
        let directories = self
            .directories
            .iter()
            .rev()
            .map(|path| (Kind::Directory, path));
        files
            .chain(removals)
            .chain(directories)
            .map(|(kind, path)| Target {
                kind,
                path: path.clone(),
            })
            .collect()
    }

    pub(crate) fn identity(&self, path: &Path) -> Option<Identity> {
        self.found.get(path).map(|found| found.identity)
    }

    /// The length of a file as the walk found it, and zero for every other target.
    pub(crate) fn size(&self, path: &Path) -> u64 {
        self.found.get(path).map_or(0, |found| found.size)
    }

    /// Reports whether a run of this plan would destroy `path`, or the directory that holds it.
    /// The check compares the device and inode of the path and of each directory above it with
    /// the targets, so a second name for a planned file or directory is caught where the text
    /// of the two paths differs: a hard link, a symlink in a parent, or a different letter case
    /// on a file system that ignores case. The path is resolved as an open resolves it, and
    /// not cleaned by text, because a `..` after a symlink leaves the directory the link points
    /// at.
    pub fn contains(&self, path: &Path) -> bool {
        let planned = |path: &Path, kind: Kind| {
            classify(path).is_ok_and(|(found_kind, found)| {
                found_kind == kind
                    && self
                        .found
                        .values()
                        .any(|target| target.identity == found.identity)
            })
        };
        if planned(path, Kind::File) {
            return true;
        }
        let parent = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        fs::canonicalize(parent)
            .is_ok_and(|parent| parent.ancestors().any(|dir| planned(dir, Kind::Directory)))
    }

    pub(crate) fn failures(&self) -> &[Failure] {
        &self.failures
    }

    /// The directory a run opens by path to reach `target`: the parent of the outermost root
    /// that holds it. Everything below that directory is reached by descriptor, so a root named
    /// inside another root never lets the run open a selected directory by path.
    pub(crate) fn base_of<'t>(&self, target: &'t Path) -> Option<&'t Path> {
        target
            .ancestors()
            .filter(|ancestor| self.root_set.contains(*ancestor))
            .last()
            .and_then(Path::parent)
    }

    /// What the enumeration could not read, in the shape a run reports what it could not
    /// destroy.
    pub fn incomplete(&self) -> Option<Incomplete> {
        (!self.failures.is_empty()).then(|| Incomplete::new(self.failures.clone()))
    }

    /// Counts what a run would destroy. The dry run and the question read the same wording, so
    /// an operator who checks with one and then runs the other compares like with like.
    pub fn summary(&self) -> String {
        format!(
            "{}, {}, and {}",
            counted(self.files.len(), "file", "files"),
            counted(
                self.removals.len(),
                "symlink or special file",
                "symlinks and special files"
            ),
            counted(self.directories.len(), "directory", "directories"),
        )
    }

    /// Writes every target, one per line, in run order. Long output is the point: a count alone
    /// cannot show that a command is aimed at the wrong directory.
    pub fn write_dry_run(&self, out: &mut impl Write, passes: u32) -> io::Result<()> {
        for target in self.targets() {
            writeln!(out, "{} {}", target.verb(), escape_path(&target.path))?;
        }
        writeln!(out, "Dry run. {}. Nothing was destroyed.", self.summary())?;
        self.statistics().write_totals(out, passes)
    }
}

fn counted(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("{count} {singular}")
    } else {
        format!("{count} {plural}")
    }
}

/// Reads the path once, without following a symlink.
fn classify(path: &Path) -> io::Result<(Kind, Found)> {
    let metadata = fs::symlink_metadata(path)?;
    let found = Found {
        identity: Identity::new(metadata.dev(), metadata.ino()),
        size: metadata.len(),
    };
    let kind = metadata.file_type();
    let kind = if kind.is_symlink() {
        Kind::Removal
    } else if kind.is_dir() {
        Kind::Directory
    } else if kind.is_file() {
        Kind::File
    } else {
        // A FIFO, a socket or a device node. The overwrite opens its target, and opening one of
        // these blocks until a peer arrives.
        Kind::Removal
    };
    Ok((kind, found))
}

/// The operating system lists a directory in no fixed order, so the names are sorted and two
/// dry runs over one tree list it the same way.
fn sorted_names(dir: &Path) -> io::Result<Vec<OsString>> {
    let mut names = fs::read_dir(dir)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<io::Result<Vec<_>>>()?;
    names.sort();
    Ok(names)
}

/// Makes `arg` absolute and removes `.` and `..` by their text. `std::path::absolute` keeps a
/// `..`, and the guard compares components, so `home/project/..` would read as a path inside
/// home while the walk destroyed home itself.
fn absolute(arg: &Path) -> io::Result<PathBuf> {
    let joined = std::path::absolute(arg)?;
    let mut clean = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                clean.pop();
            }
            other => clean.push(other),
        }
    }
    Ok(clean)
}
