use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use blank::guard::Guard;
use blank::plan::{Plan, VERB_DIRECTORY, VERB_FILE, VERB_REMOVAL};

fn enumerate(args: &[&Path]) -> Plan {
    let args: Vec<PathBuf> = args.iter().map(|arg| arg.to_path_buf()).collect();
    Plan::enumerate(&args, &Guard::for_host()).expect("no case here names a protected path")
}

fn write(path: &Path) {
    fs::write(path, "payload").expect("the test file is written");
}

/// Enumeration names every entry in the bucket that decides its fate, and destroys nothing.
///
/// Given: a tree holding a file, a symlink to it, and the directory itself.
/// When: the tree is enumerated.
/// Then: the file, the symlink and the directory each land in their own bucket, and the file
/// still holds its contents.
///
/// Why it matters: the dry run and the question are both built from this list, so an
/// enumeration with a side effect would destroy the tree while the operator is still deciding.
#[test]
fn enumeration_sorts_each_entry_into_its_bucket_and_touches_nothing() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let kept = root.path().join("kept.txt");
    write(&kept);
    let link = root.path().join("link");
    symlink(&kept, &link).unwrap();

    // When:
    let plan = enumerate(&[root.path()]);

    // Then:
    assert_eq!(plan.files, std::slice::from_ref(&kept));
    assert_eq!(plan.removals, std::slice::from_ref(&link));
    assert_eq!(plan.directories, [root.path().to_path_buf()]);
    // And:
    assert_eq!(fs::read_to_string(&kept).unwrap(), "payload");
    assert!(link.symlink_metadata().is_ok());
}

/// The plan lists directories in the order a run removes them, the deepest first.
///
/// Given: a tree two directories deep.
/// When: the plan is read back as targets.
/// Then: each directory comes before the one holding it.
///
/// Why it matters: the plan is what the operator is shown, so a top-down list would describe an
/// order the run does not use.
#[test]
fn targets_list_the_deepest_directory_first() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let middle = root.path().join("middle");
    let deep = middle.join("deep");
    fs::create_dir_all(&deep).unwrap();

    // When:
    let listed: Vec<PathBuf> = enumerate(&[root.path()])
        .targets()
        .into_iter()
        .map(|target| target.path)
        .collect();

    // Then:
    assert_eq!(listed, [deep, middle, root.path().to_path_buf()]);
}

/// The walk reads each directory in name order.
///
/// Given: three files created in reverse name order.
/// When: the directory is enumerated.
/// Then: the files are planned in name order.
///
/// Why it matters: the operating system returns entries in no fixed order, so without sorting
/// two dry runs over one tree can list it differently, and the operator cannot compare them.
#[test]
fn the_walk_reads_each_directory_in_name_order() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    for name in ["c.txt", "b.txt", "a.txt"] {
        write(&root.path().join(name));
    }

    // When:
    let plan = enumerate(&[root.path()]);

    // Then:
    let names: Vec<_> = plan
        .files
        .iter()
        .map(|file| file.file_name().unwrap())
        .collect();
    assert_eq!(names, ["a.txt", "b.txt", "c.txt"]);
}

/// A dry run and a real run describe a target with the same words.
///
/// Given: a tree holding a file, a symlink, and the directory itself.
/// When: the plan is read as targets.
/// Then: each target carries the verb the activity stream uses for it.
///
/// Why it matters: two sets of wording drift apart, and the operator would approve a plan
/// phrased one way and then watch a run phrased another.
#[test]
fn targets_carry_the_activity_verbs() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file.txt");
    write(&file);
    let link = root.path().join("link");
    symlink(&file, &link).unwrap();

    // When:
    let verbs: Vec<(&str, PathBuf)> = enumerate(&[root.path()])
        .targets()
        .into_iter()
        .map(|target| (target.verb, target.path))
        .collect();

    // Then:
    assert_eq!(
        verbs,
        [
            (VERB_FILE, file),
            (VERB_REMOVAL, link),
            (VERB_DIRECTORY, root.path().to_path_buf()),
        ]
    );
}

/// A target the enumeration cannot read travels with the plan.
///
/// Given: an argument that does not exist.
/// When: it is enumerated.
/// Then: the plan holds no targets and reports the argument by name.
///
/// Why it matters: a mistyped argument that reaches no list exits zero having destroyed
/// nothing, which reads exactly like a successful run.
#[test]
fn the_plan_carries_what_it_could_not_read() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("does-not-exist");

    // When:
    let plan = enumerate(&[&missing]);

    // Then:
    assert_eq!(plan.total(), 0);
    let report = plan
        .incomplete()
        .expect("the missing argument is reported")
        .to_string();
    assert!(report.contains(missing.to_str().unwrap()), "{report}");
}

/// An argument named twice, or named inside another argument, is planned once.
///
/// Given: a tree, named alongside a file inside it and alongside itself.
/// When: the arguments are enumerated.
/// Then: the file and the directory each appear once.
///
/// Why it matters: a glob expands beside the directory holding it, and a second copy of a
/// target would be overwritten after the first unlinked it, reporting a survivor that is gone.
#[test]
fn overlapping_arguments_plan_each_target_once() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let inside = root.path().join("secret.txt");
    write(&inside);

    // When:
    let plan = enumerate(&[root.path(), &inside, root.path()]);

    // Then:
    assert_eq!(plan.files, [inside]);
    assert_eq!(plan.directories, [root.path().to_path_buf()]);
    assert_eq!(
        plan.roots(),
        [root.path().to_path_buf(), root.path().join("secret.txt")]
    );
}

/// A special file is planned as a removal, never as a file to overwrite.
///
/// Given: a tree holding a FIFO.
/// When: it is enumerated.
/// Then: the FIFO is a removal and not a file.
///
/// Why it matters: the overwrite opens its target, and opening a FIFO blocks until a writer
/// arrives, so a FIFO among the files would hang the run.
#[test]
fn a_special_file_is_planned_as_a_removal() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let pipe = root.path().join("pipe");
    nix::unistd::mkfifo(&pipe, nix::sys::stat::Mode::S_IRWXU).unwrap();

    // When:
    let plan = enumerate(&[root.path()]);

    // Then:
    assert_eq!(plan.removals, [pipe]);
    assert!(plan.files.is_empty());
}

/// A symlink inside a walked tree is planned as a removal and never followed.
///
/// Given: a tree holding a symlink to a directory outside it.
/// When: the tree is enumerated.
/// Then: the link is a removal, and nothing from the directory it points at is planned.
///
/// Why it matters: following the link would plan, and later destroy, a directory the operator
/// never named.
#[test]
fn a_symlink_inside_the_tree_is_not_followed() {
    // Given:
    let outside = tempfile::tempdir().unwrap();
    write(&outside.path().join("elsewhere.txt"));
    let root = tempfile::tempdir().unwrap();
    let link = root.path().join("out");
    symlink(outside.path(), &link).unwrap();

    // When:
    let plan = enumerate(&[root.path()]);

    // Then:
    assert_eq!(plan.removals, [link]);
    assert!(plan.files.is_empty());
    assert_eq!(plan.directories, [root.path().to_path_buf()]);
}
