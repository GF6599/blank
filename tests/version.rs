use std::process::Command;

fn git(args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .env("TZ", "UTC")
        .output()
        .expect("git runs");
    assert!(output.status.success(), "git {args:?} failed");
    String::from_utf8(output.stdout)
        .expect("git prints UTF-8")
        .trim()
        .to_owned()
}

/// The version flag names the release, the commit it was built from, and that commit's date.
///
/// Given: a binary built from this checkout.
/// When: it runs with `--version`.
/// Then: it prints the crate version, the full commit hash at HEAD, and the commit date in UTC.
///
/// Why it matters: a bug report must identify the exact build, and a stale commit would point
/// the reader at code the binary does not contain.
#[test]
fn version_names_the_release_the_commit_and_its_date() {
    // Given:
    let head = git(&["rev-parse", "HEAD"]);
    let committed = git(&[
        "log",
        "-1",
        "--format=%cd",
        "--date=format-local:%Y-%m-%dT%H:%M:%SZ",
    ]);

    // When:
    let output = Command::new(env!("CARGO_BIN_EXE_blank"))
        .arg("--version")
        .output()
        .expect("blank runs");

    // Then:
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!(
            "blank {} (commit: {head}, built: {committed})\n",
            env!("CARGO_PKG_VERSION")
        )
    );
}
