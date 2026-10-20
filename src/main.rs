use std::error::Error;
use std::ffi::c_int;
use std::io::{self, Write};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

use blank::engine::{DEFAULT_JOBS, DEFAULT_PASSES, Options, Stage, Update, destroy};
use blank::guard::Guard;
use blank::manifest::Manifest;
use blank::plan::Plan;
use blank::question::confirm;
use blank::terminal::escape_path;
use clap::{CommandFactory, Parser};
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};

const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (commit: ",
    env!("BLANK_COMMIT"),
    ", built: ",
    env!("BLANK_COMMITTED"),
    ")"
);

/// Destroy files and directories beyond recovery.
#[derive(Parser)]
#[command(name = "blank", version = VERSION)]
struct Cli {
    /// Files and directories to destroy
    paths: Vec<PathBuf>,

    /// List what would be destroyed and destroy nothing
    #[arg(long)]
    dry_run: bool,

    /// Destroy without asking first
    #[arg(short, long)]
    yes: bool,

    /// Random covers over each file before the final zero cover
    #[arg(long, default_value_t = DEFAULT_PASSES)]
    passes: u32,

    /// Files to overwrite at the same time. Raise it on a faster disk
    #[arg(long, default_value_t = DEFAULT_JOBS)]
    jobs: NonZeroUsize,

    /// Append the state of every target to this file. It names every file destroyed, so it is
    /// a manifest of what was on the disk
    #[arg(long)]
    log: Option<PathBuf>,
}

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn request_stop(_: c_int) {
    STOP.store(true, Ordering::Relaxed);
}

/// The code a shell reports for a writer that a closed pipe stopped.
const CLOSED_PIPE: u8 = 141;

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        // A reader such as `head` closed the pipe once it had what it asked for, so there is
        // nothing to report. Rust ignores SIGPIPE, which would otherwise end the process here.
        Err(error) if closed_pipe(error.as_ref()) => ExitCode::from(CLOSED_PIPE),
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn closed_pipe(error: &(dyn Error + 'static)) -> bool {
    error
        .downcast_ref::<io::Error>()
        .is_some_and(|error| error.kind() == io::ErrorKind::BrokenPipe)
}

fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    if cli.paths.is_empty() {
        Cli::command().print_help()?;
        return Ok(());
    }
    let plan = Plan::enumerate(&cli.paths, &Guard::for_host())?;
    if cli.dry_run {
        plan.write_dry_run(&mut io::stdout().lock())?;
        // A plan built over a directory nobody could read is missing whatever is inside it, so
        // reporting it as clean would understate what the run would destroy.
        return match plan.incomplete() {
            Some(incomplete) => Err(incomplete.to_string().into()),
            None => Ok(()),
        };
    }
    if let Some(log) = &cli.log
        && plan.contains(log)
    {
        return Err(format!(
            "the log path {} is inside the selected targets. Choose a path the run will not destroy",
            escape_path(log)
        )
        .into());
    }
    // Nothing to destroy is not worth a question.
    if !cli.yes && plan.total() > 0 {
        let confirmed = confirm(&mut io::stdin().lock(), &mut io::stdout().lock(), &plan)?;
        if !confirmed {
            // An error rather than a clean exit, because exit zero is what this command says
            // when everything was destroyed.
            return Err("cancelled, nothing was destroyed".into());
        }
    }
    // Opened only now, so a dry run and a refused question change nothing on the disk.
    let mut manifest = match &cli.log {
        Some(log) => Some(
            Manifest::open(log)
                .map_err(|error| format!("open the log {}: {error}", escape_path(log)))?,
        ),
        None => None,
    };
    let interrupt = SigAction::new(
        SigHandler::Handler(request_stop),
        // A write that the signal interrupts starts again rather than failing the cover.
        SaFlags::SA_RESTART,
        SigSet::empty(),
    );
    // SAFETY: the handler only stores to an atomic, which is safe inside a signal handler.
    unsafe { sigaction(Signal::SIGINT, &interrupt) }?;
    let options = Options {
        jobs: cli.jobs,
        passes: cli.passes,
    };
    let mut out = io::stdout().lock();
    let mut percent = None;
    let report = destroy(&plan, options, &STOP, &mut |update| match update {
        // A closed standard output must not stop a run the operator confirmed halfway through a
        // tree. The report on standard error, the manifest and the exit code are the record.
        Update::Target { verb, path, stage } => {
            if stage == Stage::Start {
                let _ = writeln!(out, "Activity: {verb} {}", escape_path(path));
            }
            if let (Some(manifest), Stage::Done) = (&mut manifest, stage) {
                manifest.destroyed(path);
            }
        }
        // Truncated rather than rounded, so 100% is printed only when the run is complete.
        Update::Progress { done, total } => {
            let now = u128::from(done) * 100 / u128::from(total);
            if percent != Some(now) {
                percent = Some(now);
                let _ = writeln!(out, "Progress: {now}%");
            }
        }
    });
    let logged = manifest.map_or(Ok(()), |mut manifest| {
        if let Err(incomplete) = &report {
            manifest.survivors(incomplete);
        }
        manifest.finish()
    });
    // A failed log must not hide the survivors, so the report names both.
    let errors: Vec<String> = [
        report.err().map(|incomplete| incomplete.to_string()),
        logged.err().map(|error| format!("write the log: {error}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n").into())
    }
}
