//! WeChat iLink Bot Channel
//! Uses the official WeChat ClawBot iLink API for bot messaging.
//! Authentication: WECHAT_BOT_TOKEN env var or ~/.clawbro/channels/wechat/account.json

use crate::mention_parsing::{derive_fanout_message_id, extract_agent_mentions};
use crate::traits::Channel;
use anyhow::Result;
use async_trait::async_trait;
use base64::Engine as _;
use chrono::Utc;
use clawbro_protocol::{InboundMsg, MsgContent, MsgSource, OutboundMsg, SessionKey};
use rand::Rng;
use reqwest::header::HeaderMap;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const ILINK_BASE_URL: &str = "https://ilinkai.weixin.qq.com";
const CHANNEL_VERSION: &str = "1.0.0";
const DEFAULT_LONG_POLL_TIMEOUT_MS: u64 = 35_000;

// Session expiry
const SESSION_EXPIRED_ERRCODE: i64 = -14;
const SESSION_PAUSE_DURATION_MS: u64 = 60 * 60 * 1000; // 1 hour

// Backoff
const MAX_CONSECUTIVE_FAILURES: u32 = 3;
const BACKOFF_DELAY_MS: u64 = 30_000;
const RETRY_DELAY_MS: u64 = 2_000;

// Message type constants
const MSG_TYPE_USER: i64 = 1;
const MSG_TYPE_BOT: i64 = 2;
const MSG_STATE_FINISH: i64 = 2;
const MSG_ITEM_TEXT: i64 = 1;
const MSG_ITEM_VOICE: i64 = 3;

// ── Config ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct WeChatConfig {
    #[serde(rename = "token")]
    pub bot_token: String,
    #[serde(rename = "baseUrl", default)]
    pub base_url: String,
    #[serde(rename = "accountId", default)]
    pub account_id: String,
}

impl WeChatConfig {
    /// Load config: env var WECHAT_BOT_TOKEN first, then credential files.
    pub fn load() -> Result<Self> {
        // 1. Environment variable
        if let Ok(token) = std::env::var("WECHAT_BOT_TOKEN") {
            let base_url =
                std::env::var("WECHAT_BASE_URL").unwrap_or_else(|_| ILINK_BASE_URL.to_string());
            return Ok(Self {
                bot_token: token,
                base_url,
                account_id: String::new(),
            });
        }

        // 2. ClawBro credential file
        let path = Self::credentials_path();
        if path.exists() {
            return Self::load_from_file(&path);
        }

        // 3. Fallback: claude-code credential file
        let claude_path = dirs::home_dir()
            .unwrap_or_default()
            .join(".claude")
            .join("channels")
            .join("wechat")
            .join("account.json");
        if claude_path.exists() {
            tracing::info!(
                "WeChat: using claude-code credentials from {:?}",
                claude_path
            );
            return Self::load_from_file(&claude_path);
        }

        Err(anyhow::anyhow!(
            "WECHAT_BOT_TOKEN not set and no credential file found at {} or {}",
            path.display(),
            claude_path.display()
        ))
    }

    /// Load config from a specific credential file path.
    pub fn load_from_file(path: &std::path::Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("failed to read {}: {}", path.display(), e))?;
        let mut cfg: Self = serde_json::from_str(&content)?;
        if cfg.base_url.is_empty() {
            cfg.base_url = ILINK_BASE_URL.to_string();
        }
        Ok(cfg)
    }

    /// Path to the credential file.
    pub fn credentials_path() -> std::path::PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".clawbro")
            .join("channels")
            .join("wechat")
            .join("account.json")
    }
}

// ── iLink API helpers ────────────────────────────────────────────────────

/// Generate a random UIN for the X-WECHAT-UIN header.
/// Official: crypto.randomBytes(4).readUInt32BE(0) -> String -> base64
fn random_wechat_uin() -> String {
    let n: u32 = rand::thread_rng().gen();
    base64::engine::general_purpose::STANDARD.encode(n.to_string())
}

/// Build HTTP headers required by the iLink API.
fn build_headers(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("content-type", "application/json".parse().unwrap());
    headers.insert("authorizationtype", "ilink_bot_token".parse().unwrap());
    headers.insert("x-wechat-uin", random_wechat_uin().parse().unwrap());
    headers.insert(
        "Authorization",
        format!("Bearer {}", token).parse().unwrap(),
    );
    headers
}

/// Generate a unique client_id for message deduplication.
fn generate_client_id() -> String {
    format!("clawbro-wechat:{}", Uuid::new_v4())
}

// ── Markdown stripping ──────────────────────────────────────────────────

