mod common;

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use blank::engine::COVER_CHUNK;
use common::text;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

/// An interrupt stops the run and names each target it left, in the state it left it.
///
/// Given: a tree of files, each several writes long and holding one known byte, destroyed with
/// many covers on two workers.
/// When: the process is interrupted as soon as it reports its first target.
/// Then: it exits non-zero and reports targets not destroyed, files survive, and the manifest
/// names each surviving file as intact when it holds every original byte and as damaged when
/// it does not.
///
/// Why it matters: Ctrl-C is how an operator stops a run aimed at the wrong tree, and the
/// report is what tells them which files they still have.
#[test]
fn an_interrupt_stops_the_run_and_names_what_it_left() {
    // Given:
    let root = tempfile::tempdir().unwrap();
    let files: Vec<PathBuf> = (0..40)
        .map(|index| {
            let path = root.path().join(format!("file-{index:02}.bin"));
            fs::write(&path, vec![0xAB; COVER_CHUNK * 2]).unwrap();
            path
        })
        .collect();
    let elsewhere = tempfile::tempdir().unwrap();
    let manifest = elsewhere.path().join("manifest.log");
    let mut child = Command::new(env!("CARGO_BIN_EXE_blank"))
        .args(["--yes", "--jobs", "2", "--passes", "50", "--log"])
        .args([text(&manifest), text(root.path())])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    // When:
    let mut line = String::new();
    while !line.starts_with("Activity:") {
        line.clear();
        assert_ne!(
            stdout.read_line(&mut line).unwrap(),
            0,
            "the run printed no activity"
        );
    }
    kill(Pid::from_raw(child.id() as i32), Signal::SIGINT).unwrap();
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    std::io::copy(&mut stdout, &mut std::io::sink()).unwrap();
    let status = child.wait().unwrap();

    // Then:
    assert_eq!(status.code(), Some(1), "{stderr}");
    let survivors: Vec<&PathBuf> = files.iter().filter(|path| path.exists()).collect();
    assert!(
        survivors.len() >= files.len() / 2,
        "{} survived",
        survivors.len()
    );
    let logged = fs::read_to_string(&manifest).unwrap();
    for path in survivors {
        let untouched = fs::read(path).unwrap().iter().all(|&byte| byte == 0xAB);
        let state = if untouched { "intact" } else { "damaged" };
        let entry = format!("{state}\t{}\t", text(path));
        assert!(
            logged.lines().any(|line| line.starts_with(&entry)),
            "{} is {state} and the manifest does not say so:\n{logged}",
            text(path)
        );
    }
    assert!(stderr.contains("not destroyed"), "{stderr}");
}
