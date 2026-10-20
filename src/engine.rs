use std::ffi::OsStr;
use std::num::NonZeroUsize;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread;

use rustix::fs::{AtFlags, Mode, OFlags, Stat, openat, statat, unlinkat};
use rustix::io::Errno;

use crate::overwrite::{Covers, overwrite};
use crate::plan::{Failure, Identity, Incomplete, Kind, Plan, Target};

/// How much a cover writes per call. Large enough that the cost of the call disappears against
/// the write itself, and small enough that each worker holds one buffer of it.
pub const COVER_CHUNK: usize = 1 << 20;

/// Random covers over each file before the zero cover, matching what `shred -uzfn 10` did.
pub const DEFAULT_PASSES: u32 = 10;

/// Files overwritten at the same time. The work is bound by the disk, so the best count belongs
/// to the device and not to the processor count. Measured over 120 files of 1 MiB on an NVMe
/// disk: 16 workers took 136 ms, 8 took 143 ms, 100 took 177 ms, and 4 took 216 ms.
pub const DEFAULT_JOBS: NonZeroUsize = NonZeroUsize::new(16).unwrap();

pub(crate) const CHANGED: &str = "changed after the plan was made, so blank refuses to touch it";

pub(crate) const STOPPED: &str = "stopped";

const NOT_REACHED: &str = "stopped before the run reached it";

#[derive(Clone, Copy)]
pub struct Options {
    /// Files overwritten at the same time.
    pub jobs: NonZeroUsize,
    /// Random covers over each file before the zero cover.
    pub passes: u32,
}

/// How far the run has got with one target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Start,
    Done,
    Failed,
}

pub enum Update<'a> {
    /// One target as the run reaches it, and again as it settles.
    Target {
        verb: &'static str,
        path: &'a Path,
        stage: Stage,
    },
    /// The work finished so far. A file weighs the bytes of all its covers and one unit for its
    /// unlink, and every other target weighs one unit. Only a destroyed target earns its last
    /// unit, so `done` reaches `total` only when the run destroyed everything.
    Progress { done: u64, total: u64 },
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

/// What a worker tells the calling thread about the file at one index of the plan.
enum Message {
    Start(usize),
    Covered(usize, u64),
    Settled(usize, Result<(), TargetError>),
}

/// Destroys every target the plan holds. Files are overwritten on `options.jobs` workers, then
/// the removals and the directories run on the calling thread, deepest directory first.
/// `observe` runs on the calling thread only.
///
/// Once `stop` is set, no further target starts and a cover in progress ends at its next write.
/// Every target is then destroyed, intact, or damaged, and the error names each one the run did
/// not destroy, including the ones the walk behind the plan could not read.
pub fn destroy(
    plan: &Plan,
    options: Options,
    stop: &AtomicBool,
    observe: &mut dyn FnMut(Update<'_>),
) -> Result<(), Incomplete> {
    let targets = plan.targets();
    let mut run = Tally::new(plan, &targets, options.passes, observe);
    let (files, rest) = targets.split_at(plan.files.len());
    overwrite_files(plan, files, options, stop, &mut run);
    let mut dirs = ParentDirs::new(plan);
    for (offset, target) in rest.iter().enumerate() {
        let index = files.len() + offset;
        if stop.load(Ordering::Relaxed) {
            run.skip(index, NOT_REACHED);
            continue;
        }
        run.start(index);
        let (flags, action) = match target.kind {
            Kind::Directory => (AtFlags::REMOVEDIR, "remove directory"),
            Kind::File | Kind::Removal => (AtFlags::empty(), "remove"),
        };
        let result = reach(plan, &mut dirs, target)
            .and_then(|(parent, name, planned)| remove(parent, name, planned, flags, action));
        run.settle(index, result);
    }
    run.finish(plan)
}

/// Hands the files to the workers through a shared index, so each worker takes the next file
/// when it is free and a slow file never holds up the rest.
fn overwrite_files(
    plan: &Plan,
    files: &[Target],
    options: Options,
    stop: &AtomicBool,
    run: &mut Tally<'_, '_>,
) {
    let next = AtomicUsize::new(0);
    let mut not_started = None;
    thread::scope(|scope| {
        let (sender, receiver) = mpsc::channel();
        for _ in 0..options.jobs.get().min(files.len()) {
            let sender = sender.clone();
            let worker = thread::Builder::new().spawn_scoped(scope, || {
                work(plan, files, &next, options.passes, stop, sender);
            });
            // The workers already running finish the files, so a host that refuses one more
            // thread costs speed and not the run.
            if let Err(error) = worker {
                not_started.get_or_insert(format!("start a worker: {error}"));
                break;
            }
        }
        drop(sender);
        for message in receiver {
            match message {
                Message::Start(index) => run.start(index),
                Message::Covered(index, bytes) => run.cover(index, bytes),
                Message::Settled(index, result) => run.settle(index, result),
            }
        }
    });
    let reason = not_started.unwrap_or_else(|| NOT_REACHED.to_owned());
    for index in 0..files.len() {
        if !run.settled[index] {
            run.skip(index, &reason);
        }
    }
}

/// Each worker holds its own buffer and its own chain of open directories.
fn work(
    plan: &Plan,
    files: &[Target],
    next: &AtomicUsize,
    passes: u32,
    stop: &AtomicBool,
    sender: Sender<Message>,
) {
    let mut dirs = ParentDirs::new(plan);
    let mut buf = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        let index = next.fetch_add(1, Ordering::Relaxed);
        let Some(target) = files.get(index) else {
            return;
        };
        // The calling thread holds the receiver until every worker returns, so a send fails
        // only after a panic there. Nothing would then record what the worker destroys.
        if sender.send(Message::Start(index)).is_err() {
            return;
        }
        let result = reach(plan, &mut dirs, target).and_then(|(parent, name, planned)| {
            let mut covers = Covers {
                passes,
                buf: &mut buf,
                stop,
                landed: &mut |bytes| {
                    let _ = sender.send(Message::Covered(index, bytes));
                },
            };
            overwrite(parent, name, planned, &mut covers)
        });
        let _ = sender.send(Message::Settled(index, result));
    }
}