/// Strip markdown formatting from text for WeChat plain-text delivery.
fn strip_markdown(text: &str) -> String {
    use std::sync::LazyLock;

    static RE_IMAGE: LazyLock<regex_lite::Regex> =
        LazyLock::new(|| regex_lite::Regex::new(r"!\[[^\]]*\]\([^)]*\)").unwrap());
    static RE_LINK: LazyLock<regex_lite::Regex> =
        LazyLock::new(|| regex_lite::Regex::new(r"\[([^\]]+)\]\([^)]*\)").unwrap());
    static RE_BOLD_STAR: LazyLock<regex_lite::Regex> =
        LazyLock::new(|| regex_lite::Regex::new(r"\*\*([^*]+)\*\*").unwrap());
    static RE_BOLD_UNDER: LazyLock<regex_lite::Regex> =
        LazyLock::new(|| regex_lite::Regex::new(r"__([^_]+)__").unwrap());
    static RE_ITALIC_STAR: LazyLock<regex_lite::Regex> =
        LazyLock::new(|| regex_lite::Regex::new(r"\*([^*]+)\*").unwrap());
    static RE_ITALIC_UNDER: LazyLock<regex_lite::Regex> =
        LazyLock::new(|| regex_lite::Regex::new(r"_([^_]+)_").unwrap());
    static RE_INLINE_CODE: LazyLock<regex_lite::Regex> =
        LazyLock::new(|| regex_lite::Regex::new(r"`([^`]+)`").unwrap());
    static RE_HEADING: LazyLock<regex_lite::Regex> =
        LazyLock::new(|| regex_lite::Regex::new(r"(?m)^#{1,6}\s+").unwrap());

    // Code blocks: strip fences, keep code content
    let mut out_lines = Vec::new();
    let mut in_code_block = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        out_lines.push(line);
    }
    let mut result = out_lines.join("\n");

    result = RE_IMAGE.replace_all(&result, "").to_string();
    result = RE_LINK.replace_all(&result, "$1").to_string();
    result = RE_BOLD_STAR.replace_all(&result, "$1").to_string();
    result = RE_BOLD_UNDER.replace_all(&result, "$1").to_string();
    result = RE_ITALIC_STAR.replace_all(&result, "$1").to_string();
    result = RE_ITALIC_UNDER.replace_all(&result, "$1").to_string();
    result = RE_INLINE_CODE.replace_all(&result, "$1").to_string();
    result = RE_HEADING.replace_all(&result, "").to_string();

    result
}

// ── Message types (deserialization) ──────────────────────────────────────

