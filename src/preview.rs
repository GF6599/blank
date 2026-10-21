use std::fs::{self, File};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use image::{DynamicImage, ImageDecoder, ImageError, ImageFormat, ImageReader, Limits};
use rustix::fs::{FileType, Mode, OFlags, fstat};

use crate::guard::Guard;
use crate::plan::{Plan, Statistics, short_size};
use crate::terminal::{escape, escape_path};

/// How much of a file a preview reads. A preview runs on every cursor move, so it never reads
/// further than one pane can show.
pub const PREVIEW_BYTES: usize = 4096;

/// The largest image file a preview decodes. A larger one previews as its head.
pub const MAX_IMAGE_FILE_BYTES: u64 = 32 * 1024 * 1024;
/// The most memory the decoded pixels of a preview may take. A small file can claim a huge
/// image in its header, so this is checked against the header before any pixel is decoded.
pub const MAX_DECODED_BYTES: u64 = 128 * 1024 * 1024;
/// The longest side of the image a preview keeps. A pane shows a few hundred pixels, and the
/// image is encoded for the terminal on every draw at a new size.
const PREVIEW_IMAGE_SIDE: u32 = 1024;
/// How long an image waits before its decode. A cursor held down leaves an entry sooner, and
/// its next move stops the decode before it starts, so only the image it rests on is decoded.
const IMAGE_SETTLE: Duration = Duration::from_millis(120);
/// The formats `image` is built with.
const IMAGE_FORMATS: [ImageFormat; 6] = [
    ImageFormat::Png,
    ImageFormat::Jpeg,
    ImageFormat::Gif,
    ImageFormat::WebP,
    ImageFormat::Bmp,
    ImageFormat::Tiff,
];

/// A tab is one character and a variable number of cells, so the pane draws it as spaces.
const TAB: &str = "    ";

/// What the entry under the cursor holds, read the way the engine reads it, so a symlink is
/// never followed and a special file is never opened.
#[derive(Debug, PartialEq)]
pub enum Preview {
    /// The first lines, escaped.
    Text(Vec<String>),
    Binary(Vec<u8>),
    Image(Picture),
    /// An image the preview would not decode, as its head, and why.
    Undecoded {
        head: Vec<u8>,
        reason: String,
    },
    /// What a run of the directory would destroy.
    Directory(Totals),
    /// The link text, escaped.
    Symlink(String),
    Special(&'static str),
    /// The guard protects the path, so no walk of it starts.
    Refused(String),
    Unreadable(String),
}

/// An image file, decoded and shrunk to at most `PREVIEW_IMAGE_SIDE` on its longest side.
#[derive(Debug, PartialEq)]
pub struct Picture {
    pub format: ImageFormat,
    /// The size the file holds, before the shrink.
    pub width: u32,
    pub height: u32,
    pub image: DynamicImage,
}

/// The counts of a plan and its largest files, without the rest of the paths it holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub files: usize,
    pub removals: usize,
    pub directories: usize,
    pub bytes: u64,
    /// The directories the walk could not list, which survive a run.
    pub unreadable: usize,
    /// The largest files, biggest first.
    pub largest: Vec<(PathBuf, u64)>,
    /// The files with more than one name.
    pub linked: usize,
}

impl Totals {
    pub fn of(statistics: &Statistics<'_>) -> Totals {
        Totals {
            files: statistics.files,
            removals: statistics.removals,
            directories: statistics.directories,
            bytes: statistics.bytes,
            unreadable: statistics.unreadable.len(),
            largest: statistics.largest.to_vec(),
            linked: statistics.linked.len(),
        }
    }

    /// The totals a run of `paths` would destroy, or nothing once `stop` is set.
    pub fn of_paths(
        paths: &[PathBuf],
        guard: &Guard,
        stop: &AtomicBool,
    ) -> Option<Result<Totals, String>> {
        match Plan::enumerate_until_stopped(paths, guard, stop) {
            Ok(Some(plan)) => Some(Ok(Totals::of(&plan.statistics()))),
            Ok(None) => None,
            Err(refusal) => Some(Err(refusal.to_string())),
        }
    }
}

/// Previews `path`, or returns nothing once `stop` is set during a directory walk.
pub fn preview(path: &Path, guard: &Guard, stop: &AtomicBool) -> Option<Preview> {
    let kind = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata.file_type(),
        Err(error) => return Some(Preview::Unreadable(error.to_string())),
    };
    if kind.is_symlink() {
        return Some(match fs::read_link(path) {
            Ok(target) => Preview::Symlink(escape_path(&target)),
            Err(error) => Preview::Unreadable(error.to_string()),
        });
    }
    if kind.is_dir() {
        return Totals::of_paths(&[path.to_path_buf()], guard, stop).map(|totals| match totals {
            Ok(totals) => Preview::Directory(totals),
            Err(refusal) => Preview::Refused(refusal),
        });
    }
    if !kind.is_file() {
        return Some(Preview::Special(special_kind(&kind)));
    }
    read_file(path, stop).unwrap_or_else(|error| Some(Preview::Unreadable(error.to_string())))
}

