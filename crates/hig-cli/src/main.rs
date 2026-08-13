#![recursion_limit = "256"]

mod benchmark;
mod cli;
mod commands;
mod output;
mod runtime;

fn main() -> anyhow::Result<()> {
    cli::run()
}
