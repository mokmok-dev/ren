use clap::{Parser, Subcommand};
use std::process::ExitCode;

#[derive(Debug, Parser)]
#[clap(
    name = env!("CARGO_PKG_NAME"),
    version = env!("CARGO_PKG_VERSION"),
    arg_required_else_help = true,
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Workflow(ren_workflow::Config),
    /// Installs the embedded skill into coding agents (runs every group's init).
    Init(ren_workflow::InitArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match cli.command {
        Command::Workflow(config) => ren_workflow::run(config),
        // The top-level `init` recursively runs the init of every command
        // group; only the workflow group defines one today.
        Command::Init(args) => ren_workflow::run_init(&args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        },
    }
}
