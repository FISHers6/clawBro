use crate::cli::args::{Cli, CompletionsArgs, ShellArg};
use anyhow::Result;
use clap::CommandFactory;
use clap_complete::{generate, shells};

pub fn run(args: CompletionsArgs) -> Result<()> {
    let mut cmd = Cli::command();
    let name = "clawbro";
    match args.shell {
        ShellArg::Bash => generate(shells::Bash, &mut cmd, name, &mut std::io::stdout()),
        ShellArg::Zsh => generate(shells::Zsh, &mut cmd, name, &mut std::io::stdout()),
        ShellArg::Fish => generate(shells::Fish, &mut cmd, name, &mut std::io::stdout()),
        ShellArg::PowerShell => {
            generate(shells::PowerShell, &mut cmd, name, &mut std::io::stdout())
        }
    }
    Ok(())
}
