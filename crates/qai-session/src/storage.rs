use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

/// 消息记录（与 quick-ai JSONL 格式兼容）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: Uuid,
    pub role: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>, // NEW: @claude / alice / cron etc.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallRecord>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fragment_event_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregation_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub name: String,
    pub input: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

/// ACP backend session 的运行状态，用于检测 Gateway 崩溃后遗留的卡死 session。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SessionStatus {
    Idle,
    Running {
        backend_id: String,
        started_at: DateTime<Utc>,
    },
}

impl SessionStatus {
    pub fn default_idle() -> Self {
        Self::Idle
    }
}

impl Default for SessionStatus {
    fn default() -> Self {
        Self::Idle
    }
}

/// Session 元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub session_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub channel: String,
    pub scope: String,
    pub message_count: usize,
    /// Per-backend 的后端侧 ACP session ID（backend_id → acp_session_id）。
    /// 用于下次 turn 时通过 ACP session/load 恢复后端 session 状态。
    #[serde(default)]
    pub backend_session_ids: std::collections::HashMap<String, String>,
    /// 当前运行状态，用于检测 Gateway 崩溃后卡死的 session。
    #[serde(default = "SessionStatus::default_idle")]
    pub session_status: SessionStatus,
}

/// Session 磁盘存储（append-only JSONL + metadata.json）
pub struct SessionStorage {
    base_dir: PathBuf,
}

impl SessionStorage {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    pub fn base_dir(&self) -> &std::path::Path {
        &self.base_dir
    }

    /// 默认路径: ~/.quickai/sessions/
    pub fn default_path() -> Result<Self> {
        let dir = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Cannot find home dir"))?
            .join(".quickai")
            .join("sessions");
        std::fs::create_dir_all(&dir)?;
        Ok(Self::new(dir))
    }

    fn session_dir(&self, session_id: Uuid) -> PathBuf {
        self.base_dir.join(session_id.to_string())
    }

    /// 追加一条消息到 JSONL 文件（append-only，兼容 quick-ai 格式）
    pub async fn append_message(&self, session_id: Uuid, msg: &StoredMessage) -> Result<()> {
        let dir = self.session_dir(session_id);
        tokio::fs::create_dir_all(&dir).await?;
        let path = dir.join("messages.jsonl");
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        let line = serde_json::to_string(msg)?;
        file.write_all(format!("{}\n", line).as_bytes()).await?;
        Ok(())
    }

