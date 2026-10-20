use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

use rustix::fs::{FileType, Mode, OFlags, fchmod, fcntl_setfl, fstat};

use crate::plan::Incomplete;
use crate::terminal::{escape, escape_path};

/// The manifest names every path a run destroyed, so nobody but its owner may read it.
const MODE: Mode = Mode::RUSR.union(Mode::WUSR);

/// The record of a run: one line per target, each a state, a path, and for a survivor the
/// reason, separated by tabs. A tab or a line break inside a name is escaped, so one line
/// always holds one target.
pub struct Manifest {
    file: File,
    /// The first write that failed. The run goes on, because stopping halfway through a tree
    /// to protect its record leaves more on the disk, and the caller learns of it at the end.
    failed: Option<io::Error>,
}

impl Manifest {
    /// Opens `path` for append. A symlink is refused, because appending through one writes the
    /// manifest into a file the operator never named. An existing file is narrowed to the owner,
    /// because the mode an open applies belongs only to a file it creates.
    pub fn open(path: &Path) -> io::Result<Manifest> {
        let flags = OFlags::WRONLY
            | OFlags::CREATE
            | OFlags::APPEND
            | OFlags::NOFOLLOW
            | OFlags::NONBLOCK
            | OFlags::CLOEXEC;
        let fd = rustix::fs::open(path, flags, MODE)?;
        if FileType::from_raw_mode(fstat(&fd)?.st_mode) != FileType::RegularFile {
            return Err(io::Error::other("not a regular file"));
        }
        fchmod(&fd, MODE)?;
        fcntl_setfl(&fd, OFlags::APPEND)?;
        Ok(Manifest {
            file: File::from(fd),
            failed: None,
        })
    }

    /// Written as each target settles, so a run that ends without its report still leaves the
    /// record of what it reached.
    pub fn destroyed(&mut self, path: &Path) {
        self.line(format!("destroyed\t{}\n", escape_path(path)));
    }

    pub fn survivors(&mut self, incomplete: &Incomplete) {
        for failure in incomplete.survivors() {
            let state = if failure.damaged() {
                "damaged"
            } else {
                "intact"
            };
            self.line(format!(
                "{state}\t{}\t{}\n",
                escape_path(failure.path()),
                escape(failure.reason())
            ));
        }
    }

    pub fn finish(self) -> io::Result<()> {
        self.failed.map_or(Ok(()), Err)
    }

    fn line(&mut self, line: String) {
        if self.failed.is_none()
            && let Err(error) = self.file.write_all(line.as_bytes())
        {
            self.failed = Some(error);
        }
    }
}
