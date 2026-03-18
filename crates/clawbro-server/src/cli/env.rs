use std::path::PathBuf;

pub fn load_user_dot_env() {
    let path = env_path();
    let Ok(content) = std::fs::read_to_string(&path) else {
        sanitize_tracing_env();
        return;
    };

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        if let Some((k, v)) = line.split_once('=') {
            let key = k.trim();
            if std::env::var(key).is_err() {
                std::env::set_var(key, sanitize_env_value(v));
            }
        }
    }

    sanitize_tracing_env();
}

pub fn sanitize_tracing_env() {
    sanitize_known_env_var("RUST_LOG");
}

fn sanitize_known_env_var(key: &str) {
    let Ok(value) = std::env::var(key) else {
        return;
    };
    let sanitized = sanitize_env_value(&value);
    if sanitized != value {
        std::env::set_var(key, sanitized);
    }
}

fn sanitize_env_value(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.len() >= 2 {
        let bytes = trimmed.as_bytes();
        let first = bytes[0];
        let last = bytes[trimmed.len() - 1];
        if (first == b'\'' && last == b'\'') || (first == b'"' && last == b'"') {
            return trimmed[1..trimmed.len() - 1].to_string();
        }
    }
    trimmed.to_string()
}

fn env_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".clawbro")
        .join(".env")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_env_value_strips_matching_quotes() {
        assert_eq!(sanitize_env_value("'info'"), "info");
        assert_eq!(sanitize_env_value("\"info\""), "info");
        assert_eq!(sanitize_env_value("  'debug'  "), "debug");
    }

    #[test]
    fn sanitize_env_value_keeps_unquoted_content() {
        assert_eq!(sanitize_env_value("info"), "info");
        assert_eq!(sanitize_env_value("abc=123"), "abc=123");
        assert_eq!(sanitize_env_value("'mismatch\""), "'mismatch\"");
    }

    #[test]
    fn sanitize_tracing_env_strips_quotes_from_rust_log() {
        std::env::set_var("RUST_LOG", "'info'");
        sanitize_tracing_env();
        assert_eq!(std::env::var("RUST_LOG").unwrap(), "info");
    }
}
