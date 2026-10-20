use std::fs;
use std::num::NonZeroUsize;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use blank::engine::{COVER_CHUNK, DEFAULT_JOBS, Options, Stage, Update, destroy};
use blank::guard::Guard;
use blank::plan::Plan;
use tempfile::TempDir;

struct Run {
    report: Result<(), String>,
    /// Every target the run did not destroy, with its reason and whether it is damaged, where
    /// the report names only the first few.
    survivors: Vec<(PathBuf, String, bool)>,
    events: Vec<(&'static str, PathBuf, Stage)>,
    /// Each progress update as the share of the work done.
    progress: Vec<(u64, u64)>,
}

fn plan_of(args: &[&Path]) -> Plan {
    let args: Vec<PathBuf> = args.iter().map(|arg| arg.to_path_buf()).collect();
    Plan::enumerate(&args, &Guard::for_host()).expect("no case here names a protected path")
}

fn options(jobs: usize, passes: u32) -> Options {
    Options {
        jobs: NonZeroUsize::new(jobs).expect("a test asks for at least one worker"),
        passes,
    }
}

/// Runs the plan, and calls `react` on each update after it is recorded, so a test can stop the
/// run at a chosen point.
fn run_reacting(
    plan: &Plan,
    options: Options,
    stop: &AtomicBool,
    mut react: impl FnMut(&Update<'_>),
) -> Run {
    let mut events = Vec::new();
    let mut progress = Vec::new();
    let mut survivors = Vec::new();
    let report = destroy(plan, options, stop, &mut |update| {
        match &update {
            Update::Target { verb, path, stage } => {
                events.push((*verb, path.to_path_buf(), *stage));
            }
            Update::Progress { done, total } => progress.push((*done, *total)),
        }
        react(&update);
    })
    .map_err(|incomplete| {
        for failure in incomplete.survivors() {
            let path = failure.path().to_path_buf();
            survivors.push((path, failure.reason().to_owned(), failure.damaged()));
        }
        incomplete.to_string()
    });
    Run {
        report,
        survivors,
        events,
        progress,
    }
}

fn run_plan(plan: &Plan, passes: u32) -> Run {
    let jobs = DEFAULT_JOBS.get();
    run_reacting(plan, options(jobs, passes), &AtomicBool::new(false), |_| {})
}

fn run(args: &[&Path]) -> Run {
    run_plan(&plan_of(args), 1)
}

fn stages(run: &Run, path: &Path) -> Vec<Stage> {
    run.events
        .iter()
        .filter(|(_, at, _)| at == path)
        .map(|(_, _, stage)| *stage)
        .collect()
}

/// The report line for one path, which says what state the run left it in.
fn report_line<'r>(run: &'r Run, path: &Path) -> &'r str {
    let report = run.report.as_ref().expect_err("the run reports a survivor");
    let named = format!("{}: ", path.to_str().unwrap());
    report
        .lines()
        .find(|line| line.trim_start().starts_with(&named))
        .unwrap_or_else(|| panic!("the report does not name {}:\n{report}", path.display()))
}

/// Whether the run left `path` damaged, from the whole survivor list.
fn damaged(run: &Run, path: &Path) -> bool {
    run.survivors
        .iter()
        .find(|(at, _, _)| at == path)
        .unwrap_or_else(|| panic!("the run does not name {} as a survivor", path.display()))
        .2
}

/// Hardlinks `path` into a directory the run never touches, so a test can read the bytes after
/// the run has unlinked the name.
fn witness(path: &Path) -> (TempDir, PathBuf) {
    let keep = tempfile::tempdir().unwrap();
    let link = keep.path().join("witness");
    fs::hard_link(path, &link).unwrap();
    (keep, link)
}

/// Denies writes to a directory, which fails the rename that ends an overwrite while the open
/// and the covers still succeed. The mode comes back on drop, so the temporary directory can be
/// removed.
struct Sealed(PathBuf);

impl Sealed {
    fn new(dir: &Path) -> Option<Sealed> {
        if nix::unistd::geteuid().is_root() {
            // Root ignores the directory mode this relies on.
            return None;
        }
        fs::set_permissions(dir, fs::Permissions::from_mode(0o500)).unwrap();
        Some(Sealed(dir.to_path_buf()))
    }
}

impl Drop for Sealed {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
    }
}

fn gone(path: &Path) -> bool {
    path.symlink_metadata().is_err()
}