#[derive(Debug, Deserialize)]
struct GetUpdatesResp {
    ret: Option<i64>,
    errcode: Option<i64>,
    #[allow(dead_code)]
    errmsg: Option<String>,
    #[serde(default)]
    msgs: Vec<WeixinMessage>,
    #[serde(default)]
    get_updates_buf: Option<String>,
    #[serde(default)]
    longpolling_timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct GetConfigResp {
    ret: Option<i64>,
    #[allow(dead_code)]
    errmsg: Option<String>,
    #[serde(default)]
    typing_ticket: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WeixinMessage {
    #[serde(default)]
    from_user_id: String,
    #[allow(dead_code)]
    #[serde(default)]
    to_user_id: String,
    #[serde(default)]
    message_type: i64,
    #[allow(dead_code)]
    #[serde(default)]
    message_state: i64,
    #[serde(default)]
    context_token: String,
    #[serde(default)]
    item_list: Vec<MessageItem>,
}

#[derive(Debug, Deserialize)]
struct MessageItem {
    #[serde(default, rename = "type")]
    item_type: i64,
    #[serde(default)]
    text_item: Option<TextItem>,
    #[serde(default)]
    voice_item: Option<VoiceItem>,
    #[serde(default)]
    ref_msg: Option<RefMsg>,
}

#[derive(Debug, Deserialize)]
struct TextItem {
    #[serde(default)]
    text: String,
}

#[derive(Debug, Deserialize)]
struct VoiceItem {
    #[serde(default)]
    text: String,
}

#[derive(Debug, Deserialize)]
struct RefMsg {
    #[serde(default)]
    title: String,
}

// ── Text extraction ─────────────────────────────────────────────────────

fn extract_text(msg: &WeixinMessage) -> String {
    let mut parts: Vec<String> = Vec::new();

    for item in &msg.item_list {
        if item.item_type == MSG_ITEM_TEXT {
            if let Some(ref ti) = item.text_item {
                let t = ti.text.trim();
                if !t.is_empty() {
                    parts.push(t.to_string());
                }
            }
        } else if item.item_type == MSG_ITEM_VOICE {
            if let Some(ref vi) = item.voice_item {
                let t = vi.text.trim();
                if !t.is_empty() {
                    parts.push(t.to_string());
                }
            }
        }

        if let Some(ref rm) = item.ref_msg {
            let t = rm.title.trim();
            if !t.is_empty() {
                parts.push(format!("> {}", t));
            }
        }
    }

    parts.join("\n")
}

// ── Scope derivation ────────────────────────────────────────────────────

fn derive_scope(from_user_id: &str) -> String {
    format!("user:{}", from_user_id)
}

// ── Context token cache ─────────────────────────────────────────────────

struct ContextTokenCache {
    inner: Mutex<HashMap<String, String>>,
    path: Option<std::path::PathBuf>,
}

impl ContextTokenCache {
    /// Create a new cache, loading from default path if available.
    /// The cache file is isolated by `account_id` so that switching bot accounts
    /// does not reuse stale context_tokens from a different bot_token.
    fn new(account_id: &str) -> Self {
        let path = Self::default_path(account_id);
        let initial = Self::load_from_file(&path);
        Self {
            inner: Mutex::new(initial),
            path: Some(path),
        }
    }

    /// Create a cache with a specific file path (for testing).
    #[cfg(test)]
    fn with_path(path: std::path::PathBuf) -> Self {
        let initial = Self::load_from_file(&path);
        Self {
            inner: Mutex::new(initial),
            path: Some(path),
        }
    }

    fn default_path(account_id: &str) -> std::path::PathBuf {
        let dir_name: String = if account_id.is_empty() {
            "_default".to_string()
        } else {
            account_id
                .chars()
                .map(|c| match c {
                    '/' | '\\' | ':' => '_',
                    _ => c,
                })
                .collect()
        };
        dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".clawbro")
            .join("channels")
            .join("wechat")
            .join(dir_name)
            .join("context_tokens.json")
    }

    fn load_from_file(path: &std::path::Path) -> HashMap<String, String> {
        match std::fs::read_to_string(path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => HashMap::new(),
        }
    }

    fn set(&self, user_id: &str, token: &str) {
        let mut map = self.inner.lock().unwrap();
        map.insert(user_id.to_string(), token.to_string());
        if let Some(ref path) = self.path {
            Self::save_to_file(path, &map);
        }
    }

    fn get(&self, user_id: &str) -> Option<String> {
        self.inner.lock().unwrap().get(user_id).cloned()
    }

    fn save_to_file(path: &std::path::Path, map: &HashMap<String, String>) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(content) = serde_json::to_string(map) {
            let _ = std::fs::write(path, content);
        }
    }
}

// ── Typing ticket cache (via getConfig) ─────────────────────────────────

struct TypingTicketCache {
    inner: Mutex<HashMap<String, (String, u64)>>,
}

const TYPING_TICKET_TTL_MS: u64 = 24 * 60 * 60 * 1000;

impl TypingTicketCache {
    fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    fn get(&self, user_id: &str) -> Option<String> {
        let map = self.inner.lock().unwrap();
        if let Some((ticket, expiry)) = map.get(user_id) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            if now < *expiry && !ticket.is_empty() {
                return Some(ticket.clone());
            }
        }
        None
    }

    fn set(&self, user_id: &str, ticket: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        self.inner.lock().unwrap().insert(
            user_id.to_string(),
            (ticket.to_string(), now + TYPING_TICKET_TTL_MS),
        );
    }
}

// ── Sync cursor persistence ─────────────────────────────────────────────

fn sync_buf_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".clawbro")
        .join("channels")
        .join("wechat")
        .join("sync_buf.txt")
}

