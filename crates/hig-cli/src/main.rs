#![recursion_limit = "256"]

mod benchmark;
mod cli;
mod commands;
mod output;
mod runtime;

fn main() -> anyhow::Result<()> {
    runtime::enforce_mcp_argv_paths()?;
    cli::run()
}
