use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};
use std::os::fd::{AsFd, BorrowedFd};

use nix::sys::stat::{FchmodatFlags, Mode as ChmodMode, fchmodat};
use rand::Rng;
use rustix::fs::{
    AtFlags, FileType, Mode, OFlags, RenameFlags, fcntl_setfl, fstat, openat, renameat_with,
    unlinkat,
};
use rustix::io::Errno;

use crate::engine::{CHANGED, COVER_CHUNK, TargetError, identity_at, identity_of};
use crate::plan::Identity;

/// What a file that refuses to open for writing is set to, as `shred -f` does. Nothing narrower
/// lets the write through, and the entry is unlinked moments later.
const FORCED_MODE: ChmodMode = ChmodMode::S_IRUSR.union(ChmodMode::S_IWUSR);

/// How many random names the rename tries before it gives up on a crowded directory.
const OBSCURE_ATTEMPTS: usize = 128;

type Fill = fn(&mut [u8]);

/// A fresh stream for every chunk, so a pass never repeats a block pattern that would survive
/// as a recognisable stripe. `rand::rng` is a cryptographic generator the operating system
/// seeds: a cover has to be unpredictable, so recovering one block says nothing about another.
fn random_fill(buf: &mut [u8]) {
    rand::rng().fill_bytes(buf);
}

fn zero_fill(buf: &mut [u8]) {
    buf.fill(0);
}

/// Destroys one regular file: `passes` random covers, one zero cover, a meaningless name, and an
/// unlink. Each step acts only on the file the plan found under `name`.
///
/// A failure before the first byte lands leaves the file as it was. A failure after one leaves
/// it on the disk holding covers, and the error says so, because a caller reads a target the run
/// did not destroy as one that still holds its contents.
pub(crate) fn overwrite(
    parent: BorrowedFd<'_>,
    name: &OsStr,
    planned: Identity,
    passes: u32,
) -> Result<(), TargetError> {
    let (mut file, size) =
        open_for_overwrite(parent, name, planned).map_err(TargetError::intact)?;
    let mut buf = vec![0; COVER_CHUNK.min(usize::try_from(size).unwrap_or(COVER_CHUNK))];
    let mut covered = 0;
    // An empty file has no contents to cover, and a flush to the device costs milliseconds
    // on macOS, which a tree of many empty files multiplies into minutes.
    let covers = if size == 0 { 0 } else { passes as usize + 1 };
    let fills = std::iter::repeat_n(random_fill as Fill, passes as usize)
        .chain(std::iter::once(zero_fill as Fill))
        .take(covers);
    for (pass, fill) in fills.enumerate() {
        cover(&mut file, size, fill, &mut buf, &mut covered)
            .map_err(|error| TargetError::after(covered, format!("cover {}: {error}", pass + 1)))?;
    }
    drop(file);
    let renamed = obscure_name(parent, name, planned)
        .map_err(|reason| TargetError::after(covered, reason))?;
    let removed = match identity_at(parent, &renamed) {
        Ok(found) if found == planned => unlinkat(parent, &renamed, AtFlags::empty())
            .map_err(|error| format!("remove {}: {error}", renamed.to_string_lossy())),
        Ok(_) => Err(CHANGED.to_owned()),
        Err(error) => Err(format!("remove {}: {error}", renamed.to_string_lossy())),
    };
    removed.map_err(|reason| TargetError::after(covered, reason))
}

/// Opens `name` for writing without following a symlink, and only if it is still the regular
/// file the plan found. `O_NONBLOCK` makes a FIFO that took the name since the plan fail the
/// open, where a plain open would wait for a reader that never comes.
fn open_for_overwrite(
    parent: BorrowedFd<'_>,
    name: &OsStr,
    planned: Identity,
) -> Result<(File, u64), String> {
    let flags = OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let opened = match openat(parent, name, flags, Mode::empty()) {
        Err(refused) if refused == Errno::ACCESS || refused == Errno::PERM => {
            force_mode(parent, name, planned).map_err(|chmod| {
                format!("open for writing: {refused} (chmod also failed: {chmod})")
            })?;
            openat(parent, name, flags, Mode::empty())
        }
        other => other,
    };
    let fd = opened.map_err(|error| format!("open for writing: {error}"))?;
    let stat = fstat(&fd).map_err(|error| format!("stat the open file: {error}"))?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err("not a regular file".to_owned());
    }
    if identity_of(&stat) != planned {
        return Err(CHANGED.to_owned());
    }
    fcntl_setfl(&fd, OFlags::empty()).map_err(|error| format!("clear O_NONBLOCK: {error}"))?;
    let size = u64::try_from(stat.st_size).unwrap_or(0);
    Ok((File::from(fd), size))
}

