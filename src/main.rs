use std::error::Error;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use blank::guard::Guard;
use blank::plan::Plan;
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
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
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
    Err("this build cannot destroy anything yet; pass --dry-run to see the plan".into())
}
