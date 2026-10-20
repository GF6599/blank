use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .env("TZ", "UTC")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn main() {
    let commit = git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "none".to_owned());
    let committed = git(&[
        "log",
        "-1",
        "--format=%cd",
        "--date=format-local:%Y-%m-%dT%H:%M:%SZ",
    ])
    .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=BLANK_COMMIT={commit}");
    println!("cargo:rustc-env=BLANK_COMMITTED={committed}");

    // A commit moves HEAD or a branch ref and changes no source file, so without
    // these the build script would keep the commit it saw first. A path that
    // does not exist would make Cargo rerun the script on every build, and
    // --git-path resolves each one inside a worktree too.
    for name in ["HEAD", "refs/heads", "packed-refs"] {
        if let Some(path) = git(&["rev-parse", "--git-path", name])
            && Path::new(&path).exists()
        {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}