/// A destroyed file leaves zeros on the device and no name behind.
///
/// Given: a file of known contents, with a second link to its contents kept elsewhere.
/// When: it is destroyed with two random covers.
/// Then: the contents are zeros at the original length, and the name is gone.
///
/// Why it matters: this is the one promise the tool makes, and the zero cover is the last
/// thing written, so anything else left in the file means a pass did not reach it.
#[test]
fn a_destroyed_file_leaves_zeros_and_no_name() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("secret.txt");
    let original = "SENSITIVE".repeat(1024);
    fs::write(&path, &original).unwrap();
    let (_keep, seen) = witness(&path);

    // When:
    let run = run_plan(&plan_of(&[&path]), 2);

    // Then:
    assert_eq!(run.report, Ok(()));
    let left = fs::read(&seen).unwrap();
    assert_eq!(left.len(), original.len());
    assert!(
        left.iter().all(|&byte| byte == 0),
        "a byte other than zero survived"
    );
    assert!(gone(&path));
}

/// A file larger than one write is covered to its last byte.
///
/// Given: a file three and a half writes long, so the last write is partial.
/// When: it is destroyed.
/// Then: no byte of the original value is left, the last one included.
///
/// Why it matters: the chunk loop is where an off-by-one leaves the tail of a large file
/// untouched, and the tail is as recoverable as the head.
#[test]
fn a_file_larger_than_one_write_is_covered_to_the_last_byte() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("big.bin");
    let size = COVER_CHUNK * 3 + COVER_CHUNK / 2;
    fs::write(&path, vec![0xAB; size]).unwrap();
    let (_keep, seen) = witness(&path);

    // When:
    let run = run(&[&path]);

    // Then:
    assert_eq!(run.report, Ok(()));
    let left = fs::read(&seen).unwrap();
    assert_eq!(left.len(), size);
    assert_eq!(left.iter().position(|&byte| byte == 0xAB), None);
}

/// An empty file and a read-only file are destroyed like any other.
///
/// Given: a file of no bytes and a file with no write permission.
/// When: both are destroyed.
/// Then: the run reports success and both names are gone.
///
/// Why it matters: an empty file runs the write loop zero times, and a read-only file refuses
/// the first open. Both are common, and refusing either leaves the caller with a failure they
/// can do nothing about.
#[test]
fn an_empty_file_and_a_read_only_file_are_destroyed() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let empty = root.path().join("empty.txt");
    fs::write(&empty, "").unwrap();
    let read_only = root.path().join("read-only.txt");
    fs::write(&read_only, "payload").unwrap();
    fs::set_permissions(&read_only, fs::Permissions::from_mode(0o400)).unwrap();

    // When:
    let run = run(&[&empty, &read_only]);

    // Then:
    assert_eq!(run.report, Ok(()));
    assert!(gone(&empty));
    assert!(gone(&read_only));
}

/// A symlink is unlinked, and the file it points at is left alone.
///
/// Given: a file the run is not given, a symlink to it named as an argument, and a tree holding
/// another symlink to it.
/// When: the run finishes.
/// Then: both links are gone and the file still holds its bytes.
///
/// Why it matters: an overwrite through a link destroys a file nobody named and still reports
/// success.
#[test]
fn a_symlink_is_unlinked_and_its_target_left_alone() {
    // Given:
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("not-named.txt");
    fs::write(&target, "the caller never named this file").unwrap();
    let links = tempfile::tempdir().unwrap();
    let argument = links.path().join("link");
    symlink(&target, &argument).unwrap();
    let tree = tempfile::tempdir().unwrap();
    let inside = tree.path().join("link");
    symlink(&target, &inside).unwrap();

    // When:
    let run = run(&[&argument, tree.path()]);

    // Then:
    assert_eq!(run.report, Ok(()));
    assert!(gone(&argument));
    assert!(gone(&inside));
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "the caller never named this file"
    );
}

/// A special file is removed without ever being opened.
///
/// Given: a tree holding a FIFO beside an ordinary file.
/// When: the run finishes.
/// Then: it reports no failure, the FIFO is gone, and the ordinary file was still overwritten.
///
/// Why it matters: opening a FIFO for writing waits for a reader that never comes, so one named
/// pipe anywhere in a tree would stop the whole run.
#[test]
fn a_special_file_is_removed_without_an_overwrite() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let ordinary = root.path().join("ordinary.txt");
    fs::write(&ordinary, "payload").unwrap();
    let (_keep, seen) = witness(&ordinary);
    let pipe = root.path().join("pipe");
    nix::unistd::mkfifo(&pipe, nix::sys::stat::Mode::S_IRWXU).unwrap();

    // When:
    let run = run(&[root.path()]);

    // Then:
    assert_eq!(run.report, Ok(()));
    assert!(gone(&pipe));
    assert!(fs::read(&seen).unwrap().iter().all(|&byte| byte == 0));
}

