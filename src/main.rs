use clap::Parser;
use recall_engine::cli::Cli;
use recall_engine::commands;
use recall_engine::Result;
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("recall_engine=info".parse().unwrap()),
        )
        .init();

    commands::run(Cli::parse())
}
