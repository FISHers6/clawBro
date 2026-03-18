use crate::cli::{
    args::{ProviderArg, SetupArgs},
    i18n::{Language, Messages},
};
use anyhow::Result;
use dialoguer::{theme::ColorfulTheme, Input, Password, Select};

#[derive(Debug, Clone, PartialEq)]
pub enum ProviderKind {
    Anthropic,
    OpenAI,
    DeepSeek,
    Azure,
    Ollama,
    Custom,
}

impl ProviderKind {
    pub fn display_name(&self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "Anthropic (Claude)",
            ProviderKind::OpenAI => "OpenAI (GPT)",
            ProviderKind::DeepSeek => "DeepSeek",
            ProviderKind::Azure => "Azure OpenAI",
            ProviderKind::Ollama => "Ollama (local model)",
            ProviderKind::Custom => "Other OpenAI-compatible endpoint",
        }
    }

    pub fn env_var(&self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
            ProviderKind::Ollama => "",
            _ => "OPENAI_API_KEY",
        }
    }

    pub fn protocol_tag(&self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "anthropic_compatible",
            _ => "openai_compatible",
        }
    }

    pub fn default_base_url(&self) -> Option<&'static str> {
        match self {
            ProviderKind::Anthropic => Some("https://api.anthropic.com"),
            ProviderKind::OpenAI => Some("https://api.openai.com"),
            ProviderKind::DeepSeek => Some("https://api.deepseek.com"),
            ProviderKind::Ollama => Some("http://localhost:11434"),
            _ => None,
        }
    }

    pub fn default_model(&self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "claude-sonnet-4-6",
            ProviderKind::OpenAI => "gpt-4o",
            ProviderKind::DeepSeek => "deepseek-chat",
            ProviderKind::Azure => "gpt-4o",
            ProviderKind::Ollama => "llama3",
            ProviderKind::Custom => "gpt-4o",
        }
    }

    pub fn needs_api_key(&self) -> bool {
        !matches!(self, ProviderKind::Ollama)
    }

    pub fn slug(&self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::OpenAI => "openai",
            ProviderKind::DeepSeek => "deepseek",
            ProviderKind::Azure => "azure",
            ProviderKind::Ollama => "ollama",
            ProviderKind::Custom => "custom",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub profile_id: String,
}

impl ProviderConfig {
    pub fn env_var(&self) -> &str {
        self.kind.env_var()
    }
}

pub fn collect(args: &SetupArgs, lang: Language) -> Result<ProviderConfig> {
    let m = Messages::for_lang(lang);
    let theme = ColorfulTheme::default();

    let kind = if let Some(p) = &args.provider {
        match p {
            ProviderArg::Anthropic => ProviderKind::Anthropic,
            ProviderArg::Openai => ProviderKind::OpenAI,
            ProviderArg::Deepseek => ProviderKind::DeepSeek,
            ProviderArg::Azure => ProviderKind::Azure,
            ProviderArg::Ollama => ProviderKind::Ollama,
            ProviderArg::Custom => ProviderKind::Custom,
        }
    } else {
        let choices = [
            ProviderKind::Anthropic,
            ProviderKind::OpenAI,
            ProviderKind::DeepSeek,
            ProviderKind::Azure,
            ProviderKind::Ollama,
            ProviderKind::Custom,
        ];
        let names: Vec<&str> = choices.iter().map(|c| c.display_name()).collect();
        let idx = Select::with_theme(&theme)
            .with_prompt(m.select_provider)
            .items(&names)
            .default(0)
            .interact()?;
        choices[idx].clone()
    };

    let api_key = if !kind.needs_api_key() {
        String::new()
    } else if let Some(k) = &args.api_key {
        k.clone()
    } else {
        println!("  {}", m.enter_api_key_hint);
        Password::with_theme(&theme)
            .with_prompt(m.enter_api_key)
            .interact()?
    };

    let base_url = if let Some(b) = &args.api_base {
        b.trim().to_string()
    } else if matches!(kind, ProviderKind::Azure | ProviderKind::Custom) {
        let default = kind.default_base_url().unwrap_or("").to_string();
        let entered: String = Input::with_theme(&theme)
            .with_prompt(m.enter_api_base)
            .default(default)
            .allow_empty(false)
            .interact_text()?;
        entered.trim().to_string()
    } else {
        let default = kind.default_base_url().unwrap_or("").to_string();
        if !args.non_interactive {
            let entered: String = Input::with_theme(&theme)
                .with_prompt(m.enter_api_base)
                .default(default.clone())
                .allow_empty(true)
                .interact_text()?;
            if entered.trim().is_empty() {
                default
            } else {
                entered.trim().to_string()
            }
        } else {
            default
        }
    };

    let model = if let Some(mo) = &args.model {
        mo.clone()
    } else {
        let default_m = kind.default_model().to_string();
        if !args.non_interactive {
            let entered: String = Input::with_theme(&theme)
                .with_prompt(m.enter_model)
                .default(default_m.clone())
                .allow_empty(true)
                .interact_text()?;
            if entered.trim().is_empty() {
                default_m
            } else {
                entered.trim().to_string()
            }
        } else {
            default_m
        }
    };

    let profile_id = format!("{}-main", kind.slug());

    Ok(ProviderConfig {
        kind,
        api_key,
        base_url,
        model,
        profile_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_defaults() {
        let k = ProviderKind::Anthropic;
        assert_eq!(k.env_var(), "ANTHROPIC_API_KEY");
        assert_eq!(k.default_model(), "claude-sonnet-4-6");
        assert_eq!(k.protocol_tag(), "anthropic_compatible");
        assert!(k.needs_api_key());
    }

    #[test]
    fn deepseek_base_url() {
        assert_eq!(
            ProviderKind::DeepSeek.default_base_url(),
            Some("https://api.deepseek.com")
        );
    }

    #[test]
    fn ollama_no_key() {
        assert!(!ProviderKind::Ollama.needs_api_key());
        assert_eq!(
            ProviderKind::Ollama.default_base_url(),
            Some("http://localhost:11434")
        );
    }

    #[test]
    fn profile_id_format() {
        let k = ProviderKind::DeepSeek;
        assert_eq!(format!("{}-main", k.slug()), "deepseek-main");
    }

    #[test]
    fn azure_has_no_default_base_url() {
        assert_eq!(ProviderKind::Azure.default_base_url(), None);
    }
}
