use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use blank::guard::Guard;
use blank::plan::Plan;

fn plan_of(root: &Path) -> Plan {
    Plan::enumerate(&[root.to_path_buf()], &Guard::for_host()).unwrap()
}

fn sized(path: &Path, bytes: usize) -> PathBuf {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![b'x'; bytes]).unwrap();
    path.to_path_buf()
}

/// A plan counts what a run would destroy, its size, and what the covers would write.
///
/// Given: a tree with four files of 0, 10, 1000 and 5000 bytes, a symlink, a FIFO, and two
/// directories.
/// When: it is planned.
/// Then: it counts 4 files, 2 symlinks and special files, 2 directories, and 6010 bytes, the
/// covers over 2 random passes write 18030 bytes, and the files are listed largest first.
///
/// Why it matters: the operator reads these totals to tell whether a command aims at the tree
/// they meant, and the size is the first sign that it does not.
#[test]
fn a_plan_counts_what_it_would_destroy_and_what_the_covers_would_write() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let empty = sized(&root.path().join("empty.txt"), 0);
    let small = sized(&root.path().join("small.txt"), 10);
    let medium = sized(&root.path().join("sub").join("medium.bin"), 1000);
    let large = sized(&root.path().join("sub").join("large.bin"), 5000);
    symlink(&small, root.path().join("link")).unwrap();
    nix::unistd::mkfifo(&root.path().join("pipe"), nix::sys::stat::Mode::S_IRWXU).unwrap();

    // When:
    let plan = plan_of(root.path());
    let statistics = plan.statistics();

    // Then:
    assert_eq!(
        (
            statistics.files,
            statistics.removals,
            statistics.directories
        ),
        (4, 2, 2)
    );
    assert_eq!(statistics.bytes, 6010);
    assert_eq!(statistics.cover_bytes(2), 18030);
    assert_eq!(
        statistics.largest,
        [(large, 5000), (medium, 1000), (small, 10), (empty, 0)]
    );
    assert!(statistics.unreadable.is_empty());
}

/// The largest files are the five biggest, whatever order the walk finds them in.
///
/// Given: seven files, of 1 to 7 kilobytes, named so that the walk meets the small ones first.
/// When: the tree is planned.
/// Then: the largest list holds the five biggest, largest first.
///
/// Why it matters: the review screen shows these files so the operator sees what weighs most in
/// the run, and a list that keeps the first five it met shows whatever sorts first instead.
#[test]
fn the_largest_files_are_the_five_biggest() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let files: Vec<PathBuf> = (1..=7)
        .map(|size| sized(&root.path().join(format!("{size}.bin")), size * 1024))
        .collect();

    // When:
    let plan = plan_of(root.path());

    // Then:
    let expected: Vec<(PathBuf, u64)> = files
        .iter()
        .rev()
        .take(5)
        .map(|path| (path.clone(), fs::metadata(path).unwrap().len()))
        .collect();
    assert_eq!(plan.statistics().largest, expected.as_slice());
}

/// A directory the walk cannot read is named, and it is not counted as one the run destroys.
///
/// Given: a tree with one file and a directory nobody can list.
/// When: it is planned.
/// Then: the statistics name the unreadable directory, count one directory for the root, and
/// count the one file.
///
/// Why it matters: that directory and everything above it survive the run, and totals that
/// count it as destroyed promise more than the run will do.
#[test]
fn a_directory_the_walk_cannot_read_is_named_and_not_counted() {
    if nix::unistd::geteuid().is_root() {
        return;
    }
    // Given:
    let root = tempfile::tempdir().unwrap();
    sized(&root.path().join("file.txt"), 100);
    let locked = root.path().join("locked");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    // When:
    let plan = plan_of(root.path());
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();

    // Then:
    let statistics = plan.statistics();
    assert_eq!(statistics.unreadable, [locked]);
    assert_eq!((statistics.files, statistics.directories), (1, 1));
    assert_eq!(statistics.bytes, 100);
}
