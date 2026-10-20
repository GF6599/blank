use std::error::Error;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use blank::guard::Guard;
use blank::plan::Plan;
use blank::question::confirm;
use clap::{CommandFactory, Parser};

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
    // Nothing to destroy is not worth a question.
    if !cli.yes && plan.total() > 0 {
        let confirmed = confirm(&mut io::stdin().lock(), &mut io::stdout().lock(), &plan)?;
        if !confirmed {
            // An error rather than a clean exit, because exit zero is what this command says
            // when everything was destroyed.
            return Err("cancelled, nothing was destroyed".into());
        }
    }
    Err("this build cannot destroy anything yet; pass --dry-run to see the plan".into())
}
