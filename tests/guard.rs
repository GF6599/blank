mod common;

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use common::{Outcome, run_with, text};

/// A dry run with HOME pointing at `home`.
fn dry_run_with_home(home: &Path, args: &[&str]) -> Outcome {
    let mut all = vec!["--dry-run"];
    all.extend_from_slice(args);
    run_with(&all, "", |command| {
        command.env("HOME", home);
    })
}

fn assert_refused(outcome: &Outcome, path: &str) {
    assert_ne!(outcome.code, 0, "{path} was accepted:\n{}", outcome.stdout);
    assert!(
        outcome.stderr.contains("refuses to destroy"),
        "the refusal for {path} does not say so:\n{}",
        outcome.stderr
    );
    assert!(
        outcome.stdout.is_empty(),
        "a refused command line still planned:\n{}",
        outcome.stdout
    );
}

fn home_in(parent: &Path) -> PathBuf {
    let home = parent.join("home");
    fs::create_dir(&home).unwrap();
    home
}

/// A run refuses the paths that hold everything else.
///
/// Given: a home directory the test owns.
/// When: the filesystem root, a system directory, the home directory, or the directory holding
/// home is named.
/// Then: the run refuses it and plans nothing.
///
/// Why it matters: blank has no undo, so a mistyped argument that reaches the whole machine is
/// the one mistake nothing later in the run can soften.
#[test]
fn a_protected_path_is_refused() {
    // Given:
    let scratch = tempfile::tempdir().unwrap();
    let home = home_in(scratch.path());

    for path in ["/", "/etc", text(&home), text(scratch.path())] {
        // When:
        let outcome = dry_run_with_home(&home, &[path]);

        // Then:
        assert_refused(&outcome, path);
    }
}

/// One refused argument stops every argument beside it.
///
/// Given: an ordinary tree with a file in it.
/// When: it is named alongside the filesystem root.
/// Then: the run refuses, and the ordinary tree is not planned either.
///
/// Why it matters: a command line that names a protected path is a mistake, so the arguments
/// beside it are not to be trusted as deliberate either.
#[test]
fn a_refused_argument_stops_every_argument_beside_it() {
    // Given:
    let scratch = tempfile::tempdir().unwrap();
    let home = home_in(scratch.path());
    let tree = tempfile::tempdir().unwrap();
    fs::write(tree.path().join("victim.txt"), "payload").unwrap();

    // When:
    let outcome = dry_run_with_home(&home, &[text(tree.path()), "/"]);

    // Then:
    assert_refused(&outcome, "/");
}

/// The home directory stays protected whatever the environment says.
///
/// Given: the home directory the user database records.
/// When: it is named with HOME missing, and again with HOME pointing elsewhere.
/// Then: the run refuses it both times.
///
/// Why it matters: HOME belongs to whoever started the process, and a run from cron has a
/// minimal environment. Reading only that variable drops the protection when it is missing and
/// aims it at the wrong directory when it is wrong.
#[test]
fn the_home_guard_does_not_depend_on_the_environment() {
    // Given:
    let recorded = nix::unistd::User::from_uid(nix::unistd::getuid())
        .unwrap()
        .expect("the test user is in the user database")
        .dir;
    let elsewhere = tempfile::tempdir().unwrap();

    // When:
    let missing = run_with(&["--dry-run", text(&recorded)], "", |command| {
        command.env_remove("HOME");
    });
    let pointing_elsewhere = dry_run_with_home(elsewhere.path(), &[text(&recorded)]);

    // Then:
    assert_refused(&missing, text(&recorded));
    assert_refused(&pointing_elsewhere, text(&recorded));
}

/// A protected directory is refused under every name that reaches it.
///
/// Given: a symlink to the directory that holds home.
/// When: home is named through that symlink.
/// Then: the run refuses it and names the path it resolves to.
///
/// Why it matters: a machine whose /home is a symlink has two names for one directory, and a
/// guard that compares names protects only the one it was told about.
#[test]
fn a_protected_path_is_refused_through_a_symlink() {
    // Given:
    let scratch = tempfile::tempdir().unwrap();
    let home = home_in(scratch.path());
    let aliases = tempfile::tempdir().unwrap();
    let alias = aliases.path().join("alias");
    symlink(scratch.path(), &alias).unwrap();

    // When:
    let aliased = alias.join("home");
    let outcome = dry_run_with_home(&home, &[text(&aliased)]);

    // Then:
    assert_refused(&outcome, text(&aliased));
    assert!(outcome.stderr.contains("resolves to"), "{}", outcome.stderr);
}

/// A path that climbs back out with `..` is judged by where it lands.
///
/// Given: a project directory inside home.
/// When: home is named as the project followed by `..`, as an absolute path and as a relative
/// one from inside the project.
/// Then: the run refuses both.
///
/// Why it matters: a guard that compares the text as typed sees a path inside home and accepts
/// it, while the walk would destroy home itself.
#[test]
fn a_path_that_climbs_into_a_protected_directory_is_refused() {
    // Given:
    let scratch = tempfile::tempdir().unwrap();
    let home = home_in(scratch.path());
    let project = home.join("project");
    fs::create_dir(&project).unwrap();

    // When:
    let climbed = format!("{}/..", text(&project));
    let absolute = dry_run_with_home(&home, &[&climbed]);
    let relative = run_with(&["--dry-run", ".."], "", |command| {
        command.env("HOME", &home).current_dir(&project);
    });

    // Then:
    assert_refused(&absolute, &climbed);
    assert_refused(&relative, "..");
}

/// A symlink to a protected path is removed rather than refused.
///
/// Given: a symlink pointing at the home directory.
/// When: the link is named.
/// Then: the run plans to unlink the link and nothing else.
///
/// Why it matters: unlinking a link destroys only the link, and refusing it would make every
/// link into a protected directory impossible to clean up.
#[test]
fn a_symlink_to_a_protected_path_is_still_removable() {
    // Given:
    let scratch = tempfile::tempdir().unwrap();
    let home = home_in(scratch.path());
    let links = tempfile::tempdir().unwrap();
    let link = links.path().join("link-to-home");
    symlink(&home, &link).unwrap();

    // When:
    let outcome = dry_run_with_home(&home, &[text(&link)]);

    // Then:
    assert_eq!(outcome.code, 0, "{}", outcome.stderr);
    assert_eq!(
        outcome.stdout,
        format!(
            "Removing {}\nDry run. 0 files, 1 symlink or special file, and 0 directories. \
             Nothing was destroyed.\n",
            text(&link)
        )
    );
}

/// A path inside the home directory is an ordinary target.
///
/// Given: a project directory under home, holding a note.
/// When: the project is named.
/// Then: it is accepted and the note is planned.
///
/// Why it matters: the guard refuses a path that holds home, and the same comparison read
/// backwards would refuse everything a person owns.
#[test]
fn a_path_inside_the_home_directory_is_accepted() {
    // Given:
    let scratch = tempfile::tempdir().unwrap();
    let home = home_in(scratch.path());
    let project = home.join("project");
    fs::create_dir(&project).unwrap();
    let note = project.join("note.txt");
    fs::write(&note, "payload").unwrap();

    // When:
    let outcome = dry_run_with_home(&home, &[text(&project)]);

    // Then:
    assert_eq!(outcome.code, 0, "{}", outcome.stderr);
    assert!(
        outcome
            .stdout
            .contains(&format!("Overwriting {}\n", text(&note))),
        "{}",
        outcome.stdout
    );
}
