pub mod allowlist;
pub mod dingtalk;
pub mod lark;
mod mention_parsing;
pub mod mention_trigger;
pub mod traits;

pub use allowlist::AllowlistChecker;
pub use dingtalk::{DingTalkChannel, DingTalkConfig};
pub use lark::{LarkChannel, LarkTriggerMode, LarkTriggerPolicy};
pub use traits::{BoxChannel, Channel};

/// Maximum number of retries for transient send errors (not counting the first attempt).
pub(crate) const SEND_MAX_RETRIES: u32 = 3;
/// Initial retry delay in milliseconds; doubles each attempt.
pub(crate) const SEND_INITIAL_DELAY_MS: u64 = 200;

/// Execute `req_fn()` and retry up to `SEND_MAX_RETRIES` times on transient errors.
///
/// Retryable conditions: connection error, timeout, or HTTP 5xx response.
/// Non-retryable: HTTP 4xx, permanent errors.
pub(crate) async fn send_with_retry<F>(req_fn: F) -> anyhow::Result<()>
where
    F: Fn() -> reqwest::RequestBuilder,
{
    let mut delay_ms = SEND_INITIAL_DELAY_MS;
    for attempt in 0..=SEND_MAX_RETRIES {
        let result = req_fn().send().await;
        match result {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    return Ok(());
                } else if status.is_server_error() && attempt < SEND_MAX_RETRIES {
                    tracing::warn!(
                        attempt = attempt + 1,
                        delay_ms,
                        status = %status,
                        "channel send HTTP {status}, retrying"
                    );
                } else {
                    // 4xx or exhausted retries on 5xx
                    return Err(anyhow::anyhow!(
                        "channel send failed: HTTP {status} after {} attempt(s)",
                        attempt + 1
                    ));
                }
            }
            Err(e) if attempt < SEND_MAX_RETRIES && (e.is_connect() || e.is_timeout()) => {
                tracing::warn!(
                    attempt = attempt + 1,
                    delay_ms,
                    "channel send transient error, retrying: {e}"
                );
            }
            Err(e) => return Err(e.into()),
        }
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        delay_ms *= 2;
    }
    // All retries exhausted with a 5xx — the loop above returns Err on the final attempt,
    // so this point is only reachable if SEND_MAX_RETRIES is 0 and the match arm for
    // `attempt < SEND_MAX_RETRIES` is never taken. Treat it as a logic guard.
    Err(anyhow::anyhow!(
        "channel send failed: all {} retries exhausted",
        SEND_MAX_RETRIES
    ))
}