/// The report separates a target the covers reached from one they never touched.
///
/// Given: a file in a directory that refuses the rename, and a path with nothing at it.
/// When: both are destroyed.
/// Then: the covered file is reported as damaged and the missing path as intact.
///
/// Why it matters: a caller reads a target the run did not destroy as one that still holds its
/// contents, and a file the covers reached holds random bytes under its own name.
#[test]
fn the_report_separates_a_damaged_target_from_an_intact_one() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let covered = root.path().join("secret.txt");
    fs::write(&covered, "payload").unwrap();
    let absent = root.path().join("absent.txt");
    let Some(_sealed) = Sealed::new(root.path()) else {
        return;
    };

    // When:
    let run = run(&[&covered, &absent]);

    // Then:
    assert!(
        report_line(&run, &covered).ends_with("(damaged, the contents are gone)"),
        "{}",
        report_line(&run, &covered)
    );
    assert!(
        report_line(&run, &absent).ends_with("(intact)"),
        "{}",
        report_line(&run, &absent)
    );
}

/// A file the run cannot finish is reported, and the run goes on to the rest.
///
/// Given: an ordinary file, and a file in a directory that refuses the rename.
/// When: both directories are destroyed.
/// Then: the run names the second file, the first is gone, and each target reports its start
/// and then its own outcome.
///
/// Why it matters: counting a failed target as done lets the directory pass unlink it with its
/// name and its state unreported, and a row that settles as done reads as destroyed.
#[test]
fn each_target_reports_its_own_outcome() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let ordinary = root.path().join("ordinary.txt");
    fs::write(&ordinary, "payload").unwrap();
    let sealed = tempfile::tempdir().unwrap();
    let stubborn = sealed.path().join("refuses.txt");
    fs::write(&stubborn, "payload").unwrap();
    let Some(_seal) = Sealed::new(sealed.path()) else {
        return;
    };

    // When:
    let run = run(&[root.path(), sealed.path()]);

    // Then:
    report_line(&run, &stubborn);
    assert!(gone(&ordinary));
    assert_eq!(stages(&run, &ordinary), [Stage::Start, Stage::Done]);
    assert_eq!(stages(&run, &stubborn), [Stage::Start, Stage::Failed]);
    assert_eq!(stages(&run, root.path()), [Stage::Start, Stage::Done]);
}

/// The run reaches every target in plan order and leaves nothing of the tree.
///
/// Given: a tree three directories deep with a file at the bottom and a symlink at the top.
/// When: the run finishes.
/// Then: the file comes first, then the symlink, then each directory after the one inside it,
/// and the tree is gone.
///
/// Why it matters: a directory can only be removed once it is empty, so any other order fails
/// on every tree deeper than one level and reports failures for a tree it could destroy.
#[test]
fn the_run_follows_the_plan_order_and_leaves_nothing() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let middle = root.path().join("middle");
    let deep = middle.join("deep");
    fs::create_dir_all(&deep).unwrap();
    let payload = deep.join("payload.txt");
    fs::write(&payload, "payload").unwrap();
    let link = root.path().join("link");
    symlink(&payload, &link).unwrap();

    // When:
    let run = run(&[root.path()]);

    // Then:
    assert_eq!(run.report, Ok(()));
    let reached: Vec<PathBuf> = run
        .events
        .iter()
        .filter(|(_, _, stage)| *stage == Stage::Start)
        .map(|(_, path, _)| path.clone())
        .collect();
    assert_eq!(
        reached,
        [payload, link, deep, middle, root.path().to_path_buf()]
    );
    assert!(gone(root.path()));
}

