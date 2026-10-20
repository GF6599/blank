mod common;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};

use common::{run, run_with, text};

/// A confirmed run destroys the tree and prints one line per target as it reaches it.
///
/// Given: a tree holding one file.
/// When: the run is confirmed with --yes.
/// Then: it prints an activity line for the file and then for the directory, exits zero, and
/// the tree is gone.
///
/// Why it matters: the activity lines are what a script reads to learn what was destroyed, and
/// exit zero is the promise that everything was.
#[test]
fn a_confirmed_run_destroys_the_tree_and_names_each_target() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let secret = root.path().join("secret.txt");
    fs::write(&secret, "classified").unwrap();

    // When:
    let outcome = run(&["--yes", text(root.path())]);

    // Then:
    assert_eq!(outcome.code, 0, "{}", outcome.stderr);
    let activity: Vec<&str> = outcome
        .stdout
        .lines()
        .filter(|line| line.starts_with("Activity:"))
        .collect();
    assert_eq!(
        activity,
        [
            format!("Activity: Overwriting {}", text(&secret)),
            format!("Activity: Deleting directory {}", text(root.path())),
        ]
    );
    assert!(root.path().symlink_metadata().is_err());
}

/// A bad pass count stops the run before anything is destroyed.
///
/// Given: a tree with a file in it.
/// When: the run is given a negative pass count.
/// Then: it exits non-zero and the file still holds its contents.
///
/// Why it matters: the count arrives from a flag, so it is refused once at the entry, before
/// the first target and not halfway through the tree.
#[test]
fn a_negative_pass_count_is_refused_before_anything_is_destroyed() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let victim = root.path().join("victim.txt");
    fs::write(&victim, "payload").unwrap();

    // When:
    let outcome = run(&["--yes", "--passes", "-1", text(root.path())]);

    // Then:
    assert_ne!(outcome.code, 0);
    assert_eq!(fs::read_to_string(&victim).unwrap(), "payload");
}

/// A run over a mixed tree destroys what it can reach and names what it cannot.
///
/// Given: a tree holding a nested file, a file nobody can read, a FIFO, a broken symlink, and a
/// directory nobody can list.
/// When: the run is confirmed.
/// Then: it exits non-zero naming the unlisted directory, that directory is still there, and
/// every other entry is gone.
///
/// Why it matters: this is the whole engine against the shapes a real tree holds, and the exit
/// code and the report are what tell the caller which part of the promise was kept.
#[test]
fn a_mixed_tree_is_destroyed_except_what_cannot_be_reached() {
    if nix::unistd::geteuid().is_root() {
        return;
    }
    // Given:
    let root = tempfile::tempdir().unwrap();
    let nested = root.path().join("a").join("b");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("deep.txt"), "payload").unwrap();
    let unreadable = root.path().join("no_read.txt");
    fs::write(&unreadable, "payload").unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
    nix::unistd::mkfifo(&root.path().join("pipe"), nix::sys::stat::Mode::S_IRWXU).unwrap();
    symlink(root.path().join("nowhere"), root.path().join("broken")).unwrap();
    let locked = root.path().join("no_list_dir");
    fs::create_dir(&locked).unwrap();
    fs::write(locked.join("hidden.txt"), "payload").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    // When:
    let outcome = run_with(&["--yes", text(root.path())], "", |_| {});
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();

    // Then:
    assert_ne!(outcome.code, 0, "{}", outcome.stdout);
    assert!(
        outcome
            .stderr
            .contains(&format!("{}: read directory", text(&locked))),
        "{}",
        outcome.stderr
    );
    let mut left: Vec<String> = fs::read_dir(root.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(left, ["no_list_dir"]);
    assert!(locked.join("hidden.txt").exists());
}

/// Headless progress prints a line only when the whole percent moves.
///
/// Given: a tree of 300 files, so a line per target would be three per percent.
/// When: the tree is destroyed.
/// Then: there is at most one progress line per whole percent, and the last says 100%.
///
/// Why it matters: one line per target buries the activity lines a caller reads the run from,
/// and a tree of a hundred thousand files would print a hundred thousand of them.
#[test]
fn progress_prints_a_line_only_when_the_percent_moves() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    for index in 0..300 {
        fs::write(root.path().join(format!("file-{index:03}.txt")), "payload").unwrap();
    }

    // When:
    let outcome = run(&["--yes", "--passes", "1", text(root.path())]);

    // Then:
    assert_eq!(outcome.code, 0, "{}", outcome.stderr);
    let progress: Vec<&str> = outcome
        .stdout
        .lines()
        .filter(|line| line.starts_with("Progress:"))
        .collect();
    assert!(progress.len() <= 101, "{} progress lines", progress.len());
    assert_eq!(progress.last(), Some(&"Progress: 100%"));
}

/// A pool of no workers is refused before anything is destroyed.
///
/// Given: a tree with a file in it.
/// When: the run is given --jobs 0.
/// Then: it exits non-zero, names the flag, and the file still holds its contents.
///
/// Why it matters: a pool of no workers would leave every file queued against nothing, so the
/// run would hang with the tree half planned and nothing reported.
#[test]
fn a_pool_of_no_workers_is_refused_before_anything_is_destroyed() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let victim = root.path().join("victim.txt");
    fs::write(&victim, "payload").unwrap();

    // When:
    let outcome = run(&["--yes", "--jobs", "0", text(root.path())]);

    // Then:
    assert_ne!(outcome.code, 0);
    assert!(
        outcome.stderr.contains("invalid value '0' for '--jobs"),
        "{}",
        outcome.stderr
    );
    assert_eq!(fs::read_to_string(&victim).unwrap(), "payload");
}