/// Reads a regular file, or returns nothing for an image the cursor left before the decode.
fn read_file(path: &Path, stop: &AtomicBool) -> std::io::Result<Option<Preview>> {
    let (file, size) = open_regular(path)?;
    let mut head = Vec::with_capacity(PREVIEW_BYTES);
    (&file).take(PREVIEW_BYTES as u64).read_to_end(&mut head)?;
    let format = match image::guess_format(&head) {
        Ok(format) if IMAGE_FORMATS.contains(&format) => format,
        _ => return Ok(Some(contents(head))),
    };
    if !settled(stop) {
        return Ok(None);
    }
    Ok(Some(match decode(file, size, format) {
        Ok(picture) => Preview::Image(picture),
        Err(reason) => Preview::Undecoded { head, reason },
    }))
}

/// Waits for the cursor to settle, and says whether it stayed on this entry.
fn settled(stop: &AtomicBool) -> bool {
    if stop.load(Ordering::Relaxed) {
        return false;
    }
    thread::sleep(IMAGE_SETTLE);
    !stop.load(Ordering::Relaxed)
}

/// Decodes the image in `file`, which the head already read from, or says why it did not.
fn decode(mut file: File, size: u64, format: ImageFormat) -> Result<Picture, String> {
    if size > MAX_IMAGE_FILE_BYTES {
        return Err(format!(
            "the file is {}, larger than the {} a preview decodes",
            short_size(size),
            short_size(MAX_IMAGE_FILE_BYTES)
        ));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|error| format!("cannot read: {error}"))?;
    let mut reader = ImageReader::with_format(BufReader::new(file), format);
    // The limits would refuse a huge header with a reason that names no size, so the header is
    // read without them, checked here, and the limits apply to the pixels.
    reader.no_limits();
    let mut decoder = reader.into_decoder().map_err(cannot_decode)?;
    let (width, height) = decoder.dimensions();
    if decoder.total_bytes() > MAX_DECODED_BYTES {
        return Err(format!(
            "the image is {width} × {height}, larger than the {} a preview decodes",
            short_size(MAX_DECODED_BYTES)
        ));
    }
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    decoder.set_limits(limits).map_err(cannot_decode)?;
    let image = DynamicImage::from_decoder(decoder).map_err(cannot_decode)?;
    let image = if width.max(height) > PREVIEW_IMAGE_SIDE {
        image.thumbnail(PREVIEW_IMAGE_SIDE, PREVIEW_IMAGE_SIDE)
    } else {
        image
    };
    Ok(Picture {
        format,
        width,
        height,
        image,
    })
}

fn cannot_decode(error: ImageError) -> String {
    format!("cannot decode the image: {error}")
}

fn special_kind(kind: &fs::FileType) -> &'static str {
    use std::os::unix::fs::FileTypeExt;
    if kind.is_fifo() {
        "FIFO"
    } else if kind.is_socket() {
        "socket"
    } else if kind.is_char_device() {
        "character device"
    } else if kind.is_block_device() {
        "block device"
    } else {
        "special file"
    }
}

/// A regular file and its size. The open refuses a symlink and never waits on a FIFO, because
/// the name can be replaced between the check above and this open.
fn open_regular(path: &Path) -> std::io::Result<(File, u64)> {
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let fd = rustix::fs::open(path, flags, Mode::empty())?;
    let stat = fstat(&fd)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(std::io::Error::other("not a regular file"));
    }
    let size = u64::try_from(stat.st_size).unwrap_or(0);
    Ok((File::from(fd), size))
}

/// Text when the head is UTF-8 with no zero byte, allowing a character cut at the end of the
/// read, and binary otherwise.
fn contents(head: Vec<u8>) -> Preview {
    if head.contains(&0) {
        return Preview::Binary(head);
    }
    let text = match std::str::from_utf8(&head) {
        Ok(text) => text,
        Err(error) if error.error_len().is_none() => {
            std::str::from_utf8(&head[..error.valid_up_to()]).expect("valid up to this point")
        }
        Err(_) => return Preview::Binary(head),
    };
    let lines = text
        .lines()
        .map(|line| escape(&line.replace('\t', TAB)))
        .collect();
    Preview::Text(lines)
}
