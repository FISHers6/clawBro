use crate::key::{key_to_session_id, SessionId};
use crate::storage::{SessionMeta, SessionStorage, StoredMessage};
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
}
