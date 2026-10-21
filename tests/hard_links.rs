mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use blank::engine::{DEFAULT_JOBS, Options, destroy};
use blank::guard::Guard;
use blank::plan::Plan;
use common::{run, text};

fn plan_of(paths: &[&Path]) -> Plan {
    let paths: Vec<PathBuf> = paths.iter().map(|path| path.to_path_buf()).collect();
    Plan::enumerate(&paths, &Guard::for_host()).unwrap()
}

fn options(allow_hard_links: bool) -> Options {
    Options {
        jobs: DEFAULT_JOBS,
        passes: 1,
        allow_hard_links,
    }
}

fn destroyed(plan: &Plan, allow_hard_links: bool) -> Result<(), String> {
    destroy(
        plan,
        options(allow_hard_links),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .map_err(|incomplete| incomplete.to_string())
}

/// A tree with a file that has a second name outside it.
fn linked_from_outside() -> (tempfile::TempDir, tempfile::TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let inside = root.path().join("shared.txt");
    fs::write(&inside, "contents under both names").unwrap();
    let elsewhere = outside.path().join("kept.txt");
    fs::hard_link(&inside, &elsewhere).unwrap();
    (root, outside, inside, elsewhere)
}

/// A file with a name outside the selection is left intact and named.
///
/// Given: a tree holding a file that has a second hard link outside the tree.
/// When: the tree is destroyed without permission for hard links.
/// Then: the run reports the file as intact, and the outside name still reads the contents.
///
/// Why it matters: the covers write to the file and not to a name, so a run that covers it
/// destroys the contents under the outside name, which nobody selected. Anyone who can write in
/// a selected tree can add such a link to a file the operator owns.
#[test]
fn a_file_with_a_name_outside_the_selection_is_left_intact() {
    // Given:
    let (root, _outside, inside, elsewhere) = linked_from_outside();

    // When:
    let report = destroyed(&plan_of(&[root.path()]), false);

    // Then:
    let report = report.expect_err("the run reports the file it left");
    let line = report
        .lines()
        .find(|line| line.contains(text(&inside)))
        .unwrap_or_else(|| panic!("{report}"));
    assert!(
        line.contains("hard link") && line.ends_with("(intact)"),
        "{line}"
    );
    assert_eq!(
        fs::read_to_string(&elsewhere).unwrap(),
        "contents under both names"
    );
}

/// A link added after the plan is caught when the run reaches the file.
///
/// Given: a planned tree whose file gains a hard link outside the tree after the plan.
/// When: the tree is destroyed without permission for hard links.
/// Then: the outside name still reads the contents.
///
/// Why it matters: the question and the review take as long as the operator takes to answer,
/// and a link made in that time would otherwise pass a check made only at plan time.
#[test]
fn a_link_added_after_the_plan_is_caught_at_the_run() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let inside = root.path().join("shared.txt");
    fs::write(&inside, "contents").unwrap();
    let plan = plan_of(&[root.path()]);
    let outside = tempfile::tempdir().unwrap();
    let elsewhere = outside.path().join("kept.txt");
    fs::hard_link(&inside, &elsewhere).unwrap();

    // When:
    let report = destroyed(&plan, false);

    // Then:
    assert!(report.is_err());
    assert_eq!(fs::read_to_string(&elsewhere).unwrap(), "contents");
}

/// With permission, a file with a name outside the selection is destroyed under every name.
///
/// Given: a tree holding a file that has a second hard link outside the tree.
/// When: the tree is destroyed with permission for hard links.
/// Then: the run reports success, and the outside name reads zeros.
///
/// Why it matters: an operator who knows the other names are theirs needs a way to destroy the
/// contents, and the permission must reach every name, or the flag promises what it does not do.
#[test]
fn with_permission_the_contents_are_destroyed_under_every_name() {
    // Given:
    let (root, _outside, _inside, elsewhere) = linked_from_outside();

    // When:
    let report = destroyed(&plan_of(&[root.path()]), true);

    // Then:
    assert_eq!(report, Ok(()));
    assert!(fs::read(&elsewhere).unwrap().iter().all(|&byte| byte == 0));
}

/// A file whose every name is selected is destroyed without permission.
///
/// Given: a tree holding a file under two names, both inside the tree.
/// When: the tree is destroyed without permission for hard links.
/// Then: the run reports success and both names are gone.
///
/// Why it matters: the rule protects names outside the selection, and refusing a file whose
/// names were all chosen leaves contents the operator asked to destroy.
#[test]
fn a_file_whose_every_name_is_selected_is_destroyed() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first.txt");
    fs::write(&first, "contents").unwrap();
    let second = root.path().join("second.txt");
    fs::hard_link(&first, &second).unwrap();

    // When:
    let report = destroyed(&plan_of(&[root.path()]), false);

    // Then:
    assert_eq!(report, Ok(()));
    assert!(!root.path().exists());
}

/// The dry run names the files with links outside the selection, and the flag destroys them.
///
/// Given: a tree holding a file with a hard link outside it.
/// When: it is dry run, then destroyed with --yes, then destroyed again with
/// --allow-hard-links.
/// Then: the dry run says one file has hard links outside the selection, the first run exits
/// non-zero with the outside contents intact, and the second exits zero with them gone.
///
/// Why it matters: the operator cannot see the other names of a file, so the dry run is where
/// they learn of them, and the flag is the only way through.
#[test]
fn the_dry_run_names_linked_files_and_the_flag_destroys_them() {
    // Given:
    let (root, _outside, _inside, elsewhere) = linked_from_outside();

    // When:
    let dry = run(&["--dry-run", text(root.path())]);
    let refused = run(&["--yes", text(root.path())]);
    let kept = fs::read_to_string(&elsewhere).unwrap();
    let allowed = run(&["--yes", "--allow-hard-links", text(root.path())]);

    // Then:
    assert!(
        dry.stdout
            .contains("1 file has hard links outside the selection"),
        "{}",
        dry.stdout
    );
    assert_ne!(refused.code, 0);
    assert_eq!(kept, "contents under both names");
    assert_eq!(allowed.code, 0, "{}", allowed.stderr);
    assert!(fs::read(&elsewhere).unwrap().iter().all(|&byte| byte == 0));
}
