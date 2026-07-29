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
    /// Captures, indexes, and queries local Markdown knowledge.
    Memory(ren_memory::Config),
    /// Installs the embedded skill into coding agents (runs every group's init).
    Init(ren_workflow::InitArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match cli.command {
        Command::Workflow(config) => {
            ren_workflow::run(config).map_err(|error| CommandFailure::Workflow(error.to_string()))
        },
        Command::Memory(config) => ren_memory::run(config).map_err(CommandFailure::Memory),
        // The top-level `init` recursively runs the init of every command
        // group; only the workflow group defines one today.
        Command::Init(args) => ren_workflow::run_init(&args)
            .map_err(|error| CommandFailure::Workflow(error.to_string())),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(CommandFailure::Memory(error)) => {
            eprintln!(
                "{}",
                serde_json::json!({
                    "error": {
                        "class": error.class(),
                        "message": error.to_string()
                    }
                })
            );
            ExitCode::FAILURE
        },
        Err(CommandFailure::Workflow(error)) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        },
    }
}

enum CommandFailure {
    Workflow(String),
    Memory(ren_memory::MemoryError),
}
