//! WeChat QR-code login flow.
//!
//! 1. `GET /ilink/bot/get_bot_qrcode?bot_type=3` -> QR code URL
//! 2. Long-poll `/ilink/bot/get_qrcode_status?qrcode=<qrcode>` until confirmed
//! 3. Save credentials to `~/.clawbro/channels/wechat/account.json`

use anyhow::{bail, Context, Result};
use fast_qr::qr::QRBuilder;
use serde::Deserialize;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const ILINK_BASE_URL: &str = "https://ilinkai.weixin.qq.com";
const POLL_REQUEST_TIMEOUT: Duration = Duration::from_secs(35);
const TOTAL_DEADLINE: Duration = Duration::from_secs(480);

// ── API response types ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct QrCodeResp {
    #[serde(default)]
    errcode: Option<i64>,
    #[serde(default)]
    errmsg: Option<String>,
    #[serde(default)]
    qrcode: Option<String>,
    /// Official field is `qrcode_img_content`; alias `qrcode_url` for compat.
    #[serde(default, alias = "qrcode_url")]
    qrcode_img_content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QrStatusResp {
    #[serde(default)]
    #[allow(dead_code)]
    errcode: Option<i64>,
    #[serde(default)]
    #[allow(dead_code)]
    errmsg: Option<String>,
    #[serde(default)]
    status: Option<String>,
    /// Official field is `bot_token`; alias `token` for compat.
    #[serde(default, alias = "token")]
    bot_token: Option<String>,
    /// Official field is `baseurl` (lowercase); alias `baseUrl` for compat.
    #[serde(default, alias = "baseUrl")]
    baseurl: Option<String>,
    /// Official field is `ilink_bot_id`; alias `accountId` for compat.
    #[serde(default, alias = "accountId")]
    ilink_bot_id: Option<String>,
    /// Official field is `ilink_user_id`; alias `userId` for compat.
    #[serde(default, alias = "userId")]
    ilink_user_id: Option<String>,
}

// ── Public API ──────────────────────────────────────────────────────────

/// Run the full WeChat QR-code login flow.
///
/// Prints a terminal QR code plus URL fallback to stderr, long-polls for confirmation,
/// then saves credentials and returns the file path.
pub async fn wechat_login() -> Result<PathBuf> {
    let client = reqwest::Client::new();

    // Step 1: Get QR code
    let url = format!("{}/ilink/bot/get_bot_qrcode?bot_type=3", ILINK_BASE_URL);
    let resp: QrCodeResp = client
        .get(&url)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .context("failed to request QR code")?
        .error_for_status()
        .context("QR code request returned HTTP error")?
        .json()
        .await
        .context("failed to parse QR code response")?;

    if resp.errcode.unwrap_or(0) != 0 {
        bail!(
            "QR code request failed: errcode={}, errmsg={}",
            resp.errcode.unwrap_or(-1),
            resp.errmsg.as_deref().unwrap_or("unknown")
        );
    }

    let qrcode = resp
        .qrcode
        .as_deref()
        .filter(|s| !s.is_empty())
        .context("QR code response missing 'qrcode' field")?;

    let qrcode_url = resp
        .qrcode_img_content
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(qrcode);

    // Step 2: Display QR code in terminal + URL fallback
    eprintln!();
    eprintln!("=== WeChat QR Login ===");
    eprintln!("Scan this QR code with WeChat:");
    eprintln!();
    if let Some(qr) = render_terminal_qr(qrcode_url) {
        eprintln!("{qr}");
        eprintln!();
        eprintln!("QR URL fallback:");
        eprintln!("  {}", qrcode_url);
    } else {
        eprintln!("  {}", qrcode_url);
    }
    eprintln!();
    eprintln!(
        "Waiting for confirmation (timeout: {}s)...",
        TOTAL_DEADLINE.as_secs()
    );

    // Step 3: Long-poll for status
    let deadline = Instant::now() + TOTAL_DEADLINE;
    let encoded_qrcode = urlencoding::encode(qrcode);

    loop {
        if Instant::now() >= deadline {
            bail!("QR login timed out after {}s", TOTAL_DEADLINE.as_secs());
        }

        let status_url = format!(
            "{}/ilink/bot/get_qrcode_status?qrcode={}",
            ILINK_BASE_URL, encoded_qrcode
        );

        let result = client
            .get(&status_url)
            .timeout(POLL_REQUEST_TIMEOUT)
            .send()
            .await;

        let resp = match result {
            Ok(r) => r,
            Err(e) if e.is_timeout() => {
                // Normal long-poll timeout, retry
                continue;
            }
            Err(e) => {
                eprintln!("Warning: poll request failed: {e}, retrying...");
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };

        let status_resp: QrStatusResp = match resp.error_for_status() {
            Ok(r) => match r.json().await {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Warning: failed to parse status response: {e}, retrying...");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
            },
            Err(e) => {
                eprintln!("Warning: status request HTTP error: {e}, retrying...");
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };

        let status = status_resp.status.as_deref().unwrap_or("");

        match status {
            "confirmed" | "success" => {
                let token = status_resp
                    .bot_token
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .context("confirmed but no bot_token in response")?;

                let base_url = status_resp
                    .baseurl
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(ILINK_BASE_URL);

                let account_id = status_resp.ilink_bot_id.as_deref().unwrap_or("");

                let user_id = status_resp.ilink_user_id.as_deref().unwrap_or("");

                let path = save_credentials(token, base_url, account_id, user_id)?;

                eprintln!();
                eprintln!("Login successful! Credentials saved to: {}", path.display());
                return Ok(path);
            }
            "expired" => {
                bail!("QR code expired. Please try again.");
            }
            "cancelled" => {
                bail!("Login was cancelled.");
            }
            _ => {
                // "waiting" or other intermediate states -- keep polling
                continue;
            }
        }
    }
}

// ── Credential persistence ──────────────────────────────────────────────

fn save_credentials(
    token: &str,
    base_url: &str,
    account_id: &str,
    user_id: &str,
) -> Result<PathBuf> {
    let path = credentials_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }

    let now = chrono::Utc::now().to_rfc3339();
    let creds = serde_json::json!({
        "token": token,
        "baseUrl": base_url,
        "accountId": account_id,
        "userId": user_id,
        "savedAt": now,
    });

    let content = serde_json::to_string_pretty(&creds)?;
    std::fs::write(&path, &content)
        .with_context(|| format!("failed to write credentials to {}", path.display()))?;

    // Set file permissions to 0o600 on unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to set permissions on {}", path.display()))?;
    }

    Ok(path)
}

