use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use rustix::fs::{FileType, Mode, OFlags, fstat};

use crate::guard::Guard;
use crate::plan::{Plan, Statistics};
use crate::terminal::{escape, escape_path};

/// How much of a file a preview reads. A preview runs on every cursor move, so it never reads
/// further than one pane can show.
pub const PREVIEW_BYTES: usize = 4096;

/// A tab is one character and a variable number of cells, so the pane draws it as spaces.
const TAB: &str = "    ";

/// What the entry under the cursor holds, read the way the engine reads it, so a symlink is
/// never followed and a special file is never opened.
#[derive(Debug, PartialEq, Eq)]
pub enum Preview {
    /// The first lines, escaped.
    Text(Vec<String>),
    Binary(Vec<u8>),
    /// What a run of the directory would destroy.
    Directory(Totals),
    /// The link text, escaped.
    Symlink(String),
    Special(&'static str),
    /// The guard protects the path, so no walk of it starts.
    Refused(String),
    Unreadable(String),
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
    Some(match read_head(path) {
        Ok(head) => contents(head),
        Err(error) => Preview::Unreadable(error.to_string()),
    })
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

/// The first `PREVIEW_BYTES` of a regular file. The open refuses a symlink and never waits on a
/// FIFO, because the name can be replaced between the check above and this open.
fn read_head(path: &Path) -> std::io::Result<Vec<u8>> {
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let fd = rustix::fs::open(path, flags, Mode::empty())?;
    if FileType::from_raw_mode(fstat(&fd)?.st_mode) != FileType::RegularFile {
        return Err(std::io::Error::other("not a regular file"));
    }
    let mut head = Vec::with_capacity(PREVIEW_BYTES);
    File::from(fd)
        .take(PREVIEW_BYTES as u64)
        .read_to_end(&mut head)?;
    Ok(head)
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
