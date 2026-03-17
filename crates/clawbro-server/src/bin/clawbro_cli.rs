use anyhow::Result;
use clap::Parser;
use clawbro_server::cli::args::{Cli, Commands};

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
        Commands::Setup(args)       => clawbro_server::cli::setup::run(args).await,
        Commands::Auth(args)        => clawbro_server::cli::auth::run(args).await,
        Commands::Config(args)      => clawbro_server::cli::config_cmd::run(args).await,
        Commands::Serve(args)       => clawbro_server::cli::serve::run(args).await,
        Commands::Doctor            => clawbro_server::cli::doctor::run().await,
        Commands::Status            => clawbro_server::cli::status::run().await,
        Commands::Completions(args) => clawbro_server::cli::completions::run(args),
    }
}