fn load_sync_buf() -> Option<String> {
    std::fs::read_to_string(sync_buf_path()).ok().and_then(|s| {
        let trimmed = s.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

fn save_sync_buf(buf: &str) {
    let path = sync_buf_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, buf);
}

// ── WeChatChannel ───────────────────────────────────────────────────────

pub struct WeChatChannel {
    config: WeChatConfig,
    client: reqwest::Client,
    context_tokens: ContextTokenCache,
    typing_tickets: TypingTicketCache,
    session_paused_until: AtomicU64,
    #[allow(dead_code)]
    require_mention_in_groups: bool,
    cancel: CancellationToken,
}

impl WeChatChannel {
    pub fn new(
        config: WeChatConfig,
        require_mention_in_groups: bool,
        cancel: CancellationToken,
    ) -> Self {
        let account_id = config.account_id.clone();
        Self {
            config,
            client: reqwest::Client::new(),
            context_tokens: ContextTokenCache::new(&account_id),
            typing_tickets: TypingTicketCache::new(),
            session_paused_until: AtomicU64::new(0),
            require_mention_in_groups,
            cancel,
        }
    }

    fn is_session_paused(&self) -> bool {
        let until = self.session_paused_until.load(Ordering::Relaxed);
        if until == 0 {
            return false;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        if now >= until {
            self.session_paused_until.store(0, Ordering::Relaxed);
            return false;
        }
        true
    }

    fn pause_session(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let until = now + SESSION_PAUSE_DURATION_MS;
        self.session_paused_until.store(until, Ordering::Relaxed);
        tracing::error!(
            "WeChat: session expired (errcode={}), pausing all API calls for {} minutes",
            SESSION_EXPIRED_ERRCODE,
            SESSION_PAUSE_DURATION_MS / 60_000
        );
    }

    fn remaining_pause_ms(&self) -> u64 {
        let until = self.session_paused_until.load(Ordering::Relaxed);
        if until == 0 {
            return 0;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        until.saturating_sub(now)
    }

    async fn fetch_typing_ticket(&self, user_id: &str, context_token: &str) -> Option<String> {
        if let Some(ticket) = self.typing_tickets.get(user_id) {
            return Some(ticket);
        }

        let url = format!("{}/ilink/bot/getconfig", self.config.base_url);
        let headers = build_headers(&self.config.bot_token);
        let body = serde_json::json!({
            "ilink_user_id": user_id,
            "context_token": context_token,
            "base_info": { "channel_version": CHANNEL_VERSION }
        });

        let result = self
            .client
            .post(&url)
            .headers(headers)
            .json(&body)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await;

        match result {
            Ok(resp) => match resp.error_for_status() {
                Ok(r) => match r.json::<GetConfigResp>().await {
                    Ok(config_resp) => {
                        if config_resp.ret == Some(0) || config_resp.ret.is_none() {
                            let ticket = config_resp.typing_ticket.unwrap_or_default();
                            if !ticket.is_empty() {
                                self.typing_tickets.set(user_id, &ticket);
                                return Some(ticket);
                            }
                        }
                        None
                    }
                    Err(e) => {
                        tracing::debug!("WeChat getConfig parse error for {user_id}: {e}");
                        None
                    }
                },
                Err(e) => {
                    tracing::debug!("WeChat getConfig HTTP error for {user_id}: {e}");
                    None
                }
            },
            Err(e) => {
                tracing::debug!("WeChat getConfig request error for {user_id}: {e}");
                None
            }
        }
    }
}

#[async_trait]
impl Channel for WeChatChannel {
    fn name(&self) -> &str {
        "wechat"
    }

    async fn send(&self, msg: &OutboundMsg) -> Result<()> {
        if self.is_session_paused() {
            tracing::warn!(
                "WeChat: session paused, dropping outbound message ({}min remaining)",
                self.remaining_pause_ms() / 60_000
            );
            return Ok(());
        }

        let text = match &msg.content {
            MsgContent::Text { text } => strip_markdown(text),
            _ => "[unsupported content type]".to_string(),
        };

        let scope = &msg.session_key.scope;
        let to_user_id = scope.strip_prefix("user:").unwrap_or(scope.as_str());

        let context_token = match self.context_tokens.get(to_user_id) {
            Some(t) if !t.is_empty() => t,
            _ => {
                tracing::error!(
                    "WeChat: context_token missing for user {to_user_id}, refusing to send"
                );
                return Err(anyhow::anyhow!(
                    "context_token is required for sendMessage but missing for user {to_user_id}"
                ));
            }
        };

        let url = format!("{}/ilink/bot/sendmessage", self.config.base_url);
        let headers = build_headers(&self.config.bot_token);
        let client_id = generate_client_id();

        let body = serde_json::json!({
            "msg": {
                "from_user_id": "",
                "to_user_id": to_user_id,
                "client_id": client_id,
                "message_type": MSG_TYPE_BOT,
                "message_state": MSG_STATE_FINISH,
                "context_token": context_token,
                "item_list": [
                    {
                        "type": MSG_ITEM_TEXT,
                        "text_item": { "text": text }
                    }
                ]
            },
            "base_info": { "channel_version": CHANNEL_VERSION }
        });

        let client = self.client.clone();
        crate::send_with_retry(|| client.post(&url).headers(headers.clone()).json(&body)).await?;

        Ok(())
    }

    async fn listen(&self, tx: mpsc::Sender<InboundMsg>) -> Result<()> {
        let checker = crate::allowlist::AllowlistChecker::load();
        let mut sync_buf = load_sync_buf();
        let mut next_timeout_ms = DEFAULT_LONG_POLL_TIMEOUT_MS;
        let mut consecutive_failures: u32 = 0;

        while !self.cancel.is_cancelled() {
            if self.is_session_paused() {
                let remaining = self.remaining_pause_ms();
                tracing::info!("WeChat: session paused, sleeping {}min", remaining / 60_000);
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_millis(remaining)) => {},
                    _ = self.cancel.cancelled() => break,
                }
                continue;
            }

            let url = format!("{}/ilink/bot/getupdates", self.config.base_url);
            let headers = build_headers(&self.config.bot_token);

            let body = serde_json::json!({
                "get_updates_buf": sync_buf.as_deref().unwrap_or(""),
                "base_info": { "channel_version": CHANNEL_VERSION }
            });

            let http_fut = self
                .client
                .post(&url)
                .headers(headers)
                .json(&body)
                .timeout(std::time::Duration::from_millis(next_timeout_ms + 5_000))
                .send();

            let result = tokio::select! {
                r = http_fut => Some(r),
                _ = self.cancel.cancelled() => None,
            };
            let result = match result {
                Some(r) => r,
                None => break,
            };

            let resp = match result {
                Ok(r) => r,
                Err(e) if e.is_timeout() => {
                    tracing::debug!("WeChat long-poll timeout, retrying");
                    continue;
                }
                Err(e) => {
                    consecutive_failures += 1;
                    tracing::warn!(
                        "WeChat getupdates error ({consecutive_failures}/{MAX_CONSECUTIVE_FAILURES}): {e}"
                    );
                    if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                        consecutive_failures = 0;
                        tokio::select! {
                            _ = tokio::time::sleep(std::time::Duration::from_millis(BACKOFF_DELAY_MS)) => {},
                            _ = self.cancel.cancelled() => break,
                        }
                    } else {
                        tokio::select! {
                            _ = tokio::time::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS)) => {},
                            _ = self.cancel.cancelled() => break,
                        }
                    }
                    continue;
                }
            };

            let updates: GetUpdatesResp = match resp.error_for_status() {
                Ok(r) => match r.json().await {
                    Ok(u) => u,
                    Err(e) => {
                        consecutive_failures += 1;
                        tracing::warn!("WeChat getupdates parse error: {e}");
                        tokio::select! {
                            _ = tokio::time::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS)) => {},
                            _ = self.cancel.cancelled() => break,
                        }
                        continue;
                    }
                },
                Err(e) => {
                    consecutive_failures += 1;
                    tracing::warn!("WeChat getupdates HTTP error: {e}");
                    if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                        consecutive_failures = 0;
                        tokio::select! {
                            _ = tokio::time::sleep(std::time::Duration::from_millis(BACKOFF_DELAY_MS)) => {},
                            _ = self.cancel.cancelled() => break,
                        }
                    } else {
                        tokio::select! {
                            _ = tokio::time::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS)) => {},
                            _ = self.cancel.cancelled() => break,
                        }
                    }
                    continue;
                }
            };

            if let Some(t) = updates.longpolling_timeout_ms {
                if t > 0 {
                    next_timeout_ms = t;
                }
            }

            let is_api_error = matches!(updates.ret, Some(r) if r != 0)
                || matches!(updates.errcode, Some(e) if e != 0);

            if is_api_error {
                let is_session_expired = updates.errcode == Some(SESSION_EXPIRED_ERRCODE)
                    || updates.ret == Some(SESSION_EXPIRED_ERRCODE);

                if is_session_expired {
                    self.pause_session();
                    continue;
                }

                consecutive_failures += 1;
                tracing::warn!(
                    "WeChat getupdates API error: ret={:?} errcode={:?} errmsg={:?} ({consecutive_failures}/{MAX_CONSECUTIVE_FAILURES})",
                    updates.ret,
                    updates.errcode,
                    updates.errmsg,
                );
                if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    consecutive_failures = 0;
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_millis(BACKOFF_DELAY_MS)) => {},
                        _ = self.cancel.cancelled() => break,
                    }
                } else {
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS)) => {},
                        _ = self.cancel.cancelled() => break,
                    }
                }
                continue;
            }

            consecutive_failures = 0;

            if let Some(ref buf) = updates.get_updates_buf {
                if !buf.is_empty() {
                    sync_buf = Some(buf.clone());
                    save_sync_buf(buf);
                }
            }

            for msg in &updates.msgs {
                if msg.message_type != MSG_TYPE_USER {
                    continue;
                }

                let content_text = extract_text(msg);
                if content_text.trim().is_empty() {
                    continue;
                }

                if !msg.context_token.is_empty() {
                    self.context_tokens
                        .set(&msg.from_user_id, &msg.context_token);
                }

                if !checker.is_allowed("wechat", &msg.from_user_id) {
                    tracing::debug!("AllowlistChecker: wechat user {} denied", msg.from_user_id);
                    continue;
                }

                let scope = derive_scope(&msg.from_user_id);
                let event_id = Uuid::new_v4().to_string();

                let targets = extract_agent_mentions(&content_text);
                if targets.is_empty() {
                    let inbound = InboundMsg {
                        id: event_id,
                        session_key: SessionKey::new("wechat", &scope),
                        content: MsgContent::text(&content_text),
                        sender: msg.from_user_id.clone(),
                        channel: "wechat".to_string(),
                        timestamp: Utc::now(),
                        thread_ts: None,
                        target_agent: None,
                        source: MsgSource::Human,
                    };
                    tokio::select! {
                        _ = tx.send(inbound) => {},
                        _ = self.cancel.cancelled() => break,
                    }
                } else {
                    let mut cancelled = false;
                    for target_agent in targets {
                        let inbound = InboundMsg {
                            id: derive_fanout_message_id(&event_id, Some(&target_agent)),
                            session_key: SessionKey::new("wechat", &scope),
                            content: MsgContent::text(&content_text),
                            sender: msg.from_user_id.clone(),
                            channel: "wechat".to_string(),
                            timestamp: Utc::now(),
                            thread_ts: None,
                            target_agent: Some(target_agent),
                            source: MsgSource::Human,
                        };
                        tokio::select! {
                            _ = tx.send(inbound) => {},
                            _ = self.cancel.cancelled() => { cancelled = true; break; },
                        }
                    }
                    if cancelled {
                        break;
                    }
                }
            }
        }

        tracing::info!("WeChat: listen loop stopped (cancelled)");
        Ok(())
    }

    async fn update_typing(&self, scope: &str) -> Result<()> {
        if self.is_session_paused() {
            return Ok(());
        }

        let to_user_id = scope.strip_prefix("user:").unwrap_or(scope);
        let context_token = match self.context_tokens.get(to_user_id) {
            Some(t) if !t.is_empty() => t,
            _ => return Ok(()),
        };

        let typing_ticket = match self.fetch_typing_ticket(to_user_id, &context_token).await {
            Some(t) => t,
            None => {
                tracing::debug!(
                    "WeChat: no typing_ticket for {to_user_id}, skipping typing indicator"
                );
                return Ok(());
            }
        };

        let body = serde_json::json!({
            "ilink_user_id": to_user_id,
            "typing_ticket": typing_ticket,
            "status": 1,
            "base_info": { "channel_version": CHANNEL_VERSION }
        });
        let headers = build_headers(&self.config.bot_token);
        let url = format!("{}/ilink/bot/sendtyping", self.config.base_url);
        let _ = self
            .client
            .post(&url)
            .headers(headers)
            .json(&body)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_scope() {
        assert_eq!(derive_scope("user_abc"), "user:user_abc");
        assert_eq!(derive_scope("12345"), "user:12345");
    }

    #[test]
    fn test_random_wechat_uin_is_nonempty() {
        let uin = random_wechat_uin();
        assert!(!uin.is_empty());
        assert!(base64::engine::general_purpose::STANDARD
            .decode(&uin)
            .is_ok());
    }

    #[test]
    fn test_extract_text_simple() {
        let msg = WeixinMessage {
            from_user_id: "u1".into(),
            to_user_id: "bot".into(),
            message_type: MSG_TYPE_USER,
            message_state: 0,
            context_token: "ctx".into(),
            item_list: vec![MessageItem {
                item_type: MSG_ITEM_TEXT,
                text_item: Some(TextItem {
                    text: "hello world".into(),
                }),
                voice_item: None,
                ref_msg: None,
            }],
        };
        assert_eq!(extract_text(&msg), "hello world");
    }

    #[test]
    fn test_extract_text_with_ref() {
        let msg = WeixinMessage {
            from_user_id: "u1".into(),
            to_user_id: "bot".into(),
            message_type: MSG_TYPE_USER,
            message_state: 0,
            context_token: "ctx".into(),
            item_list: vec![MessageItem {
                item_type: MSG_ITEM_TEXT,
                text_item: Some(TextItem {
                    text: "my reply".into(),
                }),
                voice_item: None,
                ref_msg: Some(RefMsg {
                    title: "original message".into(),
                }),
            }],
        };
        let text = extract_text(&msg);
        assert!(text.contains("my reply"));
        assert!(text.contains("> original message"));
    }

    #[test]
    fn test_extract_text_voice() {
        let msg = WeixinMessage {
            from_user_id: "u1".into(),
            to_user_id: "bot".into(),
            message_type: MSG_TYPE_USER,
            message_state: 0,
            context_token: "ctx".into(),
            item_list: vec![MessageItem {
                item_type: MSG_ITEM_VOICE,
                text_item: None,
                voice_item: Some(VoiceItem {
                    text: "voice transcription".into(),
                }),
                ref_msg: None,
            }],
        };
        assert_eq!(extract_text(&msg), "voice transcription");
    }

    #[test]
    fn test_extract_text_empty() {
        let msg = WeixinMessage {
            from_user_id: "u1".into(),
            to_user_id: "bot".into(),
            message_type: MSG_TYPE_USER,
            message_state: 0,
            context_token: "ctx".into(),
            item_list: vec![],
        };
        assert_eq!(extract_text(&msg), "");
    }

    #[test]
    fn test_context_token_cache() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ContextTokenCache::with_path(dir.path().join("ctx.json"));
        assert!(cache.get("user1").is_none());

        cache.set("user1", "token_a");
        assert_eq!(cache.get("user1").unwrap(), "token_a");

        cache.set("user1", "token_b");
        assert_eq!(cache.get("user1").unwrap(), "token_b");
    }

    #[test]
    fn test_context_token_cache_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("context_tokens.json");

        // Create cache, set tokens, verify file written
        {
            let cache = ContextTokenCache::with_path(path.clone());
            cache.set("user_a", "tok_1");
            cache.set("user_b", "tok_2");
            assert!(path.exists(), "context_tokens.json should be created");
        }

        // Create new cache from same path, verify tokens loaded from disk
        {
            let cache = ContextTokenCache::with_path(path.clone());
            assert_eq!(cache.get("user_a").unwrap(), "tok_1");
            assert_eq!(cache.get("user_b").unwrap(), "tok_2");
            assert!(cache.get("nonexistent").is_none());
        }
    }

    #[test]
    fn test_context_token_cache_handles_corrupted_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("context_tokens.json");
        std::fs::write(&path, "not valid json{{{").unwrap();

        // Should not panic, starts with empty map
        let cache = ContextTokenCache::with_path(path);
        assert!(cache.get("any").is_none());
        // Should be able to set and get
        cache.set("user1", "tok1");
        assert_eq!(cache.get("user1").unwrap(), "tok1");
    }

    #[test]
    fn test_config_credentials_path() {
        let path = WeChatConfig::credentials_path();
        let path_str = path.to_string_lossy();
        assert!(path_str.contains("wechat"), "path should contain 'wechat'");
        assert!(
            path_str.contains("account.json"),
            "path should contain 'account.json'"
        );
    }

    #[test]
    fn test_send_body_structure() {
        let text = "你好！有什么可以帮你的吗？";
        let to_user_id = "o9cq800kum_xxx@im.wechat";
        let context_token = "AARzJWAFAAABAAAAAAAp";
        let client_id = generate_client_id();

        let body = serde_json::json!({
            "msg": {
                "from_user_id": "",
                "to_user_id": to_user_id,
                "client_id": client_id,
                "message_type": MSG_TYPE_BOT,
                "message_state": MSG_STATE_FINISH,
                "context_token": context_token,
                "item_list": [{ "type": MSG_ITEM_TEXT, "text_item": { "text": text } }]
            },
            "base_info": { "channel_version": CHANNEL_VERSION }
        });

        let msg = &body["msg"];
        assert_eq!(msg["from_user_id"], "");
        assert_eq!(msg["to_user_id"], to_user_id);
        assert_eq!(msg["message_type"], 2);
        assert_eq!(msg["message_state"], 2);
        assert_eq!(msg["context_token"], context_token);
        assert!(msg["client_id"]
            .as_str()
            .unwrap()
            .starts_with("clawbro-wechat:"));
        assert_eq!(msg["item_list"][0]["type"], 1);
        assert_eq!(msg["item_list"][0]["text_item"]["text"], text);
        assert_eq!(body["base_info"]["channel_version"], CHANNEL_VERSION);
    }

    #[test]
    fn test_getupdates_response_parsing() {
        let json = r#"{
            "ret": 0,
            "msgs": [
                {
                    "from_user_id": "user1@im.wechat",
                    "to_user_id": "bot1@im.bot",
                    "message_type": 1,
                    "message_state": 2,
                    "context_token": "ctx_abc",
                    "item_list": [
                        { "type": 1, "text_item": { "text": "你好" } }
                    ]
                },
                {
                    "from_user_id": "bot1@im.bot",
                    "to_user_id": "user1@im.wechat",
                    "message_type": 2,
                    "message_state": 2,
                    "context_token": "ctx_abc",
                    "item_list": [
                        { "type": 1, "text_item": { "text": "你好！" } }
                    ]
                }
            ],
            "get_updates_buf": "new_cursor_value"
        }"#;

        let resp: GetUpdatesResp = serde_json::from_str(json).unwrap();
        assert_eq!(resp.ret, Some(0));
        assert_eq!(resp.msgs.len(), 2);
        assert_eq!(resp.get_updates_buf, Some("new_cursor_value".to_string()));

        let user_msgs: Vec<_> = resp
            .msgs
            .iter()
            .filter(|m| m.message_type == MSG_TYPE_USER)
            .collect();
        assert_eq!(user_msgs.len(), 1);
        assert_eq!(extract_text(user_msgs[0]), "你好");
    }

    #[test]
    fn test_getupdates_session_expired() {
        let json = r#"{ "ret": -14, "errcode": -14, "errmsg": "session expired", "msgs": [] }"#;
        let resp: GetUpdatesResp = serde_json::from_str(json).unwrap();
        assert_eq!(resp.errcode, Some(SESSION_EXPIRED_ERRCODE));
    }

    #[test]
    fn test_getupdates_with_longpolling_timeout() {
        let json = r#"{ "ret": 0, "msgs": [], "get_updates_buf": "buf", "longpolling_timeout_ms": 45000 }"#;
        let resp: GetUpdatesResp = serde_json::from_str(json).unwrap();
        assert_eq!(resp.longpolling_timeout_ms, Some(45000));
    }

    #[test]
    fn test_build_headers() {
        let headers = build_headers("test_token_123");
        assert_eq!(headers.get("authorizationtype").unwrap(), "ilink_bot_token");
        assert!(headers.get("x-wechat-uin").is_some());
        assert_eq!(headers.get("content-type").unwrap(), "application/json");
        let auth = headers
            .get(reqwest::header::AUTHORIZATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(auth.starts_with("Bearer test_token_123"));
    }

    #[test]
    fn test_generate_client_id() {
        let id = generate_client_id();
        assert!(id.starts_with("clawbro-wechat:"));
        let id2 = generate_client_id();
        assert_ne!(id, id2);
    }

    #[test]
    fn test_typing_ticket_cache() {
        let cache = TypingTicketCache::new();
        assert!(cache.get("user1").is_none());
        cache.set("user1", "ticket_abc");
        assert_eq!(cache.get("user1").unwrap(), "ticket_abc");
    }

    #[test]
    fn test_getconfig_response_parsing() {
        let json = r#"{"ret": 0, "typing_ticket": "ticket_xyz"}"#;
        let resp: GetConfigResp = serde_json::from_str(json).unwrap();
        assert_eq!(resp.ret, Some(0));
        assert_eq!(resp.typing_ticket, Some("ticket_xyz".to_string()));
    }

    #[test]
    fn test_sendtyping_body_structure() {
        let body = serde_json::json!({
            "ilink_user_id": "user1@im.wechat",
            "typing_ticket": "ticket_abc",
            "status": 1,
            "base_info": { "channel_version": CHANNEL_VERSION }
        });
        assert_eq!(body["ilink_user_id"], "user1@im.wechat");
        assert_eq!(body["typing_ticket"], "ticket_abc");
        assert_eq!(body["status"], 1);
    }

    #[test]
    fn test_strip_markdown() {
        assert_eq!(strip_markdown("```rust\nfn main() {}\n```"), "fn main() {}");
        assert_eq!(strip_markdown("**bold text**"), "bold text");
        assert_eq!(strip_markdown("*italic*"), "italic");
        assert_eq!(
            strip_markdown("[click here](https://example.com)"),
            "click here"
        );
        assert_eq!(strip_markdown("![alt](img.png)"), "");
        assert_eq!(strip_markdown("`code`"), "code");
        assert_eq!(strip_markdown("## Heading"), "Heading");
    }

    #[tokio::test]
    async fn test_listen_respects_cancellation() {
        use tokio_util::sync::CancellationToken;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            let _ = accepted_tx.send(());
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        });

        let config = WeChatConfig {
            bot_token: "test_token".into(),
            base_url: format!("http://{}", addr),
            account_id: "test".into(),
        };
        let token = CancellationToken::new();
        let ch = WeChatChannel::new(config, false, token.clone());
        let (tx, _rx) = tokio::sync::mpsc::channel(1);

        let t = token.clone();
        tokio::spawn(async move {
            let _ = accepted_rx.await;
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            t.cancel();
        });

        let result = tokio::time::timeout(std::time::Duration::from_secs(2), ch.listen(tx)).await;

        assert!(result.is_ok(), "listen() should return before timeout");
    }
}
