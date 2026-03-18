pub mod auth_cfg;
pub mod channel;
pub mod mode;
pub mod provider;
pub mod writer;

use crate::cli::{
    args::SetupArgs,
    i18n::{Language, Messages},
};
use anyhow::Result;
use console::style;
use dialoguer::{theme::ColorfulTheme, Confirm, Select};

pub async fn run(args: SetupArgs) -> Result<()> {
    let theme = ColorfulTheme::default();

    // Step 1: Language selection
    let lang = if let Some(l) = &args.lang {
        Language::from_arg(Some(l))
    } else {
        let choices = ["中文", "English", "日本語", "한국어"];
        let idx = Select::with_theme(&theme)
            .with_prompt("请选择语言 / Select Language")
            .items(&choices)
            .default(0)
            .interact()?;
        match idx {
            1 => Language::En,
            2 => Language::Ja,
            3 => Language::Ko,
            _ => Language::Zh,
        }
    };
    let m = Messages::for_lang(lang);
    println!("\n{}\n", style(m.welcome).bold().cyan());

    // Step 2: Check existing config
    let config_path = writer::config_path();
    if config_path.exists() && !args.reinit {
        let overwrite = if args.non_interactive {
            true
        } else {
            Confirm::with_theme(&theme)
                .with_prompt(m.confirm_write)
                .default(true)
                .interact()?
        };
        if !overwrite {
            println!("Cancelled. To reconfigure: clawbro setup --reinit");
            return Ok(());
        }
    }

    // Step 3: Provider + API key
    let provider_cfg = provider::collect(&args, lang)?;

    // Step 4: Mode + port + workspace
    let mut mode_cfg = mode::collect(&args, lang)?;

    // Step 5: Auth (ws_token)
    let auth_cfg = auth_cfg::collect(&args, lang)?;

    // Step 6: Channel (optional)
    let channel_cfg = if args.non_interactive {
        channel::ChannelConfig::None
    } else {
        channel::collect(lang)?
    };

    // Step 7: Team scope details (requires knowing the channel)
    mode::collect_team_scope_details(&args, &mut mode_cfg, &channel_cfg, lang)?;

    // Step 8: Create runtime directories
    let qdir = dirs::home_dir().unwrap_or_default().join(".clawbro");
    for sub in ["sessions", "shared", "skills", "personas"] {
        std::fs::create_dir_all(qdir.join(sub))?;
    }

    // Step 9: Write files
    let inputs = writer::WriteInputs {
        provider: &provider_cfg,
        mode: &mode_cfg,
        auth: &auth_cfg,
        channel: &channel_cfg,
    };

    let backup = writer::write_config(&inputs)?;
    if let Some(bak) = &backup {
        println!("{} ({})", style(m.backed_up).dim(), bak.display());
    }
    println!("{}", style(m.written_config).green());

    writer::write_env(&provider_cfg, &channel_cfg)?;
    if !provider_cfg.api_key.is_empty() {
        println!("{}", style(m.written_env).green());
    }

    // Step 10: Done
    println!("\n{}", style(m.done).bold().green());
    println!("\n{}", m.next_steps);
    Ok(())
}
