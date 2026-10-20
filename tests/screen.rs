mod common;

use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use common::text;
use nix::pty::{Winsize, openpty};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

/// The sequence that leaves the alternate screen. Everything after it stays in the scrollback.
const LEAVE_ALTERNATE_SCREEN: &str = "\x1b[?1049l";

struct Shown {
    /// What the terminal received after the interface closed.
    after: String,
    code: i32,
}

/// How the test ends the screen once `waits_for` has been drawn.
enum Respond {
    PressQ,
    /// An interrupt from outside the terminal, which raw mode does not turn into a key.
    Interrupt,
}

/// Runs blank on a pseudo-terminal of 80 by 24, responds once `waits_for` has been drawn, and
/// returns what was printed after the alternate screen closed. A frame sends only the cells
/// that changed, so `waits_for` is a word the screen draws new, not a whole line.
fn show(args: &[&str], waits_for: &str, respond: Respond) -> Shown {
    let size = Winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let pty = openpty(Some(&size), None).unwrap();
    let terminal = |fd: &OwnedFd| Stdio::from(fd.try_clone().unwrap());
    let mut child = Command::new(env!("CARGO_BIN_EXE_blank"))
        .args(args)
        .stdin(terminal(&pty.slave))
        .stdout(terminal(&pty.slave))
        .stderr(terminal(&pty.slave))
        .spawn()
        .unwrap();
    drop(pty.slave);
    let mut writer = File::from(pty.master.try_clone().unwrap());
    let mut reader = File::from(pty.master);
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut chunk = [0; 4096];
        // The read fails once the child closes the last copy of the terminal.
        while let Ok(count) = reader.read(&mut chunk) {
            if count == 0 || sender.send(chunk[..count].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut seen = Vec::new();
    // Every frame hides the cursor again, so the screen never falls silent, and only a deadline
    // for the whole wait ends it.
    let deadline = Instant::now() + Duration::from_secs(20);
    while !String::from_utf8_lossy(&seen).contains(waits_for) {
        let left = deadline.saturating_duration_since(Instant::now());
        let chunk = receiver
            .recv_timeout(left)
            .unwrap_or_else(|_| panic!("{waits_for:?} never appeared"));
        seen.extend(chunk);
    }
    match respond {
        Respond::PressQ => writer.write_all(b"q").unwrap(),
        Respond::Interrupt => {
            let pid = Pid::from_raw(child.id() as i32);
            kill(pid, Signal::SIGINT).unwrap();
        }
    }
    let mut waited = 0;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        waited += 1;
        if waited > 100 {
            child.kill().unwrap();
            while let Ok(chunk) = receiver.recv_timeout(Duration::from_millis(200)) {
                seen.extend(chunk);
            }
            panic!(
                "blank did not exit after q:\n{:?}",
                String::from_utf8_lossy(&seen)
            );
        }
        thread::sleep(Duration::from_millis(50));
    };
    while let Ok(chunk) = receiver.recv_timeout(Duration::from_millis(500)) {
        seen.extend(chunk);
    }
    let seen = String::from_utf8_lossy(&seen).into_owned();
    let after = seen
        .rsplit_once(LEAVE_ALTERNATE_SCREEN)
        .unwrap_or_else(|| panic!("the interface never left the alternate screen:\n{seen:?}"))
        .1
        .to_owned();
    Shown {
        after,
        code: status.code().expect("blank exits rather than being killed"),
    }
}

/// A run on a terminal is drawn full screen, and its verdict is printed once the screen closes.
///
/// Given: a tree with one file.
/// When: it is destroyed on a terminal, and the operator presses q once the run is complete.
/// Then: the process exits zero, the tree is gone, and the text left after the full screen
/// closes says blanking complete.
///
/// Why it matters: the alternate screen is cleared on exit, so the printed verdict is the only
/// record of the run left in the scrollback, and it must say what the exit code says.
#[test]
fn a_complete_run_prints_its_verdict_after_the_screen_closes() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("secret.txt"), "payload").unwrap();

    // When:
    let shown = show(&["--yes", text(root.path())], "complete", Respond::PressQ);

    // Then:
    assert_eq!(shown.code, 0, "{}", shown.after);
    assert!(!root.path().exists());
    assert!(
        shown.after.contains("blanking complete"),
        "{:?}",
        shown.after
    );
}

