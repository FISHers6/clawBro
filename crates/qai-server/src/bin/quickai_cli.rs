use anyhow::Result;
use clap::Parser;
use qai_server::cli::args::{Cli, Commands};

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
        Commands::Setup(args)       => qai_server::cli::setup::run(args).await,
        Commands::Auth(args)        => qai_server::cli::auth::run(args).await,
        Commands::Config(args)      => qai_server::cli::config_cmd::run(args).await,
        Commands::Serve(args)       => qai_server::cli::serve::run(args).await,
        Commands::Doctor            => qai_server::cli::doctor::run().await,
        Commands::Status            => qai_server::cli::status::run().await,
        Commands::Completions(args) => qai_server::cli::completions::run(args),
    }
}
