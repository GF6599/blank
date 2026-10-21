use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use nix::pty::{Winsize, openpty};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

/// The sequence that leaves the alternate screen. Everything after it stays in the scrollback.
const LEAVE_ALTERNATE_SCREEN: &str = "\x1b[?1049l";
/// The device status request that ends the graphics query, and the answer every terminal gives.
/// A terminal that never answers leaves the query reading the first keys typed after it.
const STATUS_REQUEST: &[u8] = b"\x1b[5n";
const STATUS_ANSWER: &[u8] = b"\x1b[0n";

pub struct Shown {
    /// What the terminal received after the full screen closed, or all of it when no full
    /// screen opened.
    pub after: String,
    pub code: i32,
}

pub enum Respond {
    Keys(&'static str),
    /// An interrupt from outside the terminal, which raw mode does not turn into a key.
    Interrupt,
    /// Keys typed once the file exists, for a step that waits on a process the screen does not
    /// show.
    KeysOnceExists(PathBuf, &'static str),
}

pub fn show(args: &[&str], waits_for: &str, respond: Respond) -> Shown {
    let mut command = Command::new(env!("CARGO_BIN_EXE_blank"));
    command.args(args);
    drive(&mut command, [(waits_for, respond)])
}

/// Runs `command` on a pseudo-terminal of 80 by 24 and walks `steps` in order: each waits for
/// its text in what the terminal received since the step before, then responds. A frame sends
/// only the cells that changed, so a step waits for a word the screen draws new, not a whole
/// line.
pub fn drive<'a>(
    command: &mut Command,
    steps: impl IntoIterator<Item = (&'a str, Respond)>,
) -> Shown {
    let size = Winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let pty = openpty(Some(&size), None).unwrap();
    let terminal = |fd: &OwnedFd| Stdio::from(fd.try_clone().unwrap());
    let mut child = command
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
    let mut searched = 0;
    let mut answered = 0;
    for (waits_for, respond) in steps {
        // Every frame hides the cursor again, so the screen never falls silent, and only a
        // deadline for the whole wait ends it.
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let text = String::from_utf8_lossy(&seen[searched..]).into_owned();
            if let Some(found) = text.find(waits_for) {
                searched += found + waits_for.len();
                break;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            let Ok(chunk) = receiver.recv_timeout(left) else {
                panic!("{waits_for:?} never appeared:\n{text:?}");
            };
            seen.extend(chunk);
            let requests = seen
                .windows(STATUS_REQUEST.len())
                .filter(|window| *window == STATUS_REQUEST)
                .count();
            for _ in answered..requests {
                writer.write_all(STATUS_ANSWER).unwrap();
            }
            answered = requests;
        }
        match respond {
            Respond::Keys(keys) => writer.write_all(keys.as_bytes()).unwrap(),
            Respond::Interrupt => kill(Pid::from_raw(child.id() as i32), Signal::SIGINT).unwrap(),
            Respond::KeysOnceExists(file, keys) => {
                let deadline = Instant::now() + Duration::from_secs(20);
                while !file.exists() {
                    assert!(Instant::now() < deadline, "{file:?} never appeared");
                    thread::sleep(Duration::from_millis(10));
                }
                writer.write_all(keys.as_bytes()).unwrap();
            }
        }
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            while let Ok(chunk) = receiver.recv_timeout(Duration::from_millis(200)) {
                seen.extend(chunk);
            }
            panic!("blank did not exit:\n{:?}", String::from_utf8_lossy(&seen));
        }
        thread::sleep(Duration::from_millis(50));
    };
    while let Ok(chunk) = receiver.recv_timeout(Duration::from_millis(500)) {
        seen.extend(chunk);
    }
    let seen = String::from_utf8_lossy(&seen).into_owned();
    let after = match seen.rsplit_once(LEAVE_ALTERNATE_SCREEN) {
        Some((_, after)) => after.to_owned(),
        None => seen,
    };
    Shown {
        after,
        code: status.code().expect("blank exits rather than being killed"),
    }
}
