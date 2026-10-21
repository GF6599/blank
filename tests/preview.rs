use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use blank::guard::Guard;
use blank::plan::{Plan, short_size};
use blank::preview::{MAX_IMAGE_FILE_BYTES, PREVIEW_BYTES, Preview, Totals, preview};
use image::{GenericImageView, ImageFormat, Rgb, RgbImage};

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
        largest: vec![(dir.join("sub").join("b.txt"), 20), (dir.join("a.txt"), 10)],
        linked: 0,
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

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Writes a PNG of `width` by `height` in one colour.
fn png(path: &Path, width: u32, height: u32) {
    RgbImage::from_pixel(width, height, Rgb([255, 0, 0]))
        .save(path)
        .unwrap();
}

/// Writes a PNG of one pixel whose header claims `width` by `height` in 8-bit RGBA, with the
/// header checksum kept valid, so a decoder reads the claimed size before any pixel.
fn png_claiming(path: &Path, width: u32, height: u32) {
    png(path, 1, 1);
    let mut bytes = fs::read(path).unwrap();
    // The IHDR chunk follows the signature: its length, its type, 13 bytes of data, a CRC.
    bytes[16..20].copy_from_slice(&width.to_be_bytes());
    bytes[20..24].copy_from_slice(&height.to_be_bytes());
    bytes[24] = 8;
    bytes[25] = 6;
    let crc = crc32(&bytes[12..29]);
    bytes[29..33].copy_from_slice(&crc.to_be_bytes());
    fs::write(path, bytes).unwrap();
}

/// The CRC-32 that PNG chunks carry.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// An image previews as the picture it holds.
///
/// Given: a PNG of 40 by 20 pixels.
/// When: it is previewed.
/// Then: the preview is the decoded picture, a PNG of 40 by 20.
///
/// Why it matters: a photo or a scan is told apart by what it shows, and its hex dump tells the
/// operator nothing about which one is about to be destroyed.
#[test]
fn an_image_previews_as_the_picture_it_holds() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("photo.png");
    png(&path, 40, 20);

    // When:
    let shown = look(&path);

    // Then:
    let Preview::Image(picture) = shown else {
        panic!("an image previews as a picture, not {shown:?}");
    };
    assert_eq!(picture.format, ImageFormat::Png);
    assert_eq!((picture.width, picture.height), (40, 20));
    assert_eq!(picture.image.dimensions(), (40, 20));
}

/// A large image is shrunk for the pane, and keeps the size it has on the disk.
///
/// Given: a PNG of 2000 by 1000 pixels.
/// When: it is previewed.
/// Then: the picture says 2000 by 1000, and the image it holds is at most 1024 pixels wide.
///
/// Why it matters: a pane shows a few hundred pixels, and a full-size photo held for every cursor
/// move costs memory and slows the draw that encodes it for the terminal.
#[test]
fn a_large_image_is_shrunk_for_the_pane_and_keeps_its_size() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("panorama.png");
    png(&path, 2000, 1000);

    // When:
    let shown = look(&path);

    // Then:
    let Preview::Image(picture) = shown else {
        panic!("an image previews as a picture, not {shown:?}");
    };
    assert_eq!((picture.width, picture.height), (2000, 1000));
    assert!(picture.image.width() <= 1024, "{}", picture.image.width());
}

/// An image the preview will not decode falls back to its head, with the reason.
///
/// Given: a PNG whose header claims 6000 by 6000 pixels, a PNG larger than the file cap, and a
/// file that starts like a PNG and holds nothing else.
/// When: each is previewed.
/// Then: each previews as its head bytes, and the reason names the claimed size, the file cap,
/// and the failed decode in turn.
///
/// Why it matters: the files under preview are untrusted, and a header that claims a huge image
/// or a huge file must cost the selector nothing beyond the head it already reads.
#[test]
fn an_image_the_preview_will_not_decode_falls_back_with_the_reason() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let claimed = root.path().join("bomb.png");
    png_claiming(&claimed, 6000, 6000);
    let oversized = root.path().join("huge.png");
    png(&oversized, 1, 1);
    fs::File::options()
        .write(true)
        .open(&oversized)
        .unwrap()
        .set_len(MAX_IMAGE_FILE_BYTES + 1)
        .unwrap();
    let damaged = root.path().join("damaged.png");
    fs::write(&damaged, [PNG_SIGNATURE, &[0; 64]].concat()).unwrap();

    for (path, expected) in [
        (&claimed, "6000 × 6000".to_owned()),
        (&oversized, short_size(MAX_IMAGE_FILE_BYTES)),
        (&damaged, "cannot decode".to_owned()),
    ] {
        // When:
        let shown = look(path);

        // Then:
        let Preview::Undecoded { head, reason } = shown else {
            panic!("{path:?} previews as its head, not {shown:?}");
        };
        assert!(head.starts_with(PNG_SIGNATURE), "{path:?}");
        assert!(reason.contains(&expected), "{path:?}: {reason}");
    }
}

/// An image the cursor has already left is never decoded.
///
/// Given: a PNG, and a stop already requested, as a cursor move requests one.
/// When: it is previewed.
/// Then: nothing comes back.
///
/// Why it matters: each preview runs on a thread of its own, and a cursor held down over a
/// folder of photos would otherwise decode every one it passes at once, at tens of megabytes
/// each.
#[test]
fn an_image_the_cursor_has_left_is_never_decoded() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("photo.png");
    png(&path, 40, 20);

    // When:
    let shown = preview(&path, &Guard::for_host(), &AtomicBool::new(true));

    // Then:
    assert!(shown.is_none(), "{shown:?}");
}
