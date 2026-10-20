mod common;

use std::fs;
use std::path::Path;

use common::terminal::{Respond, show};
use common::text;

/// A run on a terminal is drawn full screen, and its verdict is printed once the screen closes.
///
/// Given: a tree with one file.
/// When: it is destroyed on a terminal, and the operator presses q once the run is complete.
/// Then: the process exits zero, the tree is gone, and the text left after the full screen
/// closes says blanking complete.
///
/// Why it matters: the alternate screen is cleared on exit, so the printed verdict is the only
/// record of the run left in the scrollback, and it must say what the exit code says.
#[test]
fn a_complete_run_prints_its_verdict_after_the_screen_closes() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("secret.txt"), "payload").unwrap();

    // When:
    let shown = show(
        &["--yes", text(root.path())],
        "complete",
        Respond::Keys("q"),
    );

    // Then:
    assert_eq!(shown.code, 0, "{}", shown.after);
    assert!(!root.path().exists());
    assert!(
        shown.after.contains("blanking complete"),
        "{:?}",
        shown.after
    );
}

/// A run that leaves a survivor prints a failed verdict, the survivor, and exits non-zero.
///
/// Given: a tree with one file, and a second argument that does not exist.
/// When: it is destroyed on a terminal, and the operator presses q once the run has failed.
/// Then: the process exits 1, and the text left after the full screen closes says blanking
/// failed with one survivor and names it.
///
/// Why it matters: the operator decides what to do next from the scrollback, so the verdict
/// and the survivor must both be there once the full screen is gone.
#[test]
fn a_failed_run_prints_its_verdict_and_its_survivor_after_the_screen_closes() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("secret.txt"), "payload").unwrap();
    let missing = root.path().with_extension("missing");

    // When:
    let shown = show(
        &["--yes", text(root.path()), text(Path::new(&missing))],
        "failed",
        Respond::Keys("q"),
    );

    // Then:
    assert_eq!(shown.code, 1, "{}", shown.after);
    assert!(
        shown.after.contains("blanking failed, 1 target survived"),
        "{:?}",
        shown.after
    );
    assert!(shown.after.contains(text(&missing)), "{:?}", shown.after);
}

/// q during a run stops it, and the verdict says the run was cancelled.
///
/// Given: a tree of files that takes many covers to destroy, on two workers.
/// When: the operator presses q as soon as the first file is drawn.
/// Then: the process exits 1, files survive, and the text left after the full screen closes
/// says blanking cancelled.
///
/// Why it matters: q is how an operator stops a run aimed at the wrong tree, and a screen that
/// only closes while the run goes on behind it destroys the tree it was asked to spare.
#[test]
fn q_during_a_run_stops_it_and_reports_it_cancelled() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let files: Vec<_> = (0..40)
        .map(|index| {
            let path = root.path().join(format!("file-{index:02}.bin"));
            fs::write(&path, vec![0xAB; 2 << 20]).unwrap();
            path
        })
        .collect();

    // When:
    let shown = show(
        &["--yes", "--jobs", "2", "--passes", "50", text(root.path())],
        "file-",
        Respond::Keys("q"),
    );

    // Then:
    assert_eq!(shown.code, 1, "{}", shown.after);
    let survivors = files.iter().filter(|path| path.exists()).count();
    assert!(survivors >= files.len() / 2, "{survivors} survived");
    assert!(
        shown.after.contains("blanking cancelled"),
        "{:?}",
        shown.after
    );
}

/// An interrupt from outside the terminal stops a run on the full screen and closes it.
///
/// Given: a tree of files that takes many covers to destroy, on two workers.
/// When: the process receives SIGINT as soon as the first file is drawn, and no key is pressed.
/// Then: the process exits 1 by itself, files survive, and the verdict says cancelled.
///
/// Why it matters: raw mode turns ctrl+c into a key, so a SIGINT can only come from outside,
/// such as a supervisor stopping the job, and a screen that waits for a key then never ends.
#[test]
fn an_interrupt_stops_a_run_on_the_full_screen_and_closes_it() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let files: Vec<_> = (0..40)
        .map(|index| {
            let path = root.path().join(format!("file-{index:02}.bin"));
            fs::write(&path, vec![0xAB; 2 << 20]).unwrap();
            path
        })
        .collect();

    // When:
    let shown = show(
        &["--yes", "--jobs", "2", "--passes", "50", text(root.path())],
        "file-",
        Respond::Interrupt,
    );

    // Then:
    assert_eq!(shown.code, 1, "{}", shown.after);
    let survivors = files.iter().filter(|path| path.exists()).count();
    assert!(survivors >= files.len() / 2, "{survivors} survived");
    assert!(
        shown.after.contains("blanking cancelled"),
        "{:?}",
        shown.after
    );
}