/// A directory the walk cannot list is reported, and nothing inside it is lost.
///
/// Given: a tree holding a directory nobody can list, with a file inside it.
/// When: the run finishes.
/// Then: it names the directory, the file inside still holds its contents, and every directory
/// above it is still there.
///
/// Why it matters: the walk never queued that file, so the run cannot have overwritten it, and
/// removing the directories around it would unlink it with its contents intact.
#[test]
fn a_directory_the_walk_cannot_list_survives_with_its_contents() {
    if nix::unistd::geteuid().is_root() {
        return;
    }
    // Given:
    let root = tempfile::tempdir().unwrap();
    let locked = root.path().join("locked");
    fs::create_dir(&locked).unwrap();
    let hidden = locked.join("hidden.txt");
    fs::write(&hidden, "still-here").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    // When:
    let run = run(&[root.path()]);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();

    // Then:
    assert!(report_line(&run, &locked).contains("read directory"));
    assert_eq!(fs::read_to_string(&hidden).unwrap(), "still-here");
    assert!(root.path().is_dir());
}

/// A directory swapped for a symlink after the plan is never followed.
///
/// Given: a planned tree whose subdirectory is then moved away and replaced by a symlink to an
/// outside directory holding a file of the same name.
/// When: the run finishes.
/// Then: the outside file still holds its bytes, the moved directory is untouched, and the run
/// reports the targets it refused.
///
/// Why it matters: anyone who can write in the tree can make this swap between the question
/// and the run, and a run that resolves the planned path again destroys whatever it now points
/// at.
#[test]
fn a_directory_swapped_for_a_symlink_after_the_plan_is_not_followed() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let sub = root.path().join("sub");
    fs::create_dir(&sub).unwrap();
    let planned = sub.join("x.txt");
    fs::write(&planned, "planned").unwrap();
    let plan = plan_of(&[root.path()]);
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("x.txt"), "outside").unwrap();
    let moved = root.path().join("moved");
    fs::rename(&sub, &moved).unwrap();
    symlink(outside.path(), &sub).unwrap();

    // When:
    let run = run_plan(&plan, 1);

    // Then:
    assert_eq!(
        fs::read_to_string(outside.path().join("x.txt")).unwrap(),
        "outside"
    );
    assert_eq!(fs::read_to_string(moved.join("x.txt")).unwrap(), "planned");
    assert!(report_line(&run, &planned).ends_with("(intact)"));
    report_line(&run, &sub);
}

/// A name pointed at an outside file after the plan never reaches that file.
///
/// Given: two planned files, whose names are then replaced, one by a hard link to a file outside
/// the tree and one by a symlink to it.
/// When: the run finishes.
/// Then: the outside file still holds its bytes, and both names are reported as intact.
///
/// Why it matters: a hard link is the file itself, so a run that destroys whatever holds the
/// name would overwrite the outside file through it. Its inode is not the planned one, which is
/// what the run checks. A name deleted and created again is not this case: the system may hand
/// the new file the inode it just freed, and the identity check cannot tell the two apart.
#[test]
fn a_name_pointed_at_an_outside_file_never_reaches_it() {
    // Given:
    let outside = tempfile::tempdir().unwrap();
    let victim = outside.path().join("victim.txt");
    fs::write(&victim, "outside").unwrap();
    let root = tempfile::tempdir().unwrap();
    let hard = root.path().join("hard.txt");
    let soft = root.path().join("soft.txt");
    fs::write(&hard, "planned").unwrap();
    fs::write(&soft, "planned").unwrap();
    let plan = plan_of(&[&hard, &soft]);
    fs::remove_file(&hard).unwrap();
    fs::hard_link(&victim, &hard).unwrap();
    fs::remove_file(&soft).unwrap();
    symlink(&victim, &soft).unwrap();

    // When:
    let run = run_plan(&plan, 1);

    // Then:
    assert_eq!(fs::read_to_string(&victim).unwrap(), "outside");
    assert!(report_line(&run, &hard).ends_with("(intact)"));
    assert!(report_line(&run, &soft).ends_with("(intact)"));
}

/// A directory moved out of the tree after the plan is not reached through a link left behind.
///
/// Given: a planned tree whose subdirectory is then moved outside the tree and replaced by a
/// symlink to its new place.
/// When: the run finishes.
/// Then: the file in the moved directory still holds its bytes and is reported as intact.
///
/// Why it matters: every identity behind that link still matches the plan, so only refusing to
/// follow a link keeps the run from renaming and unlinking entries outside the tree.
#[test]
fn a_directory_moved_out_of_the_tree_is_not_reached_through_a_link() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let sub = root.path().join("sub");
    fs::create_dir(&sub).unwrap();
    let planned = sub.join("x.txt");
    fs::write(&planned, "planned").unwrap();
    let plan = plan_of(&[root.path()]);
    let outside = tempfile::tempdir().unwrap();
    let moved = outside.path().join("sub");
    fs::rename(&sub, &moved).unwrap();
    symlink(&moved, &sub).unwrap();

    // When:
    let run = run_plan(&plan, 1);

    // Then:
    assert_eq!(fs::read_to_string(moved.join("x.txt")).unwrap(), "planned");
    assert!(report_line(&run, &planned).ends_with("(intact)"));
}

