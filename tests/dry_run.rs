mod common;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};

use common::{run, text};

/// A dry run names every target in run order and destroys nothing.
///
/// Given: a tree holding a symlink and a nested directory with a note in it.
/// When: the tree is dry run.
/// Then: it prints one line per target in the order the run reaches them, then the counts, and
/// every entry is still on the disk.
///
/// Why it matters: a dry run is how an operator checks the aim of a command, so it must show
/// exactly what the run would do and must be safe to point at anything.
#[test]
fn a_dry_run_names_every_target_and_destroys_nothing() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let nested = root.path().join("nested");
    fs::create_dir(&nested).unwrap();
    let note = nested.join("note.txt");
    fs::write(&note, "payload").unwrap();
    let link = root.path().join("link");
    symlink(&note, &link).unwrap();

    // When:
    let outcome = run(&["--dry-run", text(root.path())]);

    // Then:
    assert_eq!(outcome.code, 0, "{}", outcome.stderr);
    assert_eq!(
        outcome.stdout,
        format!(
            "Overwriting {note}\nRemoving {link}\nDeleting directory {nested}\n\
             Deleting directory {root}\n\
             Dry run. 1 file, 1 symlink or special file, and 2 directories. \
             Nothing was destroyed.\n",
            note = text(&note),
            link = text(&link),
            nested = text(&nested),
            root = text(root.path()),
        )
    );
    // And:
    assert_eq!(fs::read_to_string(&note).unwrap(), "payload");
    assert!(link.symlink_metadata().is_ok());
}

/// A dry run that could not read everything says so in its exit code.
///
/// Given: a tree holding a directory nobody can list.
/// When: the tree is dry run.
/// Then: it exits non-zero and names that directory.
///
/// Why it matters: a plan built over a directory nobody could read is missing whatever is
/// inside it, and reporting it as clean would understate what the run would destroy.
#[test]
fn a_dry_run_reports_what_it_could_not_read() {
    if nix::unistd::geteuid().is_root() {
        // Root ignores directory permissions, so the listing cannot fail.
        return;
    }
    // Given:
    let root = tempfile::tempdir().unwrap();
    let locked = root.path().join("locked");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    // When:
    let outcome = run(&["--dry-run", text(root.path())]);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();

    // Then:
    assert_ne!(outcome.code, 0, "{}", outcome.stdout);
    assert!(
        outcome
            .stderr
            .contains(&format!("{}: read directory", text(&locked))),
        "{}",
        outcome.stderr
    );
}

/// A name cannot drive the terminal it is printed on.
///
/// Given: a file whose name holds an escape sequence, a bell and a newline.
/// When: the tree is dry run.
/// Then: the output carries no control character except the line ends, and the readable parts
/// of the name are still there.
///
/// Why it matters: a file name is chosen by whoever wrote the file, and a control character
/// inside it can retitle the window, move the cursor, or hide the rest of the line.
#[test]
fn control_characters_in_a_name_are_escaped() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("evil\x1b]0;hijacked\x07\x08\n.txt"),
        "payload",
    )
    .unwrap();

    // When:
    let outcome = run(&["--dry-run", text(root.path())]);

    // Then:
    assert_eq!(outcome.code, 0, "{}", outcome.stderr);
    for line in outcome.stdout.lines() {
        let control = line
            .chars()
            .find(|&c| c < '\u{20}' || c == '\u{7f}' || ('\u{80}'..='\u{9f}').contains(&c));
        assert_eq!(
            control, None,
            "a control character reached the terminal in {line:?}"
        );
    }
    // And:
    assert!(
        outcome
            .stdout
            .contains("evil\\x1b]0;hijacked\\x07\\x08\\x0a.txt")
    );
}

/// No arguments and no terminal is a usage question.
///
/// Given: no paths on the command line and a pipe on standard input.
/// When: the run starts, bare and with --yes.
/// Then: it prints the usage, naming the dry run, and exits zero.
///
/// Why it matters: a script that calls blank with no arguments must get an answer, not a wait.
#[test]
fn no_arguments_prints_the_usage() {
    for args in [&[][..], &["--yes"]] {
        // When:
        let outcome = run(args);

        // Then:
        assert_eq!(outcome.code, 0, "{args:?}: {}", outcome.stderr);
        assert!(
            outcome.stdout.contains("Usage:"),
            "{args:?}: {}",
            outcome.stdout
        );
        assert!(
            outcome.stdout.contains("--dry-run"),
            "{args:?}: {}",
            outcome.stdout
        );
    }
}