/// Changes the mode only of the file the plan found. This goes through libc rather than a raw
/// system call: before Linux 6.6 the kernel has no no-follow mode change, and glibc 2.32 and
/// later emulates it through an `O_PATH` descriptor.
fn force_mode(parent: BorrowedFd<'_>, name: &OsStr, planned: Identity) -> Result<(), String> {
    match identity_at(parent, name) {
        Ok(found) if found == planned => {}
        Ok(_) => return Err(CHANGED.to_owned()),
        Err(error) => return Err(error.to_string()),
    }
    fchmodat(
        parent.as_fd(),
        name,
        FORCED_MODE,
        FchmodatFlags::NoFollowSymlink,
    )
    .map_err(|error| error.to_string())
}

/// One pass over the first `size` bytes, then a flush to the device. Without the flush the
/// passes collapse into one write of the last pattern, because the page cache holds every
/// earlier pass and only the final state reaches the device. `covered` counts every byte that
/// landed, which is what separates a damaged target from an intact one.
fn cover(
    file: &mut File,
    size: u64,
    fill: Fill,
    buf: &mut [u8],
    covered: &mut u64,
) -> io::Result<()> {
    file.seek(SeekFrom::Start(0))?;
    let mut written = 0;
    while written < size {
        let length = usize::try_from(size - written).map_or(buf.len(), |left| left.min(buf.len()));
        let chunk = &mut buf[..length];
        fill(chunk);
        let mut landed = 0;
        while landed < chunk.len() {
            let count = file.write(&chunk[landed..])?;
            if count == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            landed += count;
            *covered += count as u64;
        }
        written += length as u64;
    }
    file.sync_all()
}

