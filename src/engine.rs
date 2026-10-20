use std::ffi::OsStr;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::{Path, PathBuf};

use rustix::fs::{AtFlags, Mode, OFlags, Stat, openat, statat, unlinkat};
use rustix::io::Errno;

use crate::overwrite::overwrite;
use crate::plan::{Failure, Identity, Incomplete, Kind, Plan, Target};

/// How much a cover writes per call. Large enough that the cost of the call disappears against
/// the write itself, and small enough that each worker holds one buffer of it.
pub const COVER_CHUNK: usize = 1 << 20;

/// Random covers over each file before the zero cover, matching what `shred -uzfn 10` did.
pub const DEFAULT_PASSES: u32 = 10;

pub(crate) const CHANGED: &str = "changed after the plan was made, so blank refuses to touch it";

/// How far the run has got with one target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Start,
    Done,
    Failed,
}

/// One target as the run reaches it, and again as it settles.
pub struct Event<'a> {
    pub verb: &'static str,
    pub path: &'a Path,
    pub stage: Stage,
}

/// Why the run did not destroy one target, and whether a cover had already landed.
pub(crate) struct TargetError {
    reason: String,
    damaged: bool,
}

impl TargetError {
    pub(crate) fn intact(reason: String) -> TargetError {
        TargetError {
            reason,
            damaged: false,
        }
    }

    pub(crate) fn after(covered: u64, reason: String) -> TargetError {
        TargetError {
            reason,
            damaged: covered > 0,
        }
    }
}

/// `st_dev` is 32 bits on macOS and 64 on Linux, so the cast is a no-op on one of them.
#[allow(clippy::unnecessary_cast)]
pub(crate) fn identity_of(stat: &Stat) -> Identity {
    Identity::new(stat.st_dev as u64, stat.st_ino as u64)
}

/// The identity of the entry `name` itself, never of what a symlink there points at.
pub(crate) fn identity_at(parent: BorrowedFd<'_>, name: &OsStr) -> rustix::io::Result<Identity> {
    statat(parent, name, AtFlags::SYMLINK_NOFOLLOW).map(|stat| identity_of(&stat))
}

/// Destroys every target the plan holds, in plan order, and reports each one to `observe` as it
/// starts and again as it settles. The error names every target the run did not destroy,
/// including the ones the walk behind the plan could not read.
pub fn destroy(
    plan: &Plan,
    passes: u32,
    observe: &mut dyn FnMut(Event<'_>),
) -> Result<(), Incomplete> {
    let mut failures = plan.failures().to_vec();
    let mut dirs = ParentDirs::new(plan);
    for target in plan.targets() {
        let verb = target.verb();
        observe(Event {
            verb,
            path: &target.path,
            stage: Stage::Start,
        });
        let stage = match destroy_target(plan, &mut dirs, &target, passes) {
            Ok(()) => Stage::Done,
            Err(error) => {
                failures.push(Failure::new(&target.path, error.reason, error.damaged));
                Stage::Failed
            }
        };
        observe(Event {
            verb,
            path: &target.path,
            stage,
        });
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(Incomplete::new(failures))
    }
}

fn destroy_target(
    plan: &Plan,
    dirs: &mut ParentDirs<'_>,
    target: &Target,
    passes: u32,
) -> Result<(), TargetError> {
    let planned = plan
        .identity(&target.path)
        .expect("the walk records the identity of every target it plans");
    let name = target
        .path
        .file_name()
        .expect("the guard refuses the root, the only path with no name");
    let parent = dirs
        .open_parent(&target.path)
        .map_err(TargetError::intact)?;
    match target.kind {
        Kind::File => overwrite(parent, name, planned, passes),
        Kind::Removal => remove(parent, name, planned, AtFlags::empty(), "remove"),
        Kind::Directory => remove(
            parent,
            name,
            planned,
            AtFlags::REMOVEDIR,
            "remove directory",
        ),
    }
}

/// Unlinks one entry, and never what it holds or points at. The system refuses to remove a
/// directory that still holds anything, which is what turns a target the run failed to destroy
/// into a reported failure, rather than unlinking it with its contents still on the disk.
fn remove(
    parent: BorrowedFd<'_>,
    name: &OsStr,
    planned: Identity,
    flags: AtFlags,
    action: &str,
) -> Result<(), TargetError> {
    match identity_at(parent, name) {
        Ok(found) if found == planned => {}
        Ok(_) => return Err(TargetError::intact(CHANGED.to_owned())),
        Err(Errno::NOENT) => return Ok(()),
        Err(error) => return Err(TargetError::intact(format!("{action}: {error}"))),
    }
    unlinkat(parent, name, flags).map_err(|error| TargetError::intact(format!("{action}: {error}")))
}

/// The directories a run acts inside. The parent of a root is opened by path, because the guard
/// has already checked it and it lies outside what the operator selected. Every directory below
/// it is opened from the one above without following a symlink, so a directory moved out of the
/// tree and replaced by a link leads nowhere. Each action then checks the identity of the entry
/// it acts on, which catches every other replacement.
///
/// Targets arrive in walk order, so the chain only grows and shrinks at its end, and it holds
/// one descriptor per level of the tree.
struct ParentDirs<'p> {
    plan: &'p Plan,
    chain: Vec<(PathBuf, OwnedFd)>,
}

impl<'p> ParentDirs<'p> {
    fn new(plan: &'p Plan) -> ParentDirs<'p> {
        ParentDirs {
            plan,
            chain: Vec::new(),
        }
    }

    fn open_parent(&mut self, target: &Path) -> Result<BorrowedFd<'_>, String> {
        let parent = target
            .parent()
            .expect("the guard refuses the root, the only path with no parent");
        let base = self
            .plan
            .base_of(target)
            .expect("every target lies under one of the roots");
        if self.chain.first().is_some_and(|(path, _)| path != base) {
            self.chain.clear();
        }
        while self
            .chain
            .last()
            .is_some_and(|(path, _)| !parent.starts_with(path))
        {
            self.chain.pop();
        }
        if self.chain.is_empty() {
            let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
            let fd = rustix::fs::open(base, flags, Mode::empty())
                .map_err(|error| format!("open {}: {error}", base.to_string_lossy()))?;
            self.chain.push((base.to_path_buf(), fd));
        }
        let (reached, _) = self.chain.last().expect("the chain holds the base");
        let below = parent
            .strip_prefix(reached)
            .expect("the chain ends at the parent or above it")
            .to_path_buf();
        for component in below.components() {
            let (path, fd) = self.chain.last().expect("the chain holds the base");
            let next = path.join(component);
            let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
            let opened = openat(fd, component.as_os_str(), flags, Mode::empty())
                .map_err(|error| format!("open {}: {error}", next.to_string_lossy()))?;
            self.chain.push((next, opened));
        }
        let (_, fd) = self.chain.last().expect("the chain holds the base");
        Ok(fd.as_fd())
    }
}
