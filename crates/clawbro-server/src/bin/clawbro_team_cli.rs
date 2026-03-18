use anyhow::Result;
use clap::Parser;
use clawbro::cli::args::{Cli, Commands};
use std::env;

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("clawbro-team-cli: {err:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse_from(
        std::iter::once("clawbro".to_string())
            .chain(std::iter::once("team-helper".to_string()))
            .chain(env::args().skip(1)),
    );
    match cli.command {
        Commands::TeamHelper(args) => clawbro::cli::team_helper::run(args).await,
        _ => unreachable!("team helper bin must parse into TeamHelper command"),
    }
}
