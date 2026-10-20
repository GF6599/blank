mod common;

use std::fs::{self, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

use common::{run, text};

/// The manifest names every target with the state the run left it in.
///
/// Given: a tree holding a file the run can destroy and a directory nobody can list, with a
/// file inside it.
/// When: the tree is destroyed with --log.
/// Then: the manifest is at the path given, names the file as destroyed, and names the
/// directory as intact with the reason.
///
/// Why it matters: once the files are gone, the manifest is the only record of what the run
/// reached and what it left behind.
#[test]
fn the_manifest_names_every_target_and_its_state() {
    if nix::unistd::geteuid().is_root() {
        return;
    }
    // Given:
    let root = tempfile::tempdir().unwrap();
    let secret = root.path().join("secret.txt");
    fs::write(&secret, "payload").unwrap();
    let locked = root.path().join("locked");
    fs::create_dir(&locked).unwrap();
    fs::write(locked.join("hidden.txt"), "payload").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let manifest = elsewhere.path().join("manifest.log");

    // When:
    let outcome = run(&["--yes", "--log", text(&manifest), text(root.path())]);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();

    // Then:
    assert_ne!(outcome.code, 0);
    let lines = fs::read_to_string(&manifest).unwrap();
    let lines: Vec<&str> = lines.lines().collect();
    assert!(
        lines.contains(&format!("destroyed\t{}", text(&secret)).as_str()),
        "{lines:?}"
    );
    let prefix = format!("intact\t{}\tread directory: ", text(&locked));
    assert!(
        lines.iter().any(|line| line.starts_with(&prefix)),
        "{lines:?}"
    );
}

/// A manifest cannot be placed where the same run will destroy it.
///
/// Given: a log path inside a selected directory, and another that is a hard link to a selected
/// file.
/// When: a run is asked to start with each.
/// Then: it refuses before the question, says why, creates no manifest, and the target keeps its
/// contents.
///
/// Why it matters: the manifest is the only record of what was destroyed, so a run that writes
/// it into its own targets destroys the record along with the tree.
#[test]
fn a_manifest_inside_a_selected_target_is_refused() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let victim = root.path().join("secret.txt");
    fs::write(&victim, "payload").unwrap();
    let inside = root.path().join("manifest.log");
    let elsewhere = tempfile::tempdir().unwrap();
    let linked = elsewhere.path().join("linked.log");
    fs::hard_link(&victim, &linked).unwrap();

    for (log, selected) in [(&inside, root.path()), (&linked, victim.as_path())] {
        // When:
        let outcome = run(&["--log", text(log), text(selected)]);

        // Then:
        assert_ne!(outcome.code, 0, "{}", text(log));
        assert!(
            outcome.stderr.contains("inside the selected targets"),
            "{}",
            outcome.stderr
        );
        assert_eq!(fs::read_to_string(&victim).unwrap(), "payload");
    }
    assert!(inside.symlink_metadata().is_err());
}

/// A manifest reached through a symlink and `..` is checked where the open lands.
///
/// Given: a selected tree holding a manifest and a directory, and a link elsewhere to that
/// directory.
/// When: a run is asked to log to the link followed by `..` and the manifest name.
/// Then: it refuses, and the manifest keeps what it held.
///
/// Why it matters: the system resolves the link before `..`, so the open reaches the manifest
/// inside the tree, while a check that cleans `..` by text looks beside the link and lets the
/// run overwrite its own record.
#[test]
fn a_manifest_reached_through_a_link_and_dot_dot_is_checked_where_it_lands() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let inside = root.path().join("manifest.log");
    fs::write(&inside, "from an earlier run\n").unwrap();
    fs::create_dir(root.path().join("sub")).unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    symlink(root.path().join("sub"), elsewhere.path().join("l")).unwrap();
    let log = elsewhere.path().join("l").join("..").join("manifest.log");

    // When:
    let outcome = run(&["--yes", "--log", text(&log), text(root.path())]);

    // Then:
    assert_ne!(outcome.code, 0, "{}", outcome.stdout);
    assert!(
        outcome.stderr.contains("inside the selected targets"),
        "{}",
        outcome.stderr
    );
    assert_eq!(
        fs::read_to_string(&inside).unwrap(),
        "from an earlier run\n"
    );
}