/// Naming a file inside a selected directory as well never opens a path the walk could not.
///
/// Given: a tree and a file deep inside it, both named, whose directory is then moved outside the
/// tree and replaced by a symlink to its new place.
/// When: the run finishes.
/// Then: the file in the moved directory still holds its bytes.
///
/// Why it matters: a glob names a directory and the files inside it together, and a run that
/// opens the inner path by name follows the link that refusing to follow links exists to stop.
#[test]
fn a_file_named_inside_a_named_tree_is_reached_from_the_outer_root() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let sub = root.path().join("sub");
    fs::create_dir(&sub).unwrap();
    let planned = sub.join("x.txt");
    fs::write(&planned, "planned").unwrap();
    let plan = plan_of(&[root.path(), &planned]);
    let outside = tempfile::tempdir().unwrap();
    let moved = outside.path().join("sub");
    fs::rename(&sub, &moved).unwrap();
    symlink(&moved, &sub).unwrap();

    // When:
    let run = run_plan(&plan, 1);

    // Then:
    assert_eq!(fs::read_to_string(moved.join("x.txt")).unwrap(), "planned");
    assert!(report_line(&run, &planned).ends_with("(intact)"));
}

fn files_in(root: &Path, count: usize, contents: &[u8]) -> Vec<PathBuf> {
    (0..count)
        .map(|index| {
            let path = root.join(format!("file-{index:02}.bin"));
            fs::write(&path, contents).unwrap();
            path
        })
        .collect()
}

/// The workers never run more targets at once than they were told to.
///
/// Given: many more files than the limit allows in flight.
/// When: the run finishes, with one worker and then with four.
/// Then: counting up on each start and down on each settle never passes the limit.
///
/// Why it matters: every target in flight is an open overwrite of a real file, so an unbounded
/// pool starts hundreds at once, thrashes the disk, and holds a buffer for each.
#[test]
fn the_workers_never_run_more_targets_than_the_limit() {
    for jobs in [1, 4] {
        // Given:
        let root = tempfile::tempdir().unwrap();
        files_in(root.path(), 24, b"payload");

        // When:
        let stop = AtomicBool::new(false);
        let run = run_reacting(&plan_of(&[root.path()]), options(jobs, 1), &stop, |_| {});

        // Then:
        assert_eq!(run.report, Ok(()), "jobs {jobs}");
        let (mut live, mut peak) = (0, 0);
        for (_, _, stage) in &run.events {
            if *stage == Stage::Start {
                live += 1;
                peak = peak.max(live);
            } else {
                live -= 1;
            }
        }
        assert!(
            peak <= jobs,
            "{peak} targets ran at once, the limit was {jobs}"
        );
    }
}

/// A stop that arrives before the run starts destroys nothing and names every target.
///
/// Given: a tree of files, and a stop already requested.
/// When: the run is started.
/// Then: no target is started, every file still holds its contents, and the report names each
/// file and the directory as intact.
///
/// Why it matters: the exit code is the only thing a script reads, and a stopped run that
/// reports success says the tree is gone while it still holds every byte.
#[test]
fn a_stop_before_the_run_starts_destroys_nothing_and_names_every_target() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let files = files_in(root.path(), 3, b"payload");
    let stop = AtomicBool::new(true);

    // When:
    let run = run_reacting(&plan_of(&[root.path()]), options(2, 1), &stop, |_| {});

    // Then:
    assert!(run.events.is_empty(), "{:?}", run.events);
    for path in files.iter().map(PathBuf::as_path).chain([root.path()]) {
        assert!(!damaged(&run, path), "{}", path.display());
    }
    for path in &files {
        assert_eq!(fs::read(path).unwrap(), b"payload");
    }
}

