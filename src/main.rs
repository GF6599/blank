use clap::Parser;

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
struct Cli {}

fn main() {
    Cli::parse();
}