/// Renames the entry to a meaningless name of the same length. The contents are already gone,
/// but a directory entry keeps the name until it is unlinked, and a name is often as telling as
/// the file. The rename never replaces an existing entry, because on Unix a rename replaces its
/// destination, and a name drawn onto an entry nobody selected would destroy that entry too.
pub(crate) fn obscure_name(
    parent: BorrowedFd<'_>,
    name: &OsStr,
    planned: Identity,
) -> Result<OsString, String> {
    let length = name.len();
    for _ in 0..OBSCURE_ATTEMPTS {
        let mut raw = vec![0; length.div_ceil(2)];
        random_fill(&mut raw);
        let mut hex: String = raw.iter().map(|byte| format!("{byte:02x}")).collect();
        hex.truncate(length);
        let renamed = OsString::from(hex);
        if renamed == name {
            continue;
        }
        match identity_at(parent, name) {
            Ok(found) if found == planned => {}
            Ok(_) => return Err(CHANGED.to_owned()),
            Err(error) => return Err(format!("rename: {error}")),
        }
        match renameat_with(parent, name, parent, &renamed, RenameFlags::NOREPLACE) {
            Ok(()) => return Ok(renamed),
            Err(Errno::EXIST) => continue,
            Err(error) => return Err(format!("rename: {error}")),
        }
    }
    Err(format!(
        "rename: no unused name after {OBSCURE_ATTEMPTS} attempts"
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::fd::OwnedFd;
    use std::os::unix::fs::{MetadataExt, symlink};
    use std::path::Path;

    use super::*;

    fn open_dir(path: &Path) -> OwnedFd {
        rustix::fs::open(path, OFlags::RDONLY | OFlags::DIRECTORY, Mode::empty()).unwrap()
    }

    fn identity_of_path(path: &Path) -> Identity {
        let metadata = fs::symlink_metadata(path).unwrap();
        Identity::new(metadata.dev(), metadata.ino())
    }

    /// A random cover is random, not a constant.
    ///
    /// Given: a buffer of 36 KiB.
    /// When: it is filled for a random cover.
    /// Then: close to every byte value appears in it.
    ///
    /// Why it matters: a constant or a short repeating pattern would let one recovered block
    /// describe every other, and the overwrite would still look finished.
    #[test]
    fn a_random_cover_uses_close_to_every_byte_value() {
        // Given:
        let mut buf = vec![0; 36 * 1024];

        // When:
        random_fill(&mut buf);

        // Then:
        let mut present = [false; 256];
        for &byte in &buf {
            present[usize::from(byte)] = true;
        }
        let distinct = present.iter().filter(|&&seen| seen).count();
        assert!(distinct >= 200, "only {distinct} distinct byte values");
    }

    /// The name is replaced with a meaningless one of the same length before the unlink.
    ///
    /// Given: a file with a telling name.
    /// When: its name is obscured.
    /// Then: the old name is gone, and a different name of the same length in the same
    /// directory holds the same file.
    ///
    /// Why it matters: a directory entry keeps the name after the contents are gone.
    #[test]
    fn the_name_is_replaced_by_a_meaningless_one_of_the_same_length() {
        // Given:
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("resignation-letter.txt");
        fs::write(&path, "payload").unwrap();
        let planned = identity_of_path(&path);
        let dir = open_dir(root.path());

        // When:
        let renamed = obscure_name(dir.as_fd(), path.file_name().unwrap(), planned).unwrap();

        // Then:
        assert!(path.symlink_metadata().is_err());
        assert_ne!(renamed, "resignation-letter.txt");
        assert_eq!(renamed.len(), "resignation-letter.txt".len());
        assert_eq!(identity_of_path(&root.path().join(&renamed)), planned);
    }

    /// The rename never replaces an entry nobody selected.
    ///
    /// Given: every one-character hexadecimal name is taken, beside a one-character target.
    /// When: the target's name is obscured.
    /// Then: the rename fails and every entry is still there.
    ///
    /// Why it matters: a rename replaces its destination on Unix, so a name drawn onto a
    /// sibling would destroy the sibling.
    #[test]
    fn the_rename_never_replaces_a_sibling() {
        // Given:
        let root = tempfile::tempdir().unwrap();
        let siblings: Vec<String> = (0..16).map(|digit| format!("{digit:x}")).collect();
        for name in &siblings {
            fs::write(root.path().join(name), format!("keep-{name}")).unwrap();
        }
        let target = root.path().join("x");
        fs::write(&target, "target").unwrap();
        let dir = open_dir(root.path());

        // When:
        let renamed = obscure_name(dir.as_fd(), OsStr::new("x"), identity_of_path(&target));

        // Then:
        assert!(renamed.is_err(), "{renamed:?}");
        for name in siblings.iter().map(String::as_str).chain(["x"]) {
            assert!(root.path().join(name).exists(), "{name} is gone");
        }
    }

    /// The open refuses a symlink even when it points at the planned file.
    ///
    /// Given: a planned file, moved aside and replaced by a symlink pointing at it.
    /// When: the old name is opened for the overwrite.
    /// Then: the open is refused, although the identity behind the link matches the plan.
    ///
    /// Why it matters: this is the one case the identity check cannot see, so only the refusal
    /// to follow the link keeps the run acting on the entry it planned.
    #[test]
    fn the_open_refuses_a_symlink_to_the_planned_file() {
        // Given:
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("planned.txt");
        fs::write(&path, "planned").unwrap();
        let planned = identity_of_path(&path);
        let moved = root.path().join("moved.txt");
        fs::rename(&path, &moved).unwrap();
        symlink(&moved, &path).unwrap();
        let dir = open_dir(root.path());

        // When:
        let opened = open_for_overwrite(dir.as_fd(), OsStr::new("planned.txt"), planned);

        // Then:
        assert!(opened.is_err());
        assert_eq!(fs::read_to_string(&moved).unwrap(), "planned");
    }
}
