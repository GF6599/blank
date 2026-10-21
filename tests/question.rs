mod common;

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use blank::engine::{DEFAULT_JOBS, DEFAULT_PASSES, Options};
use blank::guard::Guard;
use blank::plan::Plan;
use blank::question::confirm;
use common::{run, run_with, text};

fn plan_of(args: &[&Path]) -> Plan {
    let args: Vec<PathBuf> = args.iter().map(|arg| arg.to_path_buf()).collect();
    Plan::enumerate(&args, &Guard::for_host()).expect("no case here names a protected path")
}

fn ask(plan: &Plan, answer: &str) -> (bool, String) {
    let mut shown = Vec::new();
    let options = Options {
        jobs: DEFAULT_JOBS,
        passes: DEFAULT_PASSES,
        allow_hard_links: false,
    };
    let go_on =
        confirm(&mut answer.as_bytes(), &mut shown, plan, &options).expect("the question is asked");
    (go_on, String::from_utf8(shown).unwrap())
}

/// Only a whole yes lets a run go on.
///
/// Given: a plan with one file in it.
/// When: the question is answered with each of a set of typed answers, including none at all.
/// Then: yes in any case, with or without padding, goes on, and every other answer stops.
///
/// Why it matters: the run destroys beyond recovery, so a stray keystroke must not reach it,
/// and a closed input must read as a refusal or a command in a pipeline destroys a tree nobody
/// confirmed.
#[test]
fn only_a_whole_yes_goes_on() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("victim.txt"), "payload").unwrap();
    let plan = plan_of(&[root.path()]);

    for (answer, expected) in [
        ("yes\n", true),
        ("YES\n", true),
        ("  yes  \n", true),
        ("y\n", false),
        ("no\n", false),
        ("\n", false),
        ("", false),
        ("yesno\n", false),
    ] {
        // When:
        let (go_on, _) = ask(&plan, answer);

        // Then:
        assert_eq!(go_on, expected, "the answer {answer:?}");
    }
}

/// The question counts what is about to go and names where it lives.
///
/// Given: a tree holding two files, one symlink and two directories.
/// When: the question is asked.
/// Then: it counts each kind, says what the files hold and what the covers will write, and
/// names the selected root.
///
/// Why it matters: a bare "are you sure" is answered yes by reflex. The counts and the root are
/// what make a command aimed at the wrong directory visible in time.
#[test]
fn the_question_counts_each_kind_and_names_the_root() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let nested = root.path().join("nested");
    fs::create_dir(&nested).unwrap();
    fs::write(root.path().join("one.txt"), "payload").unwrap();
    fs::write(nested.join("two.txt"), "payload").unwrap();
    symlink(root.path().join("one.txt"), root.path().join("link")).unwrap();

    // When:
    let (_, shown) = ask(&plan_of(&[root.path()]), "no\n");

    // Then:
    assert_eq!(
        shown,
        format!(
            "blank will destroy 2 files, 1 symlink or special file, and 2 directories.\n\
             The files hold 14 bytes, and the covers will write 154 bytes in 11 passes.\n\
             Selected roots:\n  {}\n\
             This cannot be undone. Type yes to continue: \n",
            text(root.path())
        )
    );
}

/// The question names five roots and counts the rest.
///
/// Given: six files named as six arguments.
/// When: the question is asked.
/// Then: it names the first five and says how many more there are.
///
/// Why it matters: a glob can expand to thousands of arguments, and a question that lists them
/// all scrolls its own counts off the screen.
#[test]
fn the_question_names_five_roots_and_counts_the_rest() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let files: Vec<PathBuf> = (1..=6)
        .map(|n| root.path().join(format!("{n}.txt")))
        .collect();
    for file in &files {
        fs::write(file, "payload").unwrap();
    }
    let args: Vec<&Path> = files.iter().map(PathBuf::as_path).collect();

    // When:
    let (_, shown) = ask(&plan_of(&args), "no\n");

    // Then:
    assert!(
        shown.contains(&format!("  {}\n  ... and 1 more\n", text(&files[4]))),
        "{shown}"
    );
    assert!(!shown.contains(text(&files[5])), "{shown}");
}

/// Only the answer decides whether anything is destroyed.
///
/// Given: a tree with a file in it.
/// When: the run is answered with a no, with nothing on standard input, and with a yes.
/// Then: the refusals exit non-zero, say nothing was destroyed, and leave the file, and the yes
/// destroys the tree and exits zero.
///
/// Why it matters: exit zero is what this command says when everything was destroyed, so a
/// refusal that exits zero reads to a script as a completed run, and a refusal that destroys
/// anything is the one failure the question exists to prevent.
#[test]
fn only_the_answer_decides_whether_anything_is_destroyed() {
    for answer in ["no\n", ""] {
        // Given:
        let root = tempfile::tempdir().unwrap();
        let victim = root.path().join("victim.txt");
        fs::write(&victim, "payload").unwrap();

        // When:
        let outcome = run_with(&[text(root.path())], answer, |_| {});

        // Then:
        assert_ne!(outcome.code, 0, "{answer:?}");
        assert!(
            outcome.stdout.contains("Type yes to continue"),
            "{}",
            outcome.stdout
        );
        assert!(
            outcome.stderr.contains("cancelled, nothing was destroyed"),
            "{answer:?}: {}",
            outcome.stderr
        );
        assert_eq!(
            fs::read_to_string(&victim).unwrap(),
            "payload",
            "{answer:?}"
        );
    }

    // Given:
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("victim.txt"), "payload").unwrap();

    // When:
    let outcome = run_with(&[text(root.path())], "yes\n", |_| {});

    // Then:
    assert_eq!(outcome.code, 0, "{}", outcome.stderr);
    assert!(root.path().symlink_metadata().is_err());
}

/// A scripted run skips the question rather than hanging on it.
///
/// Given: a tree with a file in it and nothing on standard input.
/// When: the run is given --yes, and again -y.
/// Then: no question is asked, the run exits zero, and the tree is gone.
///
/// Why it matters: without a way past the question the command cannot run from a script, and
/// that is the pressure that gets safety checks removed.
#[test]
fn yes_skips_the_question() {
    for flag in ["--yes", "-y"] {
        // Given:
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("victim.txt"), "payload").unwrap();

        // When:
        let outcome = run(&[flag, text(root.path())]);

        // Then:
        assert!(
            !outcome.stdout.contains("Type yes"),
            "{flag}: {}",
            outcome.stdout
        );
        assert_eq!(outcome.code, 0, "{flag}: {}", outcome.stderr);
        assert!(root.path().symlink_metadata().is_err(), "{flag}");
    }
}
