use clap::Parser;

/// Destroy files and directories beyond recovery.
#[derive(Parser)]
#[command(name = "blank")]
struct Cli {}

fn main() {
    Cli::parse();
}
