use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use nix::unistd::{User, getuid};

use crate::terminal::escape_path;

/// The directories an operating system installs itself into. Naming one is never what the
/// caller meant, and the list costs nothing to a caller who meant something narrower.
const SYSTEM_PATHS: &[&str] = &[
    "/bin",
    "/boot",
    "/cores",
    "/dev",
    "/etc",
    "/home",
    "/lib",
    "/lib32",
    "/lib64",
    "/opt",
    "/proc",
    "/root",
    "/run",
    "/sbin",
    "/srv",
    "/sys",
    "/usr",
    "/var",
    "/Applications",
    "/Library",
    "/System",
    "/Users",
    "/Volumes",
    "/private",
];

/// Reports what a run refuses to destroy. There is no flag to override a refusal: the answer to
/// one is to name a narrower path, which is also the answer to the mistake the guard catches.
pub struct Guard {
    protected: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct Refusal(String);

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refusal {}

impl Guard {
    /// Protects the system paths and the home directory of this host. HOME belongs to whoever
    /// started the process: missing, it drops the protection, and wrong, it aims the protection
    /// at a directory nobody lives in. The user database is the answer the environment cannot
    /// move, and HOME still counts, because a caller who sets it on purpose means that
    /// directory.
    pub fn for_host() -> Guard {
        let mut protected: Vec<PathBuf> = SYSTEM_PATHS.iter().map(PathBuf::from).collect();
        if let Some(home) = env::var_os("HOME").filter(|home| !home.is_empty()) {
            protected.push(PathBuf::from(home));
        }
        if let Ok(Some(user)) = User::from_uid(getuid())
            && !user.dir.as_os_str().is_empty()
        {
            protected.push(user.dir);
        }
        // /etc is a symlink to /private/etc on macOS, and a home directory is often a symlink to
        // wherever its disk is mounted. Both names reach one directory, so both are listed.
        let resolved: Vec<PathBuf> = protected
            .iter()
            .filter_map(|path| fs::canonicalize(path).ok())
            .collect();
        for path in resolved {
            if !protected.contains(&path) {
                protected.push(path);
            }
        }
        Guard { protected }
    }

    /// Refuses a protected path, and also a path that holds one, because destroying a directory
    /// destroys everything under it. That second rule is the one that works: a list of names
    /// cannot know that home lives under /mnt/nfs on this host, and comparing against home
    /// itself does.
    ///
    /// The path must be absolute and free of `.` and `..`. Every symlink above the last
    /// component is resolved and checked too. The last component is left alone, because a
    /// symlink target is unlinked and never followed.
    pub fn check(&self, path: &Path) -> Result<(), Refusal> {
        refuse(path, &self.protected)?;
        let real = resolve_parent(path);
        if real == path {
            return Ok(());
        }
        refuse(&real, &self.protected).map_err(|refusal| {
            Refusal(format!(
                "{} resolves to {}: {refusal}",
                escape_path(path),
                escape_path(&real)
            ))
        })
    }
}

fn resolve_parent(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => fs::canonicalize(parent)
            .map(|parent| parent.join(name))
            .unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    }
}

fn refuse(path: &Path, protected: &[PathBuf]) -> Result<(), Refusal> {
    if path.parent().is_none() {
        return Err(Refusal(format!(
            "{} is the root of a filesystem, which blank refuses to destroy",
            escape_path(path)
        )));
    }
    for guarded in protected {
        if path == guarded {
            return Err(Refusal(format!(
                "{} is a protected path, which blank refuses to destroy",
                escape_path(path)
            )));
        }
        if guarded.starts_with(path) {
            return Err(Refusal(format!(
                "{} holds the protected path {}, so blank refuses to destroy it",
                escape_path(path),
                escape_path(guarded)
            )));
        }
    }
    Ok(())
}
