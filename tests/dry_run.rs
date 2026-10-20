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
/// Then: it exits non-zero, names that directory, and does not list it as a directory to delete.
///
/// Why it matters: a plan built over a directory nobody could read is missing whatever is
/// inside it, and reporting it as clean would understate what the run would destroy. The run
/// leaves that directory alone, so listing it would promise a deletion that never happens.
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
    assert!(
        !outcome
            .stdout
            .contains(&format!("Deleting directory {}\n", text(&locked))),
        "{}",
        outcome.stdout
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

/// A name cannot disguise itself as a different name.
///
/// Given: a file whose name holds a right-to-left override, a zero-width space, a line
/// separator, and ordinary Japanese text.
/// When: the tree is dry run.
/// Then: the three invisible or reordering characters are escaped, and the Japanese text is
/// printed as it is.
///
/// Why it matters: a right-to-left override makes `invoice\u{202e}fdp.exe` display as
/// `invoiceexe.pdf`, so the operator would approve one name and the run would destroy another.
#[test]
fn invisible_and_reordering_characters_in_a_name_are_escaped() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path()
            .join("日本-invoice\u{202e}fdp\u{200b}\u{2028}.exe"),
        "payload",
    )
    .unwrap();

    // When:
    let outcome = run(&["--dry-run", text(root.path())]);

    // Then:
    assert_eq!(outcome.code, 0, "{}", outcome.stderr);
    assert!(
        outcome
            .stdout
            .contains("日本-invoice\\u{202e}fdp\\u{200b}\\u{2028}.exe"),
        "{}",
        outcome.stdout
    );
    for hidden in ['\u{202e}', '\u{200b}', '\u{2028}'] {
        assert!(
            !outcome.stdout.contains(hidden),
            "{hidden:?} reached the terminal"
        );
    }
}

/// A reader that stops early, as `head` does, gets no error message.
///
/// Given: a tree with more targets than a pipe buffer holds.
/// When: the dry run is read for one line and the pipe is closed.
/// Then: the run prints nothing on standard error and exits with 141, the code a shell reports
/// for a writer stopped by a closed pipe.
///
/// Why it matters: paging a long dry run through `head` or `less` is how an operator reads it,
/// and an error line there reads as a fault in the plan.
#[test]
fn a_closed_pipe_ends_the_dry_run_quietly() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    // Given:
    let root = tempfile::tempdir().unwrap();
    for n in 0..4000 {
        fs::write(
            root.path().join(format!("file-with-a-longer-name-{n}.txt")),
            "",
        )
        .unwrap();
    }

    // When:
    let mut child = Command::new(env!("CARGO_BIN_EXE_blank"))
        .args(["--dry-run", text(root.path())])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut first = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut first)
        .unwrap();
    let output = child.wait_with_output().unwrap();

    // Then:
    assert!(first.starts_with("Overwriting "), "{first}");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    assert_eq!(output.status.code(), Some(141));
}

/// Two different names never display the same.
///
/// Given: a file named with the text `\x07` and a file named with a real bell character.
/// When: the tree is dry run.
/// Then: the backslash of the first is doubled, so the two lines differ.
///
/// Why it matters: without escaping the backslash, a name can spell out the escape of another
/// name, and the operator cannot tell which of the two the run will destroy.
#[test]
fn a_backslash_in_a_name_is_escaped_too() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("bell\\x07.txt"), "").unwrap();
    fs::write(root.path().join("bell\x07.txt"), "").unwrap();

    // When:
    let outcome = run(&["--dry-run", text(root.path())]);

    // Then:
    assert_eq!(outcome.code, 0, "{}", outcome.stderr);
    let lines: Vec<&str> = outcome.stdout.lines().take(2).collect();
    assert_eq!(
        lines,
        [
            format!("Overwriting {}/bell\\x07.txt", text(root.path())),
            format!("Overwriting {}/bell\\\\x07.txt", text(root.path())),
        ]
    );
}
