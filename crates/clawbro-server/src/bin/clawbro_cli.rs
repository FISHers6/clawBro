use anyhow::Result;
use clap::Parser;
use clawbro::cli::args::{Cli, Commands};

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Setup(args) => clawbro::cli::setup::run(args).await,
        Commands::Auth(args) => clawbro::cli::auth::run(args).await,
        Commands::Config(args) => clawbro::cli::config_cmd::run(args).await,
        Commands::Serve(args) => clawbro::cli::serve::run(args).await,
        Commands::TeamHelper(args) => clawbro::cli::team_helper::run(args).await,
        Commands::RuntimeBridge => clawbro::cli::internal_agent::run_runtime_bridge().await,
        Commands::AcpAgent => clawbro::cli::internal_agent::run_acp_agent().await,
        Commands::Doctor => clawbro::cli::doctor::run().await,
        Commands::Status => clawbro::cli::status::run().await,
        Commands::Schedule(args) => clawbro::cli::schedule::run(args).await,
        Commands::Skill(args) => clawbro::cli::skills::run(args).await,
        Commands::Completions(args) => clawbro::cli::completions::run(args),
    }
}
