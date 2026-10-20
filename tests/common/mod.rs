#![allow(dead_code)]

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

pub struct Outcome {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

/// Runs the built binary with nothing on standard input, which is what a command in a pipeline
/// sees.
pub fn run(args: &[&str]) -> Outcome {
    run_with(args, "", |_| {})
}

/// Runs the built binary with `input` on standard input. `adjust` sets the environment or the
/// working directory of the child, so no test changes the environment of the test process.
pub fn run_with(args: &[&str], input: &str, adjust: impl FnOnce(&mut Command)) -> Outcome {
    let mut command = Command::new(env!("CARGO_BIN_EXE_blank"));
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    adjust(&mut command);
    let mut child = command.spawn().expect("blank starts");
    child
        .stdin
        .take()
        .expect("standard input is piped")
        .write_all(input.as_bytes())
        .expect("standard input accepts the answer");
    let output = child.wait_with_output().expect("blank finishes");
    Outcome {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        code: output
            .status
            .code()
            .expect("blank exits rather than being killed"),
    }
}

pub fn text(path: &Path) -> &str {
    path.to_str().expect("test paths are UTF-8")
}
