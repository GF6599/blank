use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use blank::guard::Guard;
use blank::plan::Plan;
use blank::preview::{PREVIEW_BYTES, Preview, Totals, preview};

fn look(path: &Path) -> Preview {
    preview(path, &Guard::for_host(), &AtomicBool::new(false)).expect("nothing stops the preview")
}

/// A text file previews as its first lines, escaped.
///
/// Given: a text file of three lines, one holding a tab and one an escape sequence.
/// When: it is previewed.
/// Then: the preview holds the three lines, the tab as spaces, and the escape as `\x1b`.
///
/// Why it matters: the head of a file is what tells the operator which file it is, and a file
/// is written by whoever wrote it, so a control character in it must not drive the terminal.
#[test]
fn a_text_file_previews_as_its_first_lines() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("notes.txt");
    fs::write(&path, "first line\n\tindented\nred \x1b[31mtext\n").unwrap();

    // When:
    let shown = look(&path);

    // Then:
    let Preview::Text(lines) = shown else {
        panic!("a text file previews as text");
    };
    assert_eq!(lines, ["first line", "    indented", r"red \x1b[31mtext"]);
}

/// A large file is read only as far as its head.
///
/// Given: a text file with a marker just past the preview's read.
/// When: it is previewed.
/// Then: the preview does not hold the marker, and holds no more text than the read allows.
///
/// Why it matters: a preview runs on every cursor move, and a read of a whole file of many
/// gigabytes would freeze the selector for as long as the disk takes.
#[test]
fn a_large_file_is_read_only_as_far_as_its_head() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("large.txt");
    let mut contents = "a".repeat(PREVIEW_BYTES);
    contents.push_str("\nMARKER PAST THE HEAD\n");
    fs::write(&path, contents).unwrap();

    // When:
    let shown = look(&path);

    // Then:
    let Preview::Text(lines) = shown else {
        panic!("a text file previews as text");
    };
    let text = lines.concat();
    assert!(!text.contains("MARKER"));
    assert!(text.len() <= PREVIEW_BYTES);
}

/// A binary file previews as the raw bytes of its head.
///
/// Given: a file that starts with an executable header and holds zero bytes.
/// When: it is previewed.
/// Then: the preview is binary and starts with those bytes.
///
/// Why it matters: binary contents shown as text scramble the pane, and the bytes are what
/// identify the file.
#[test]
fn a_binary_file_previews_as_its_head_bytes() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("program");
    let mut contents = vec![0x7f, b'E', b'L', b'F', 2, 1, 1, 0];
    contents.extend([0u8; 100]);
    fs::write(&path, &contents).unwrap();

    // When:
    let shown = look(&path);

    // Then:
    let Preview::Binary(head) = shown else {
        panic!("a file with zero bytes previews as binary");
    };
    assert_eq!(head, contents);
}

/// A directory previews as the totals a run of it would destroy.
///
/// Given: a directory holding two files of 10 and 20 bytes, a symlink, and a subdirectory.
/// When: it is previewed.
/// Then: the totals count 2 files, 1 symlink or special file, 2 directories and 30 bytes.
///
/// Why it matters: marking a directory destroys everything under it, and the totals are what
/// say how much that is before the mark is made.
#[test]
fn a_directory_previews_as_its_totals() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("project");
    fs::create_dir_all(dir.join("sub")).unwrap();
    fs::write(dir.join("a.txt"), vec![b'x'; 10]).unwrap();
    fs::write(dir.join("sub").join("b.txt"), vec![b'x'; 20]).unwrap();
    symlink("a.txt", dir.join("link")).unwrap();

    // When:
    let shown = look(&dir);

    // Then:
    let expected = Totals {
        files: 2,
        removals: 1,
        directories: 2,
        bytes: 30,
        unreadable: 0,
    };
    assert_eq!(shown, Preview::Directory(expected));
}

/// A symlink previews as its link text and never as what it points at.
///
/// Given: a symlink that points at a FIFO nobody writes to.
/// When: it is previewed.
/// Then: the preview is the link text, and it returns rather than waiting on the FIFO.
///
/// Why it matters: marking a symlink destroys the link and never its target, and a preview that
/// followed it would show a file the run will not touch, or wait for ever on a FIFO.
#[test]
fn a_symlink_previews_as_its_link_text() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let fifo = root.path().join("pipe");
    nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRWXU).unwrap();
    let link = root.path().join("link");
    symlink(&fifo, &link).unwrap();

    // When:
    let shown = look(&link);

    // Then:
    let expected = fifo.to_str().unwrap().to_owned();
    assert_eq!(shown, Preview::Symlink(expected));
}

/// A special file previews as its kind, without being opened.
///
/// Given: a FIFO nobody writes to.
/// When: it is previewed.
/// Then: the preview names it a FIFO, and it returns rather than waiting.
///
/// Why it matters: opening a FIFO waits for a writer that may never come, and the selector would
/// freeze on the cursor that reached it.
#[test]
fn a_special_file_previews_as_its_kind() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let fifo = root.path().join("pipe");
    nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRWXU).unwrap();

    // When:
    let shown = look(&fifo);

    // Then:
    assert_eq!(shown, Preview::Special("FIFO"));
}

/// A file nobody can read, and a protected directory, preview as the reason.
///
/// Given: a file with no permissions, and the protected directory `/usr`.
/// When: each is previewed.
/// Then: the file previews as unreadable, and `/usr` as refused, without a walk of it.
///
/// Why it matters: a pane that stays blank reads as an empty file, and a walk of a system
/// directory would hold a thread for minutes over a path no key can mark.
#[test]
fn a_file_nobody_can_read_and_a_protected_directory_preview_as_the_reason() {
    if nix::unistd::geteuid().is_root() {
        return;
    }
    // Given:
    let root = tempfile::tempdir().unwrap();
    let locked = root.path().join("locked.txt");
    fs::write(&locked, "payload").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    // When:
    let file = look(&locked);
    let system = look(Path::new("/usr"));

    // Then:
    assert!(matches!(file, Preview::Unreadable(_)), "{file:?}");
    assert!(matches!(system, Preview::Refused(_)), "{system:?}");
}

/// A walk that is told to stop returns nothing.
///
/// Given: a tree, and a stop already requested.
/// When: it is enumerated with the stop flag.
/// Then: no plan comes back.
///
/// Why it matters: a preview or a total starts a walk on every cursor move and every mark, and
/// a walk nobody will read must end rather than hold a thread for the size of the tree.
#[test]
fn a_walk_that_is_told_to_stop_returns_nothing() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a.txt"), "payload").unwrap();
    let args: Vec<PathBuf> = vec![root.path().to_path_buf()];

    // When:
    let plan = Plan::enumerate_until_stopped(&args, &Guard::for_host(), &AtomicBool::new(true));

    // Then:
    assert!(matches!(plan, Ok(None)));
}