    /// 读取该 session 的所有消息
    pub async fn load_messages(&self, session_id: Uuid) -> Result<Vec<StoredMessage>> {
        let path = self.session_dir(session_id).join("messages.jsonl");
        if !path.exists() {
            return Ok(vec![]);
        }
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(content) => content,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(err) => return Err(err.into()),
        };
        let msgs = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(serde_json::from_str)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(msgs)
    }

    /// 读取该 session 最近 `limit` 条消息（避免长对话将完整 JSONL 全部反序列化）。
    ///
    /// I/O 仍为 O(n)（append-only JSONL 无法 seek），但只解析最后 limit 行，
    /// 大幅减少长会话的堆分配。
    pub async fn load_recent_messages(
        &self,
        session_id: Uuid,
        limit: usize,
    ) -> Result<Vec<StoredMessage>> {
        let path = self.session_dir(session_id).join("messages.jsonl");
        if !path.exists() {
            return Ok(vec![]);
        }
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(content) => content,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(err) => return Err(err.into()),
        };
        let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
        let start = lines.len().saturating_sub(limit);
        lines[start..]
            .iter()
            .map(|l| serde_json::from_str(l).map_err(|e| anyhow::anyhow!(e)))
            .collect()
    }

    /// 写入/覆盖 metadata.json（原子写：先写 tmp，再 rename）
    pub async fn save_meta(&self, meta: &SessionMeta) -> Result<()> {
        let dir = self.session_dir(meta.session_id);
        tokio::fs::create_dir_all(&dir).await?;
        let path = dir.join("metadata.json");
        let tmp = dir.join("metadata.json.tmp");
        let json = serde_json::to_string_pretty(meta)?;
        tokio::fs::write(&tmp, json).await?;
        tokio::fs::rename(&tmp, &path).await?;
        Ok(())
    }

    /// 加载 metadata.json
    pub async fn load_meta(&self, session_id: Uuid) -> Result<Option<SessionMeta>> {
        let path = self.session_dir(session_id).join("metadata.json");
        if !path.exists() {
            return Ok(None);
        }
        let json = tokio::fs::read_to_string(&path).await?;
        Ok(Some(serde_json::from_str(&json)?))
    }

    fn message_path(&self, session_id: Uuid) -> PathBuf {
        self.session_dir(session_id).join("messages.jsonl")
    }

    /// 清除该 session 的所有消息（删除 JSONL 文件）
    pub async fn clear_messages(&self, session_id: Uuid) -> Result<()> {
        let path = self.message_path(session_id);
        if path.exists() {
            tokio::fs::remove_file(&path).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_append_and_load_messages() {
        let dir = tempdir().unwrap();
        let storage = SessionStorage::new(dir.path().to_path_buf());
        let session_id = Uuid::new_v4();
        let msg = StoredMessage {
            id: Uuid::new_v4(),
            role: "user".to_string(),
            content: "hello".to_string(),
            timestamp: Utc::now(),
            sender: None,
            tool_calls: None,
            fragment_event_ids: None,
            aggregation_mode: None,
        };
        storage.append_message(session_id, &msg).await.unwrap();
        let loaded = storage.load_messages(session_id).await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].content, "hello");
    }

    #[tokio::test]
    async fn test_meta_roundtrip() {
        let dir = tempdir().unwrap();
        let storage = SessionStorage::new(dir.path().to_path_buf());
        let session_id = Uuid::new_v4();
        let meta = SessionMeta {
            session_id,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            channel: "dingtalk".to_string(),
            scope: "user_123".to_string(),
            message_count: 0,
            backend_session_ids: Default::default(),
            session_status: SessionStatus::Idle,
        };
        storage.save_meta(&meta).await.unwrap();
        let loaded = storage.load_meta(session_id).await.unwrap().unwrap();
        assert_eq!(loaded.channel, "dingtalk");
        assert_eq!(loaded.scope, "user_123");
    }

    #[tokio::test]
    async fn test_empty_messages() {
        let dir = tempdir().unwrap();
        let storage = SessionStorage::new(dir.path().to_path_buf());
        let msgs = storage.load_messages(Uuid::new_v4()).await.unwrap();
        assert!(msgs.is_empty());
    }

    #[tokio::test]
    async fn test_load_recent_messages_when_session_dir_exists_but_jsonl_missing() {
        let dir = tempdir().unwrap();
        let storage = SessionStorage::new(dir.path().to_path_buf());
        let session_id = Uuid::new_v4();
        tokio::fs::create_dir_all(dir.path().join(session_id.to_string()))
            .await
            .unwrap();

        let recent = storage.load_recent_messages(session_id, 50).await.unwrap();
        assert!(recent.is_empty());
    }

    #[tokio::test]
    async fn test_stored_message_sender_roundtrip() {
        let dir = tempdir().unwrap();
        let storage = SessionStorage::new(dir.path().to_path_buf());
        let session_id = Uuid::new_v4();
        let msg = StoredMessage {
            id: Uuid::new_v4(),
            role: "assistant".to_string(),
            content: "I reviewed your code.".to_string(),
            timestamp: Utc::now(),
            sender: Some("@claude".to_string()),
            tool_calls: None,
            fragment_event_ids: None,
            aggregation_mode: None,
        };
        storage.append_message(session_id, &msg).await.unwrap();
        let loaded = storage.load_messages(session_id).await.unwrap();
        assert_eq!(loaded[0].sender.as_deref(), Some("@claude"));
    }

    #[tokio::test]
    async fn test_load_recent_messages_respects_limit() {
        let dir = tempdir().unwrap();
        let storage = SessionStorage::new(dir.path().to_path_buf());
        let session_id = Uuid::new_v4();

        // Append 10 messages
        for i in 0..10u32 {
            let msg = StoredMessage {
                id: Uuid::new_v4(),
                role: "user".to_string(),
                content: format!("msg-{i}"),
                timestamp: Utc::now(),
                sender: None,
                tool_calls: None,
                fragment_event_ids: None,
                aggregation_mode: None,
            };
            storage.append_message(session_id, &msg).await.unwrap();
        }

        // load_recent_messages(5) should return only the last 5
        let recent = storage.load_recent_messages(session_id, 5).await.unwrap();
        assert_eq!(recent.len(), 5);
        assert_eq!(recent[0].content, "msg-5");
        assert_eq!(recent[4].content, "msg-9");

        // limit > total: should return all
        let all = storage.load_recent_messages(session_id, 100).await.unwrap();
        assert_eq!(all.len(), 10);
    }

    #[test]
    fn test_stored_message_sender_optional_skip() {
        // sender=None should not appear in JSON (backward compat)
        let msg = StoredMessage {
            id: Uuid::new_v4(),
            role: "user".to_string(),
            content: "hello".to_string(),
            timestamp: Utc::now(),
            sender: None,
            tool_calls: None,
            fragment_event_ids: None,
            aggregation_mode: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            !json.contains("sender"),
            "sender=None should not appear in JSON"
        );
    }

    #[test]
    fn tool_call_record_roundtrips_optional_identity_fields() {
        let record = ToolCallRecord {
            tool_call_id: Some("call-1".into()),
            name: "read_file".into(),
            input: serde_json::json!({"path":"README.md"}),
            output: Some("ok".into()),
        };

        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("\"tool_call_id\":\"call-1\""));

        let roundtrip: ToolCallRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip.tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(roundtrip.name, "read_file");
        assert_eq!(roundtrip.output.as_deref(), Some("ok"));
    }

    #[tokio::test]
    async fn stored_message_fragment_refs_roundtrip() {
        let dir = tempdir().unwrap();
        let storage = SessionStorage::new(dir.path().to_path_buf());
        let session_id = Uuid::new_v4();
        let msg = StoredMessage {
            id: Uuid::new_v4(),
            role: "assistant".to_string(),
            content: "internal turn".to_string(),
            timestamp: Utc::now(),
            sender: Some("@alpha".to_string()),
            tool_calls: None,
            fragment_event_ids: Some(vec!["frag-1".to_string(), "frag-2".to_string()]),
            aggregation_mode: Some("turn_compacted".to_string()),
        };
        storage.append_message(session_id, &msg).await.unwrap();
        let loaded = storage.load_messages(session_id).await.unwrap();
        assert_eq!(
            loaded[0].fragment_event_ids.as_ref().unwrap(),
            &vec!["frag-1".to_string(), "frag-2".to_string()]
        );
        assert_eq!(
            loaded[0].aggregation_mode.as_deref(),
            Some("turn_compacted")
        );
    }

    #[test]
    fn session_meta_old_json_deserializes_with_defaults() {
        // Old metadata.json without backend_session_ids / session_status
        let old_json = r#"{
            "session_id": "00000000-0000-0000-0000-000000000001",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "channel": "ws",
            "scope": "u1",
            "message_count": 5
        }"#;
        let meta: SessionMeta = serde_json::from_str(old_json).unwrap();
        assert!(meta.backend_session_ids.is_empty());
        assert_eq!(meta.session_status, SessionStatus::Idle);
    }

    #[test]
    fn session_status_running_roundtrips() {
        let status = SessionStatus::Running {
            backend_id: "acp-claude".into(),
            started_at: chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        };
        let json = serde_json::to_string(&status).unwrap();
        let roundtrip: SessionStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(status, roundtrip);
    }
}
