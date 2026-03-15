use crate::key::{key_to_session_id, SessionId};
use crate::storage::{SessionMeta, SessionStatus, SessionStorage, StoredMessage};
use anyhow::Result;
use chrono::Utc;
use dashmap::DashMap;
use qai_protocol::SessionKey;
use std::sync::Arc;

pub struct SessionManager {
    storage: Arc<SessionStorage>,
    /// 内存缓存: SessionKey → SessionId
    active: DashMap<SessionKey, SessionId>,
}

impl SessionManager {
    pub fn new(storage: SessionStorage) -> Self {
        Self {
            storage: Arc::new(storage),
            active: DashMap::new(),
        }
    }

    /// 获取或创建 Session（幂等，基于 UUID v5 确定性 ID）
    pub async fn get_or_create(&self, key: &SessionKey) -> Result<SessionId> {
        if let Some(id) = self.active.get(key) {
            return Ok(*id);
        }
        let session_id = key_to_session_id(key);
        if self.storage.load_meta(session_id).await?.is_none() {
            let meta = SessionMeta {
                session_id,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                channel: key.channel.clone(),
                scope: key.scope.clone(),
                message_count: 0,
                backend_session_ids: Default::default(),
                session_status: SessionStatus::Idle,
            };
            self.storage.save_meta(&meta).await?;
        }
        self.active.insert(key.clone(), session_id);
        Ok(session_id)
    }

    pub async fn append_message(&self, session_id: SessionId, msg: &StoredMessage) -> Result<()> {
        self.storage.append_message(session_id, msg).await
    }

    pub fn storage(&self) -> Arc<SessionStorage> {
        self.storage.clone()
    }

    /// 读取指定 session 的 meta（用于获取 backend_session_ids）
    pub async fn load_meta(&self, session_id: SessionId) -> Result<Option<SessionMeta>> {
        self.storage.load_meta(session_id).await
    }

    /// 覆盖写 meta（原子 tmp→rename）
    pub async fn save_meta(&self, meta: &SessionMeta) -> Result<()> {
        self.storage.save_meta(meta).await
    }

    /// turn 开始时调用：将 session_status 设为 Running。
    /// 写入失败只记录警告，不影响 turn 执行。
    pub async fn begin_turn(&self, session_id: SessionId, backend_id: &str) -> Result<()> {
        let mut meta = self
            .storage
            .load_meta(session_id)
            .await?
            .unwrap_or_else(|| SessionMeta {
                session_id,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                channel: String::new(),
                scope: String::new(),
                message_count: 0,
                backend_session_ids: Default::default(),
                session_status: SessionStatus::Idle,
            });
        meta.session_status = SessionStatus::Running {
            backend_id: backend_id.to_string(),
            started_at: Utc::now(),
        };
        meta.updated_at = Utc::now();
        self.storage.save_meta(&meta).await
    }

    /// turn 完成时调用：更新 backend_session_id，重置 session_status 为 Idle。
    /// emitted_session_id=Some(id) 时持久化新 ACP session ID；
    /// None 时不更新（resume 路径：prior_id 未变）。
    pub async fn complete_turn(
        &self,
        session_id: SessionId,
        backend_id: &str,
        emitted_session_id: Option<String>,
    ) -> Result<()> {
        let mut meta = self
            .storage
            .load_meta(session_id)
            .await?
            .unwrap_or_else(|| SessionMeta {
                session_id,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                channel: String::new(),
                scope: String::new(),
                message_count: 0,
                backend_session_ids: Default::default(),
                session_status: SessionStatus::Idle,
            });
        if let Some(sid) = emitted_session_id {
            meta.backend_session_ids.insert(backend_id.to_string(), sid);
        }
        meta.session_status = SessionStatus::Idle;
        meta.updated_at = Utc::now();
        self.storage.save_meta(&meta).await
    }

    /// 重置会话的 conversation state：清除消息记录、清空所有 backend_session_ids、
    /// 重置 message_count 为 0、重置 session_status 为 Idle。
    ///
    /// 不清除 workspace、shared memory、agent memory。
    /// 对应 /reset 命令的正确语义：清当前对话，不清长期记忆。
    pub async fn reset_conversation(&self, session_id: SessionId) -> Result<()> {
        // 1. 清除宿主侧消息文件
        self.storage.clear_messages(session_id).await?;
        // 2. 清除 metadata 里的 backend_session_ids 和 message_count
        let mut meta = self
            .storage
            .load_meta(session_id)
            .await?
            .unwrap_or_else(|| SessionMeta {
                session_id,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                channel: String::new(),
                scope: String::new(),
                message_count: 0,
                backend_session_ids: Default::default(),
                session_status: SessionStatus::Idle,
            });
        meta.backend_session_ids.clear();
        meta.message_count = 0;
        meta.session_status = SessionStatus::Idle;
        meta.updated_at = Utc::now();
        self.storage.save_meta(&meta).await
    }