/// A run that leaves a survivor prints a failed verdict, the survivor, and exits non-zero.
///
/// Given: a tree with one file, and a second argument that does not exist.
/// When: it is destroyed on a terminal, and the operator presses q once the run has failed.
/// Then: the process exits 1, and the text left after the full screen closes says blanking
/// failed with one survivor and names it.
///
/// Why it matters: the operator decides what to do next from the scrollback, so the verdict
/// and the survivor must both be there once the full screen is gone.
#[test]
fn a_failed_run_prints_its_verdict_and_its_survivor_after_the_screen_closes() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("secret.txt"), "payload").unwrap();
    let missing = root.path().with_extension("missing");

    // When:
    let shown = show(
        &["--yes", text(root.path()), text(Path::new(&missing))],
        "failed",
        Respond::PressQ,
    );

    // Then:
    assert_eq!(shown.code, 1, "{}", shown.after);
    assert!(
        shown.after.contains("blanking failed, 1 target survived"),
        "{:?}",
        shown.after
    );
    assert!(shown.after.contains(text(&missing)), "{:?}", shown.after);
}

/// q during a run stops it, and the verdict says the run was cancelled.
///
/// Given: a tree of files that takes many covers to destroy, on two workers.
/// When: the operator presses q as soon as the first file is drawn.
/// Then: the process exits 1, files survive, and the text left after the full screen closes
/// says blanking cancelled.
///
/// Why it matters: q is how an operator stops a run aimed at the wrong tree, and a screen that
/// only closes while the run goes on behind it destroys the tree it was asked to spare.
#[test]
fn q_during_a_run_stops_it_and_reports_it_cancelled() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let files: Vec<_> = (0..40)
        .map(|index| {
            let path = root.path().join(format!("file-{index:02}.bin"));
            fs::write(&path, vec![0xAB; 2 << 20]).unwrap();
            path
        })
        .collect();

    // When:
    let shown = show(
        &["--yes", "--jobs", "2", "--passes", "50", text(root.path())],
        "file-",
        Respond::PressQ,
    );

    // Then:
    assert_eq!(shown.code, 1, "{}", shown.after);
    let survivors = files.iter().filter(|path| path.exists()).count();
    assert!(survivors >= files.len() / 2, "{survivors} survived");
    assert!(
        shown.after.contains("blanking cancelled"),
        "{:?}",
        shown.after
    );
}

/// An interrupt from outside the terminal stops a run on the full screen and closes it.
///
/// Given: a tree of files that takes many covers to destroy, on two workers.
/// When: the process receives SIGINT as soon as the first file is drawn, and no key is pressed.
/// Then: the process exits 1 by itself, files survive, and the verdict says cancelled.
///
/// Why it matters: raw mode turns ctrl+c into a key, so a SIGINT can only come from outside,
/// such as a supervisor stopping the job, and a screen that waits for a key then never ends.
#[test]
fn an_interrupt_stops_a_run_on_the_full_screen_and_closes_it() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let files: Vec<_> = (0..40)
        .map(|index| {
            let path = root.path().join(format!("file-{index:02}.bin"));
            fs::write(&path, vec![0xAB; 2 << 20]).unwrap();
            path
        })
        .collect();

    // When:
    let shown = show(
        &["--yes", "--jobs", "2", "--passes", "50", text(root.path())],
        "file-",
        Respond::Interrupt,
    );

    // Then:
    assert_eq!(shown.code, 1, "{}", shown.after);
    let survivors = files.iter().filter(|path| path.exists()).count();
    assert!(survivors >= files.len() / 2, "{survivors} survived");
    assert!(
        shown.after.contains("blanking cancelled"),
        "{:?}",
        shown.after
    );
}
