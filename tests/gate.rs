mod common;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use common::terminal::{Respond, drive};
use common::text;

fn tree() -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("notes.txt");
    fs::write(&file, "payload").unwrap();
    (root, file)
}

fn blank(args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_blank"));
    command.args(args);
    command
}

/// On a terminal, nothing is destroyed until the review screen is answered yes.
///
/// Given: a tree named on the command line of blank on a terminal, without --yes.
/// When: the operator presses enter with no answer, and then quits.
/// Then: the process exits non-zero saying nothing was destroyed, and the file holds its bytes.
///
/// Why it matters: the review is the gate before a run that cannot be undone, and enter is the
/// key most often pressed by reflex.
#[test]
fn nothing_is_destroyed_until_the_review_is_answered_yes() {
    // Given:
    let (root, file) = tree();

    // When:
    let shown = drive(
        &mut blank(&[text(root.path())]),
        [
            ("press enter to destroy", Respond::Keys("\r")),
            ("notes.txt", Respond::Keys("q")),
        ],
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

/// A yes on the review screen starts the run.
///
/// Given: a tree named on the command line of blank on a terminal, without --yes.
/// When: the operator types yes, presses enter, and closes the run once it completes.
/// Then: the process exits zero and the tree is gone.
///
/// Why it matters: a gate that never opens is a tool that does not work, and it would pass the
/// test above.
#[test]
fn a_yes_on_the_review_starts_the_run() {
    // Given:
    let (root, _file) = tree();

    // When:
    let shown = drive(
        &mut blank(&[text(root.path())]),
        [
            ("press enter to destroy", Respond::Keys("yes\r")),
            ("complete", Respond::Keys("q")),
        ],
    );

    // Then:
    assert_eq!(shown.code, 0, "{}", shown.after);
    assert!(!root.path().exists());
}

/// --yes skips the review on a terminal too.
///
/// Given: a tree named on the command line of blank on a terminal, with --yes.
/// When: the run completes and the operator closes it.
/// Then: the process exits zero and the tree is gone, so the review never waited for an answer.
///
/// Why it matters: --yes is how a caller that already decided says so, and a review that asks
/// again stalls every such caller on a terminal.
#[test]
fn yes_skips_the_review() {
    // Given:
    let (root, _file) = tree();

    // When:
    let shown = drive(
        &mut blank(&["--yes", text(root.path())]),
        [("complete", Respond::Keys("q"))],
    );

    // Then:
    assert_eq!(shown.code, 0, "{}", shown.after);
    assert!(!root.path().exists());
}