/// A stop in the middle of a run ends the files in progress and reports each one as it is.
///
/// Given: a tree of files, each 20 MiB of covers long and holding one known byte.
/// When: a stop is requested as the first file starts.
/// Then: every file survives, an intact one still holds every original byte, and a damaged one
/// no longer does.
///
/// Why it matters: an operator who stops a run decides what to do next from the report. A file
/// reported intact that the covers reached is plaintext they believe they still have, and a
/// stop that waits for a large file to finish can take minutes to arrive.
#[test]
fn a_stop_during_the_run_ends_the_files_in_progress() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let files = files_in(root.path(), 12, &vec![0xAB; COVER_CHUNK * 4]);
    let stop = AtomicBool::new(false);

    // When:
    let run = run_reacting(&plan_of(&[root.path()]), options(2, 4), &stop, |update| {
        if matches!(
            update,
            Update::Target {
                stage: Stage::Start,
                ..
            }
        ) {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    });

    // Then:
    for path in &files {
        let untouched = fs::read(path).unwrap().iter().all(|&byte| byte == 0xAB);
        assert_eq!(
            damaged(&run, path),
            !untouched,
            "{} is reported in the wrong state",
            path.display()
        );
    }
    assert!(!damaged(&run, root.path()));
}

/// Progress reaches the whole of the work when every target is destroyed.
///
/// Given: a tree with files of several sizes, an empty file, and a symlink.
/// When: the run finishes.
/// Then: progress never moves back, and the last update is the whole of the work.
///
/// Why it matters: a bar that ends short of the end on a clean run reads as a failure, and one
/// that moves back reads as a fault in the tool.
#[test]
fn progress_reaches_the_whole_of_the_work_on_a_clean_run() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("small.txt"), "payload").unwrap();
    fs::write(root.path().join("large.bin"), vec![1; COVER_CHUNK * 2]).unwrap();
    fs::write(root.path().join("empty.txt"), "").unwrap();
    symlink("small.txt", root.path().join("link")).unwrap();

    // When:
    let run = run(&[root.path()]);

    // Then:
    assert_eq!(run.report, Ok(()));
    assert!(run.progress.windows(2).all(|pair| pair[0].0 <= pair[1].0));
    let &(done, total) = run.progress.last().expect("the run reports progress");
    assert_eq!(done, total);
}

/// A run that could not destroy everything never reports the whole of the work.
///
/// Given: a file whose directory refuses the rename that ends the overwrite.
/// When: every cover has landed and the entry still cannot be removed.
/// Then: the last progress update stops short of the whole.
///
/// Why it matters: a full bar printed above a list of targets the run did not destroy reads as
/// a contradiction, and this is the case that reaches it, since every byte of the covers landed.
#[test]
fn progress_stops_short_when_a_target_is_not_destroyed() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("secret.txt");
    fs::write(&path, vec![0xAB; 4096]).unwrap();
    let Some(_sealed) = Sealed::new(root.path()) else {
        return;
    };

    // When:
    let run = run(&[&path]);

    // Then:
    assert!(run.report.is_err());
    let &(done, total) = run.progress.last().expect("the run reports progress");
    assert!(done < total, "{done} of {total}");
}

/// Progress moves inside a large file before the file is finished.
///
/// Given: one file several writes long.
/// When: it is destroyed.
/// Then: the first progress update is a part of the work, not none and not all of it.
///
/// Why it matters: a run over one large file would otherwise sit still for minutes, which reads
/// as a hang and invites the operator to stop exactly the work that must not be stopped.
#[test]
fn progress_moves_inside_a_large_file() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("large.bin");
    fs::write(&path, vec![1; COVER_CHUNK * 4]).unwrap();

    // When:
    let run = run(&[&path]);

    // Then:
    assert_eq!(run.report, Ok(()));
    let &(done, total) = run.progress.first().expect("the run reports progress");
    assert!(0 < done && done < total, "{done} of {total}");
}

/// A run whose caller fails stops taking new files.
///
/// Given: a tree of files and one worker.
/// When: the caller panics as it records the first start.
/// Then: every file after the first is still on the disk.
///
/// Why it matters: the caller holds the only record of the run, so a worker that goes on after
/// it is gone destroys the rest of the tree with nothing left to report what it destroyed.
#[test]
fn a_run_whose_caller_fails_stops_taking_new_files() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let files = files_in(root.path(), 10, b"payload");
    let plan = plan_of(&[root.path()]);

    // When:
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_reacting(&plan, options(1, 1), &AtomicBool::new(false), |update| {
            if matches!(update, Update::Target { .. }) {
                panic!("the caller fails");
            }
        })
    }));

    // Then:
    assert!(failed.is_err());
    for path in &files[1..] {
        assert_eq!(fs::read(path).unwrap(), b"payload", "{}", path.display());
    }
}