fn credentials_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".clawbro")
        .join("channels")
        .join("wechat")
        .join("account.json")
}

fn render_terminal_qr(content: &str) -> Option<String> {
    let qr = QRBuilder::new(content).build().ok()?;
    let rendered = qr.to_str();
    (!rendered.trim().is_empty()).then_some(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_code_resp_parses_official_fields() {
        let json = r#"{"qrcode": "qr_value", "qrcode_img_content": "https://img.url"}"#;
        let resp: QrCodeResp = serde_json::from_str(json).unwrap();
        assert_eq!(resp.qrcode, Some("qr_value".to_string()));
        assert_eq!(resp.qrcode_img_content, Some("https://img.url".to_string()));
    }

    #[test]
    fn qr_code_resp_parses_alias_qrcode_url() {
        let json = r#"{"qrcode": "qr_value", "qrcode_url": "https://alt.url"}"#;
        let resp: QrCodeResp = serde_json::from_str(json).unwrap();
        assert_eq!(resp.qrcode_img_content, Some("https://alt.url".to_string()));
    }

    #[test]
    fn qr_status_resp_parses_official_fields() {
        let json = r#"{
            "status": "confirmed",
            "bot_token": "tok_123",
            "baseurl": "https://base.url",
            "ilink_bot_id": "bot_abc",
            "ilink_user_id": "user_xyz"
        }"#;
        let resp: QrStatusResp = serde_json::from_str(json).unwrap();
        assert_eq!(resp.status, Some("confirmed".to_string()));
        assert_eq!(resp.bot_token, Some("tok_123".to_string()));
        assert_eq!(resp.baseurl, Some("https://base.url".to_string()));
        assert_eq!(resp.ilink_bot_id, Some("bot_abc".to_string()));
        assert_eq!(resp.ilink_user_id, Some("user_xyz".to_string()));
    }

    #[test]
    fn qr_status_resp_parses_compat_aliases() {
        let json = r#"{
            "status": "success",
            "token": "tok_456",
            "baseUrl": "https://compat.url",
            "accountId": "acct_789",
            "userId": "uid_012"
        }"#;
        let resp: QrStatusResp = serde_json::from_str(json).unwrap();
        assert_eq!(resp.bot_token, Some("tok_456".to_string()));
        assert_eq!(resp.baseurl, Some("https://compat.url".to_string()));
        assert_eq!(resp.ilink_bot_id, Some("acct_789".to_string()));
        assert_eq!(resp.ilink_user_id, Some("uid_012".to_string()));
    }

    #[test]
    fn credentials_path_contains_expected_segments() {
        let p = credentials_path();
        let s = p.to_string_lossy();
        assert!(s.contains(".clawbro"));
        assert!(s.contains("wechat"));
        assert!(s.contains("account.json"));
    }

    #[test]
    fn render_terminal_qr_returns_multiline_unicode_output() {
        let rendered = render_terminal_qr("https://example.com").unwrap();
        assert!(!rendered.trim().is_empty());
        assert!(rendered.lines().count() > 5);
    }
}
