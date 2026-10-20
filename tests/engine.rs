use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use blank::engine::{COVER_CHUNK, Stage, destroy};
use blank::guard::Guard;
use blank::plan::Plan;
use tempfile::TempDir;

struct Run {
    report: Result<(), String>,
    events: Vec<(&'static str, PathBuf, Stage)>,
}

fn plan_of(args: &[&Path]) -> Plan {
    let args: Vec<PathBuf> = args.iter().map(|arg| arg.to_path_buf()).collect();
    Plan::enumerate(&args, &Guard::for_host()).expect("no case here names a protected path")
}

fn run_plan(plan: &Plan, passes: u32) -> Run {
    let mut events = Vec::new();
    let report = destroy(plan, passes, &mut |event| {
        events.push((event.verb, event.path.to_path_buf(), event.stage));
    })
    .map_err(|incomplete| incomplete.to_string());
    Run { report, events }
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