fn reach<'d, 't>(
    plan: &Plan,
    dirs: &'d mut ParentDirs<'_>,
    target: &'t Target,
) -> Result<(BorrowedFd<'d>, &'t OsStr, Identity), TargetError> {
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
    Ok((parent, name, planned))
}

/// The state of a run as the calling thread sees it. It is the only owner of the progress and
/// the failures, so the workers share nothing but the channel.
struct Tally<'p, 'o> {
    targets: &'p [Target],
    /// What each target weighs, and what it has earned so far.
    weights: Vec<u64>,
    earned: Vec<u64>,
    settled: Vec<bool>,
    failures: Vec<(usize, Failure)>,
    done: u64,
    total: u64,
    observe: &'o mut dyn FnMut(Update<'_>),
}

impl<'p, 'o> Tally<'p, 'o> {
    fn new(
        plan: &Plan,
        targets: &'p [Target],
        passes: u32,
        observe: &'o mut dyn FnMut(Update<'_>),
    ) -> Tally<'p, 'o> {
        let weights: Vec<u64> = targets
            .iter()
            .map(|target| match target.kind {
                Kind::File => file_weight(plan.size(&target.path), passes),
                Kind::Removal | Kind::Directory => 1,
            })
            .collect();
        let total = weights
            .iter()
            .fold(0, |total: u64, weight| total.saturating_add(*weight));
        Tally {
            targets,
            earned: vec![0; weights.len()],
            settled: vec![false; weights.len()],
            weights,
            failures: Vec::new(),
            done: 0,
            total: total.max(1),
            observe,
        }
    }

    fn start(&mut self, index: usize) {
        self.event(index, Stage::Start);
    }

    /// Credits landed bytes, short of the last unit, which only the end of the target earns.
    fn cover(&mut self, index: usize, bytes: u64) {
        let room = self.weights[index] - 1 - self.earned[index];
        self.earn(index, bytes.min(room));
    }

    fn settle(&mut self, index: usize, result: Result<(), TargetError>) {
        self.settled[index] = true;
        let stage = match result {
            Ok(()) => {
                let rest = self.weights[index] - self.earned[index];
                self.earn(index, rest);
                Stage::Done
            }
            Err(error) => {
                self.earn(index, 0);
                let failure = Failure::new(&self.targets[index].path, error.reason, error.damaged);
                self.failures.push((index, failure));
                Stage::Failed
            }
        };
        self.event(index, stage);
    }

    fn skip(&mut self, index: usize, reason: &str) {
        self.settled[index] = true;
        let failure = Failure::new(&self.targets[index].path, reason.to_owned(), false);
        self.failures.push((index, failure));
    }

    fn earn(&mut self, index: usize, units: u64) {
        self.earned[index] += units;
        self.done = self.done.saturating_add(units).min(self.total);
        (self.observe)(Update::Progress {
            done: self.done,
            total: self.total,
        });
    }

    fn event(&mut self, index: usize, stage: Stage) {
        let target = &self.targets[index];
        (self.observe)(Update::Target {
            verb: target.verb(),
            path: &target.path,
            stage,
        });
    }

    /// The report, with the targets the walk could not read first and then the rest in plan
    /// order, so two runs over one tree list their survivors the same way.
    fn finish(mut self, plan: &Plan) -> Result<(), Incomplete> {
        self.failures.sort_by_key(|(index, _)| *index);
        let failures: Vec<Failure> = plan
            .failures()
            .iter()
            .cloned()
            .chain(self.failures.into_iter().map(|(_, failure)| failure))
            .collect();
        if failures.is_empty() {
            Ok(())
        } else {
            Err(Incomplete::new(failures))
        }
    }
}

fn file_weight(size: u64, passes: u32) -> u64 {
    size.saturating_mul(u64::from(passes) + 1).saturating_add(1)
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
