mod common;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::process::Command;

use common::terminal::{Respond, drive};
use nix::errno::Errno;
use nix::sys::signal::kill;
use nix::unistd::Pid;

fn blank_in(dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_blank"));
    command.current_dir(dir);
    command
}

/// Marked targets go to the same review and the same run as a command line.
///
/// Given: a directory holding two files, and blank started there with no arguments.
/// When: the operator marks the first file, chooses done, answers yes on the review, and closes
/// the run.
/// Then: the process exits zero, the marked file is gone, and the other is still there.
///
/// Why it matters: the selector is a way to write a command line, so what it hands over must
/// pass the same review and reach the same run, and nothing it did not mark may be touched.
#[test]
fn the_marked_set_goes_through_the_review_to_the_run() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let marked = root.path().join("a.txt");
    let kept = root.path().join("b.txt");
    fs::write(&marked, "payload").unwrap();
    fs::write(&kept, "payload").unwrap();

    // When:
    let shown = drive(
        &mut blank_in(root.path()),
        [
            ("marked", Respond::Keys(" ")),
            ("*", Respond::Keys("\r")),
            ("press enter to destroy", Respond::Keys("yes\r")),
            ("complete", Respond::Keys("q")),
        ],
    );

    // Then:
    assert_eq!(shown.code, 0, "{}", shown.after);
    assert!(!marked.exists());
    assert_eq!(fs::read_to_string(&kept).unwrap(), "payload");
}

/// Quitting the selector destroys nothing.
///
/// Given: a directory holding a file, and blank started there with no arguments.
/// When: the operator marks the file and quits.
/// Then: the process exits non-zero saying nothing was destroyed, and the file is still there.
///
/// Why it matters: a quit is how an operator leaves a screen opened by mistake, and the exit
/// code is how a script learns that nothing happened.
#[test]
fn quitting_the_selector_destroys_nothing() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("a.txt");
    fs::write(&file, "payload").unwrap();

    // When:
    let shown = drive(
        &mut blank_in(root.path()),
        [("marked", Respond::Keys(" ")), ("*", Respond::Keys("q"))],
    );

    // Then:
    assert_eq!(shown.code, 1, "{}", shown.after);
    assert!(
        shown.after.contains("cancelled, nothing was destroyed"),
        "{:?}",
        shown.after
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "payload");
}

/// The selector refuses to open in a protected directory, under its own name or another.
///
/// Given: a home directory, and a symlink elsewhere that points at it.
/// When: blank starts with no arguments in each.
/// Then: it exits non-zero, says why, and tells the operator to name a path on the command line.
///
/// Why it matters: marking costs one keystroke, so a selector opened on the home directory puts
/// all of it one key and one answer from destruction.
#[test]
fn the_selector_refuses_to_open_in_a_protected_directory() {
    // Given:
    let home = tempfile::tempdir().unwrap();
    let home = home.path().canonicalize().unwrap();
    fs::write(home.join("a.txt"), "payload").unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let link = elsewhere.path().join("shortcut");
    symlink(&home, &link).unwrap();

    for start in [&home, &link] {
        // When:
        let mut command = blank_in(start);
        command.env("HOME", &home);
        let shown = drive(&mut command, []);

        // Then:
        assert_eq!(shown.code, 1, "{}", shown.after);
        assert!(shown.after.contains("protected"), "{:?}", shown.after);
        assert!(shown.after.contains("command line"), "{:?}", shown.after);
    }
}

/// Without a full screen, no arguments print the usage.
///
/// Given: blank on a terminal with --no-ui.
/// When: it starts with no arguments.
/// Then: it prints the usage and exits zero, and never opens a full screen.
///
/// Why it matters: a script that runs blank must never wait on a screen nobody can answer, and
/// --no-ui is how an operator asks for no screen at all.
#[test]
fn no_arguments_with_no_ui_prints_the_usage() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let mut command = blank_in(root.path());
    command.arg("--no-ui");

    // When:
    let shown = drive(&mut command, []);

    // Then:
    assert_eq!(shown.code, 0, "{}", shown.after);
    assert!(shown.after.contains("Usage:"), "{:?}", shown.after);
    assert!(!shown.after.contains("\x1b[?1049h"));
}

/// Closing the selector kills a tool still drawing a preview, and removes the directory it
/// wrote in.
///
/// Given: a directory holding a PDF, a pdftoppm that never finishes on the search path, and a
/// temporary directory of the test's own.
/// When: blank opens there with the cursor on the PDF, and the operator quits once the tool has
/// started.
/// Then: the tool process is gone when blank has exited, and nothing is left in the temporary
/// directory.
///
/// Why it matters: a tool left running outlives blank on an untrusted file, and a render left on
/// the disk keeps a picture of a file the operator may go on to destroy.
#[test]
fn closing_the_selector_kills_its_tool_and_leaves_nothing_behind() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("report.pdf"), "%PDF-1.4\n%%EOF\n").unwrap();
    let tools = tempfile::tempdir().unwrap();
    let started = tools.path().join("started");
    let script = format!(
        "#!/bin/sh\necho $$ > '{started}.part'\n/bin/mv '{started}.part' '{started}'\nexec /bin/sleep 60\n",
        started = started.display()
    );
    let tool = tools.path().join("pdftoppm");
    fs::write(&tool, script).unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let mut command = blank_in(root.path());
    let search = format!("{}:/usr/bin:/bin", tools.path().display());
    command.env("PATH", search).env("TMPDIR", scratch.path());

    // When:
    let shown = drive(
        &mut command,
        [("report.pdf", Respond::KeysOnceExists(started.clone(), "q"))],
    );

    // Then:
    assert_eq!(shown.code, 1, "{}", shown.after);
    let pid: i32 = fs::read_to_string(&started)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(
        kill(Pid::from_raw(pid), None),
        Err(Errno::ESRCH),
        "the tool still runs"
    );
    let left: Vec<_> = fs::read_dir(scratch.path()).unwrap().collect();
    assert!(left.is_empty(), "{left:?}");
}
