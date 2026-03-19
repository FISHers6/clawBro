use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;
const DINGTALK_ROBOT_SEND_API: &str = "https://oapi.dingtalk.com/robot/send";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedQuery {
    pub timestamp_ms: i64,
    pub sign: String,
}

pub fn sign_dingtalk_secret(secret_key: &str, timestamp_ms: i64) -> Result<SignedQuery> {
    let mut mac = HmacSha256::new_from_slice(secret_key.as_bytes())
        .map_err(|e| anyhow::anyhow!("failed to initialize DingTalk HMAC: {e}"))?;
    let text_to_sign = format!("{timestamp_ms}\n{secret_key}");
    mac.update(text_to_sign.as_bytes());
    let sign = urlencoding::encode(&BASE64_STANDARD.encode(mac.finalize().into_bytes())).to_string();
    Ok(SignedQuery { timestamp_ms, sign })
}

pub async fn send_text_by_session_webhook(
    client: &reqwest::Client,
    secret_key: &str,
    session_webhook: &str,
    text: &str,
) -> Result<()> {
    let signed = sign_dingtalk_secret(secret_key, chrono::Utc::now().timestamp_millis())?;
    let separator = if session_webhook.contains('?') { '&' } else { '?' };
    let url = format!(
        "{session_webhook}{separator}timestamp={}&sign={}",
        signed.timestamp_ms, signed.sign
    );
    let body = serde_json::json!({
        "msgtype": "text",
        "text": { "content": text }
    });
    crate::channels_internal::send_with_retry(|| client.post(url.clone()).json(&body)).await
}

pub async fn send_text_by_robot_access_token(
    client: &reqwest::Client,
    secret_key: &str,
    access_token: &str,
    text: &str,
) -> Result<()> {
    let signed = sign_dingtalk_secret(secret_key, chrono::Utc::now().timestamp_millis())?;
    let url = format!(
        "{DINGTALK_ROBOT_SEND_API}?access_token={}&timestamp={}&sign={}",
        urlencoding::encode(access_token),
        signed.timestamp_ms,
        signed.sign
    );
    let body = serde_json::json!({
        "msgtype": "text",
        "text": { "content": text }
    });
    crate::channels_internal::send_with_retry(|| client.post(url.clone()).json(&body)).await
}

#[cfg(test)]
mod tests {
    use super::sign_dingtalk_secret;

    #[test]
    fn sign_dingtalk_secret_matches_reference_algorithm_shape() {
        let signed = sign_dingtalk_secret("SEC-test", 1_700_000_000_000).unwrap();
        assert_eq!(signed.timestamp_ms, 1_700_000_000_000);
        assert!(signed.sign.contains('%') || signed.sign.chars().all(|ch| ch.is_ascii_alphanumeric()));
        assert!(!signed.sign.is_empty());
    }
}