/// The manifest is readable only by its owner, even when it was already there.
///
/// Given: one log path that does not exist yet, and one an earlier run left readable by anyone.
/// When: a run writes to each.
/// Then: neither carries a group or other permission afterwards.
///
/// Why it matters: the manifest names every path a run destroyed, and the mode an open applies
/// belongs only to a file it creates.
#[test]
fn the_manifest_is_readable_only_by_its_owner() {
    // Given:
    let elsewhere = tempfile::tempdir().unwrap();
    let fresh = elsewhere.path().join("fresh.log");
    let existing = elsewhere.path().join("existing.log");
    fs::write(&existing, "from an earlier run\n").unwrap();
    fs::set_permissions(&existing, fs::Permissions::from_mode(0o666)).unwrap();

    for log in [&fresh, &existing] {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("secret.txt"), "payload").unwrap();

        // When:
        let outcome = run(&["--yes", "--log", text(log), text(root.path())]);

        // Then:
        assert_eq!(outcome.code, 0, "{}", outcome.stderr);
        let mode = fs::metadata(log).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "{} is mode {mode:o}", text(log));
    }
    assert!(
        fs::read_to_string(&existing)
            .unwrap()
            .starts_with("from an earlier run\n")
    );
}

/// A manifest that cannot be opened safely stops the run before anything is destroyed.
///
/// Given: a log path that is a symlink to another file, one in a directory that does not exist,
/// a device, and a FIFO that another process reads.
/// When: a run is asked to start with each.
/// Then: it exits non-zero, the target keeps its contents, and the file behind the link is
/// unchanged.
///
/// Why it matters: a link at a predictable path would append the manifest to a file the operator
/// never named, a run without its manifest destroys the tree and the record together, the mode
/// change that narrows a manifest would narrow a device that everyone needs, and a FIFO streams
/// the manifest to whoever holds its other end.
#[test]
fn a_manifest_that_cannot_be_opened_safely_stops_the_run() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let victim = root.path().join("secret.txt");
    fs::write(&victim, "payload").unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let unrelated = elsewhere.path().join("unrelated.txt");
    fs::write(&unrelated, "never named").unwrap();
    let link = elsewhere.path().join("manifest.log");
    symlink(&unrelated, &link).unwrap();
    let missing = elsewhere
        .path()
        .join("no-such-directory")
        .join("manifest.log");
    let fifo = elsewhere.path().join("fifo.log");
    nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRWXU).unwrap();
    let _reader = OpenOptions::new()
        .read(true)
        // Without it the open waits for a writer, and the writer is the run under test.
        .custom_flags(nix::fcntl::OFlag::O_NONBLOCK.bits())
        .open(&fifo)
        .unwrap();

    for log in [&link, &missing, Path::new("/dev/null"), &fifo] {
        // When:
        let outcome = run(&["--yes", "--log", text(log), text(root.path())]);

        // Then:
        assert_ne!(outcome.code, 0, "{}", text(log));
        assert_eq!(fs::read_to_string(&victim).unwrap(), "payload");
    }
    assert_eq!(fs::read_to_string(&unrelated).unwrap(), "never named");
}

/// A dry run never creates the manifest.
///
/// Given: a tree and a log path that does not exist.
/// When: a dry run is made with --log.
/// Then: no file is at the log path.
///
/// Why it matters: a dry run promises to change nothing, and a manifest of a run that never
/// happened reads as a record of files that were destroyed.
#[test]
fn a_dry_run_never_creates_the_manifest() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("secret.txt"), "payload").unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let manifest = elsewhere.path().join("manifest.log");

    // When:
    let outcome = run(&["--dry-run", "--log", text(&manifest), text(root.path())]);

    // Then:
    assert_eq!(outcome.code, 0, "{}", outcome.stderr);
    assert!(manifest.symlink_metadata().is_err());
}
