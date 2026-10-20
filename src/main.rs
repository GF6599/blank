use std::error::Error;
use std::ffi::c_int;
use std::io::{self, IsTerminal, Write};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use blank::engine::{DEFAULT_JOBS, DEFAULT_PASSES, Options, Stage, Update, destroy};
use blank::guard::Guard;
use blank::manifest::Manifest;
use blank::plan::{Incomplete, Plan};
use blank::question::confirm;
use blank::screen;
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

    /// Turn off the full screen and print one line per target to stdout. A run whose input or
    /// output is not a terminal is headless anyway
    #[arg(long)]
    no_ui: bool,
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
    // A pipe or a file on either end means a script, which reads lines and cannot answer keys.
    let headless = cli.no_ui || !io::stdin().is_terminal() || !io::stdout().is_terminal();
    let guard = Arc::new(Guard::for_host());
    let paths = if !cli.paths.is_empty() {
        cli.paths.clone()
    } else if headless {
        Cli::command().print_help()?;
        return Ok(());
    } else {
        let picked = screen::pick(&selector_start(&guard)?, Arc::clone(&guard))?;
        if picked.is_empty() {
            return Err("cancelled, nothing was destroyed".into());
        }
        picked
    };
    let plan = Plan::enumerate(&paths, &guard)?;
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
    let mut record = |update: &Update<'_>| {
        if let (Some(manifest), Update::Target { path, stage, .. }) = (&mut manifest, update)
            && *stage == Stage::Done
        {
            manifest.destroyed(path);
        }
    };
    let (report, interface) = if headless {
        (run_headless(&plan, options, &mut record), Ok(()))
    } else {
        let ended = screen::show(&plan, options, &STOP, &mut record)
            .map_err(|error| format!("open the interface: {error}. Nothing was destroyed"))?;
        // The full screen is gone, so this line is the record of the run in the scrollback.
        let _ = writeln!(io::stdout(), "{}", ended.outcome.verdict());
        (ended.report, ended.interface)
    };
    let logged = manifest.map_or(Ok(()), |mut manifest| {
        if let Err(incomplete) = &report {
            manifest.survivors(incomplete);
        }
        manifest.finish()
    });
    // A failed log must not hide the survivors, so the report names both.
    let errors: Vec<String> = [
        interface
            .err()
            .map(|error| format!("the interface failed, so the run was stopped: {error}")),
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

fn run_headless(
    plan: &Plan,
    options: Options,
    record: &mut dyn FnMut(&Update<'_>),
) -> Result<(), Incomplete> {
    let mut out = io::stdout().lock();
    let mut percent = None;
    destroy(plan, options, &STOP, &mut |update| {
        record(&update);
        match update {
            // A closed standard output must not stop a run the operator confirmed halfway
            // through a tree. The report on standard error, the manifest and the exit code are
            // the record.
            Update::Target { verb, path, stage } => {
                if stage == Stage::Start {
                    let _ = writeln!(out, "Activity: {verb} {}", escape_path(path));
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
        }
    })
}

/// The directory the selector opens on: the working directory, with every symlink on the way
/// resolved, because the selector lists what a link points at. Marking costs one keystroke and
/// the command line costs a typed path, so a directory the guard protects is refused here too.
fn selector_start(guard: &Guard) -> Result<PathBuf, Box<dyn Error>> {
    let start = std::env::current_dir()?.canonicalize()?;
    guard.check(&start).map_err(|refusal| {
        format!(
            "{refusal}, so it will not open a selector there either. Name a path on the command \
             line, or change to a narrower directory first"
        )
    })?;
    Ok(start)
}