    /// 启动时扫描：找出所有 session_status=Running 的 session，重置为 Idle。
    /// 保留 backend_session_ids 不变（下次 turn 仍可尝试 resume）。
    /// 返回恢复的 session_id 列表（用于日志）。
    pub async fn recover_stuck_sessions(&self) -> Result<Vec<SessionId>> {
        let base_dir = self.storage.base_dir().to_path_buf();
        let mut recovered = Vec::new();

        let mut entries = match tokio::fs::read_dir(&base_dir).await {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(recovered),
            Err(e) => return Err(e.into()),
        };

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let meta_path = path.join("metadata.json");
            if !meta_path.exists() {
                continue;
            }
            let json = match tokio::fs::read_to_string(&meta_path).await {
                Ok(j) => j,
                Err(e) => {
                    tracing::warn!(path = %meta_path.display(), error = %e, "skipping unreadable metadata.json during stuck session scan");
                    continue;
                }
            };
            let mut meta: SessionMeta = match serde_json::from_str(&json) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(path = %meta_path.display(), error = %e, "skipping malformed metadata.json during stuck session scan");
                    continue;
                }
            };
            if matches!(meta.session_status, SessionStatus::Running { .. }) {
                meta.session_status = SessionStatus::Idle;
                meta.updated_at = Utc::now();
                if let Err(e) = self.storage.save_meta(&meta).await {
                    tracing::warn!(
                        session_id = %meta.session_id,
                        "Failed to reset stuck session: {e}"
                    );
                    continue;
                }
                recovered.push(meta.session_id);
            }
        }
        Ok(recovered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qai_protocol::SessionKey;

    fn make_manager() -> (SessionManager, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        let storage = SessionStorage::new(dir.path().to_path_buf());
        (SessionManager::new(storage), dir)
    }

    #[tokio::test]
    async fn begin_turn_sets_running_status() {
        let (mgr, _dir) = make_manager();
        let key = SessionKey::new("ws", "user:test");
        let session_id = mgr.get_or_create(&key).await.unwrap();

        mgr.begin_turn(session_id, "claude-main").await.unwrap();

        let meta = mgr.load_meta(session_id).await.unwrap().unwrap();
        assert!(
            matches!(meta.session_status, SessionStatus::Running { ref backend_id, .. } if backend_id == "claude-main"),
            "expected Running after begin_turn"
        );
    }

    #[tokio::test]
    async fn complete_turn_resets_to_idle_and_stores_session_id() {
        let (mgr, _dir) = make_manager();
        let key = SessionKey::new("ws", "user:test");
        let session_id = mgr.get_or_create(&key).await.unwrap();

        mgr.begin_turn(session_id, "claude-main").await.unwrap();
        mgr.complete_turn(session_id, "claude-main", Some("acp-sess-abc123".into()))
            .await
            .unwrap();

        let meta = mgr.load_meta(session_id).await.unwrap().unwrap();
        assert_eq!(
            meta.session_status,
            SessionStatus::Idle,
            "expected Idle after complete_turn"
        );
        assert_eq!(
            meta.backend_session_ids
                .get("claude-main")
                .map(String::as_str),
            Some("acp-sess-abc123"),
            "expected ACP session ID stored"
        );
    }

    #[tokio::test]
    async fn complete_turn_with_none_preserves_existing_session_id() {
        let (mgr, _dir) = make_manager();
        let key = SessionKey::new("ws", "user:test");
        let session_id = mgr.get_or_create(&key).await.unwrap();

        // First turn: store a session ID
        mgr.begin_turn(session_id, "claude-main").await.unwrap();
        mgr.complete_turn(session_id, "claude-main", Some("existing-id".into()))
            .await
            .unwrap();

        // Second turn: complete with None (e.g. load_session path or error path)
        mgr.begin_turn(session_id, "claude-main").await.unwrap();
        mgr.complete_turn(session_id, "claude-main", None)
            .await
            .unwrap();

        let meta = mgr.load_meta(session_id).await.unwrap().unwrap();
        assert_eq!(meta.session_status, SessionStatus::Idle);
        // Existing ID must not be overwritten by None
        assert_eq!(
            meta.backend_session_ids
                .get("claude-main")
                .map(String::as_str),
            Some("existing-id"),
            "None emitted_session_id must not clobber existing stored ID"
        );
    }

    #[tokio::test]
    async fn recover_stuck_sessions_resets_running_and_returns_ids() {
        let (mgr, _dir) = make_manager();
        let key1 = SessionKey::new("ws", "user:stuck1");
        let key2 = SessionKey::new("ws", "user:stuck2");
        let key3 = SessionKey::new("ws", "user:idle");

        let id1 = mgr.get_or_create(&key1).await.unwrap();
        let id2 = mgr.get_or_create(&key2).await.unwrap();
        let id3 = mgr.get_or_create(&key3).await.unwrap();

        mgr.begin_turn(id1, "claude-main").await.unwrap();
        mgr.begin_turn(id2, "codex-main").await.unwrap();
        // id3 stays Idle

        let recovered = mgr.recover_stuck_sessions().await.unwrap();

        assert_eq!(recovered.len(), 2, "should recover both stuck sessions");
        assert!(recovered.contains(&id1));
        assert!(recovered.contains(&id2));

        // All three sessions should now be Idle
        for id in [id1, id2, id3] {
            let meta = mgr.load_meta(id).await.unwrap().unwrap();
            assert_eq!(meta.session_status, SessionStatus::Idle);
        }
    }

    #[tokio::test]
    async fn reset_conversation_clears_messages_and_backend_ids() {
        let (mgr, _dir) = make_manager();
        let key = SessionKey::new("lark", "user:reset-test");
        let session_id = mgr.get_or_create(&key).await.unwrap();

        // Simulate a completed turn that stored a backend session ID
        mgr.begin_turn(session_id, "claude-main").await.unwrap();
        mgr.complete_turn(session_id, "claude-main", Some("old-acp-id".into()))
            .await
            .unwrap();
        // Also store a second backend to confirm all are cleared
        mgr.complete_turn(session_id, "codex", Some("old-codex-id".into()))
            .await
            .unwrap();

        // Append a message so we can verify clear_messages works
        mgr.append_message(
            session_id,
            &StoredMessage {
                id: uuid::Uuid::new_v4(),
                role: "user".to_string(),
                content: "hello".to_string(),
                timestamp: Utc::now(),
                sender: None,
                tool_calls: None,
                fragment_event_ids: None,
                aggregation_mode: None,
            },
        )
        .await
        .unwrap();

        mgr.reset_conversation(session_id).await.unwrap();

        let meta = mgr.load_meta(session_id).await.unwrap().unwrap();
        assert!(
            meta.backend_session_ids.is_empty(),
            "reset_conversation must clear all backend_session_ids"
        );
        assert_eq!(
            meta.message_count, 0,
            "reset_conversation must reset message_count to 0"
        );
        assert_eq!(
            meta.session_status,
            SessionStatus::Idle,
            "reset_conversation must set status to Idle"
        );
        // messages.jsonl should be gone
        let msgs = mgr.storage().load_messages(session_id).await.unwrap();
        assert!(msgs.is_empty(), "reset_conversation must clear messages");
    }

    #[tokio::test]
    async fn reset_conversation_does_not_affect_other_sessions() {
        let (mgr, _dir) = make_manager();
        let key_a = SessionKey::new("lark", "user:session-a");
        let key_b = SessionKey::new("lark", "user:session-b");
        let id_a = mgr.get_or_create(&key_a).await.unwrap();
        let id_b = mgr.get_or_create(&key_b).await.unwrap();

        mgr.complete_turn(id_a, "claude-main", Some("id-a".into()))
            .await
            .unwrap();
        mgr.complete_turn(id_b, "claude-main", Some("id-b".into()))
            .await
            .unwrap();

        mgr.reset_conversation(id_a).await.unwrap();

        // session_b must be untouched
        let meta_b = mgr.load_meta(id_b).await.unwrap().unwrap();
        assert_eq!(
            meta_b
                .backend_session_ids
                .get("claude-main")
                .map(String::as_str),
            Some("id-b"),
            "reset of session_a must not affect session_b backend_session_ids"
        );
    }

    #[tokio::test]
    async fn recover_stuck_sessions_preserves_backend_session_ids() {
        let (mgr, _dir) = make_manager();
        let key = SessionKey::new("ws", "user:recover");
        let session_id = mgr.get_or_create(&key).await.unwrap();

        mgr.begin_turn(session_id, "claude-main").await.unwrap();
        // Simulate crash: store backend_session_id then leave Running
        mgr.complete_turn(session_id, "claude-main", Some("prior-acp-id".into()))
            .await
            .unwrap();
        mgr.begin_turn(session_id, "claude-main").await.unwrap();
        // Now it's stuck Running with a prior ACP session ID stored

        mgr.recover_stuck_sessions().await.unwrap();

        let meta = mgr.load_meta(session_id).await.unwrap().unwrap();
        assert_eq!(meta.session_status, SessionStatus::Idle);
        // backend_session_ids must survive recovery so resume still works on next turn
        assert_eq!(
            meta.backend_session_ids
                .get("claude-main")
                .map(String::as_str),
            Some("prior-acp-id"),
            "backend_session_ids must not be cleared by recover_stuck_sessions"
        );
    }
}
