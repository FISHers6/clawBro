use anyhow::Result;
use async_trait::async_trait;
use dashmap::DashMap;
use clawbro_protocol::{render_scope_storage_key, SessionKey};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn scope_key(scope: &SessionKey) -> String {
    render_scope_storage_key(scope)
}

#[async_trait]
pub trait MemoryStore: Send + Sync {
    async fn load_shared_memory(&self, scope: &SessionKey) -> Result<String>;
    async fn load_agent_memory(&self, persona_dir: &Path, scope: &SessionKey) -> Result<String>;
    async fn append_shared(&self, scope: &SessionKey, content: &str) -> Result<()>;
    async fn overwrite_agent_memory(
        &self,
        persona_dir: &Path,
        scope: &SessionKey,
        content: &str,
    ) -> Result<()>;
    async fn append_daily_log(
        &self,
        persona_dir: &Path,
        scope: &SessionKey,
        entry: &str,
    ) -> Result<()>;
    async fn load_recent_logs(
        &self,
        persona_dir: &Path,
        scope: &SessionKey,
        days: u64,
    ) -> Result<String>;
    async fn append_to_agent_memory(
        &self,
        persona_dir: &Path,
        scope: &SessionKey,
        content: &str,
    ) -> Result<()>;
    async fn overwrite_shared(&self, scope: &SessionKey, content: &str) -> Result<()>;
    /// Returns the last-modified timestamp of the shared memory file, or None if not yet created.
    async fn shared_last_modified(
        &self,
        scope: &SessionKey,
    ) -> Result<Option<chrono::DateTime<chrono::Local>>>;
}

pub struct FileMemoryStore {
    shared_dir: PathBuf,
    write_locks: DashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>,
}

impl FileMemoryStore {
    pub fn new(shared_dir: PathBuf) -> Self {
        Self {
            shared_dir,
            write_locks: DashMap::new(),
        }
    }

    fn shared_path(&self, scope: &SessionKey) -> PathBuf {
        self.shared_dir
            .join("memory")
            .join(format!("{}.md", scope_key(scope)))
    }

    fn agent_path(&self, persona_dir: &Path, scope: &SessionKey) -> PathBuf {
        persona_dir
            .join("memory")
            .join(format!("{}.md", scope_key(scope)))
    }

    fn daily_log_path(&self, persona_dir: &Path, scope: &SessionKey) -> PathBuf {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        persona_dir
            .join("logs")
            .join(format!("{}_{}.md", scope_key(scope), date))
    }

    fn lock_for(&self, path: &Path) -> Arc<tokio::sync::Mutex<()>> {
        self.write_locks
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    async fn atomic_append(path: &Path, content: &str) -> Result<()> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        file.write_all(content.as_bytes()).await?;
        Ok(())
    }
}

#[async_trait]
impl MemoryStore for FileMemoryStore {
    async fn load_shared_memory(&self, scope: &SessionKey) -> Result<String> {
        let path = self.shared_path(scope);
        match tokio::fs::read_to_string(&path).await {
            Ok(s) => Ok(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(e.into()),
        }
    }

    async fn load_agent_memory(&self, persona_dir: &Path, scope: &SessionKey) -> Result<String> {
        let path = self.agent_path(persona_dir, scope);
        match tokio::fs::read_to_string(&path).await {
            Ok(s) => Ok(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(e.into()),
        }
    }

    async fn append_shared(&self, scope: &SessionKey, content: &str) -> Result<()> {
        let path = self.shared_path(scope);
        let lock = self.lock_for(&path);
        let _guard = lock.lock().await;
        Self::atomic_append(&path, &format!("{content}\n")).await
    }

    async fn overwrite_agent_memory(
        &self,
        persona_dir: &Path,
        scope: &SessionKey,
        content: &str,
    ) -> Result<()> {
        let path = self.agent_path(persona_dir, scope);
        let lock = self.lock_for(&path);
        let _guard = lock.lock().await;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, content).await?;
        Ok(())
    }

    async fn append_daily_log(
        &self,
        persona_dir: &Path,
        scope: &SessionKey,
        entry: &str,
    ) -> Result<()> {
        let path = self.daily_log_path(persona_dir, scope);
        let lock = self.lock_for(&path);
        let _guard = lock.lock().await;
        let now = chrono::Local::now().format("%H:%M").to_string();
        Self::atomic_append(&path, &format!("## {now}\n\n{entry}\n\n---\n")).await
    }

    async fn append_to_agent_memory(
        &self,
        persona_dir: &Path,
        scope: &SessionKey,
        content: &str,
    ) -> Result<()> {
        let path = self.agent_path(persona_dir, scope);
        let lock = self.lock_for(&path);
        let _guard = lock.lock().await;
        Self::atomic_append(&path, &format!("{content}\n")).await
    }

    async fn overwrite_shared(&self, scope: &SessionKey, content: &str) -> Result<()> {
        let path = self.shared_path(scope);
        let lock = self.lock_for(&path);
        let _guard = lock.lock().await;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, content).await?;
        Ok(())
    }

    async fn shared_last_modified(
        &self,
        scope: &SessionKey,
    ) -> Result<Option<chrono::DateTime<chrono::Local>>> {
        let path = self.shared_path(scope);
        match tokio::fs::metadata(&path).await {
            Ok(meta) => {
                let modified = meta.modified()?;
                let dt: chrono::DateTime<chrono::Local> = modified.into();
                Ok(Some(dt))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn load_recent_logs(
        &self,
        persona_dir: &Path,
        scope: &SessionKey,
        days: u64,
    ) -> Result<String> {
        let logs_dir = persona_dir.join("logs");
        let prefix = scope_key(scope);
        let cutoff_date = (chrono::Local::now() - chrono::Duration::days(days as i64)).date_naive();
        let mut out = String::new();

        let mut read_dir = match tokio::fs::read_dir(&logs_dir).await {
            Ok(d) => d,
            Err(_) => return Ok(out),
        };
        let mut entries = Vec::new();
        loop {
            match read_dir.next_entry().await {
                Ok(Some(e)) => {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.ends_with(".md") {
                        // filename: {scope_key}_{YYYY-MM-DD}.md
                        if let Some(date_str) = name
                            .strip_prefix(&format!("{prefix}_"))
                            .and_then(|s| s.strip_suffix(".md"))
                        {
                            if let Ok(date) =
                                chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
                            {
                                if date >= cutoff_date {
                                    entries.push((date, e.path()));
                                }
                            }
                        }
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!("Failed to read log dir entry: {e}");
                    continue;
                }
            }
        }
        entries.sort_by_key(|(d, _)| *d);
        for (_, path) in entries {
            match tokio::fs::read_to_string(&path).await {
                Ok(content) => out.push_str(&content),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!("Failed to read log file {:?}: {e}", path),
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawbro_protocol::SessionKey;
    use tempfile::tempdir;

    fn scope() -> SessionKey {
        SessionKey::new("dingtalk", "group_C123")
    }

    #[test]
    fn group_scope_storage_key_ignores_channel_instance() {
        let scope = SessionKey::with_instance("lark", "beta", "group:oc_1");
        assert_eq!(scope_key(&scope), "c=lark#s=group:oc_1");
    }

    #[test]
    fn dm_scope_storage_key_keeps_channel_instance() {
        let scope = SessionKey::with_instance("lark", "beta", "user:ou_1");
        assert_eq!(scope_key(&scope), "c=lark#i=beta#s=user:ou_1");
    }

    #[tokio::test]
    async fn test_load_shared_empty_when_missing() {
        let dir = tempdir().unwrap();
        let store = FileMemoryStore::new(dir.path().to_path_buf());
        let result = store.load_shared_memory(&scope()).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_append_then_load_shared() {
        let dir = tempdir().unwrap();
        let store = FileMemoryStore::new(dir.path().to_path_buf());
        store.append_shared(&scope(), "我们用 Go").await.unwrap();
        let content = store.load_shared_memory(&scope()).await.unwrap();
        assert!(content.contains("我们用 Go"));
    }

    #[tokio::test]
    async fn test_agent_memory_scope_isolated() {
        let dir = tempdir().unwrap();
        let store = FileMemoryStore::new(dir.path().to_path_buf());
        let persona_a = tempdir().unwrap();
        let persona_b = tempdir().unwrap();
        let scope_a = SessionKey::new("dingtalk", "group_A");
        let scope_b = SessionKey::new("dingtalk", "group_B");
        store
            .overwrite_agent_memory(persona_a.path(), &scope_a, "content-A")
            .await
            .unwrap();
        store
            .overwrite_agent_memory(persona_b.path(), &scope_b, "content-B")
            .await
            .unwrap();
        let a = store
            .load_agent_memory(persona_a.path(), &scope_a)
            .await
            .unwrap();
        let b = store
            .load_agent_memory(persona_b.path(), &scope_b)
            .await
            .unwrap();
        assert!(a.contains("content-A") && !a.contains("content-B"));
        assert!(b.contains("content-B") && !b.contains("content-A"));
    }

    #[tokio::test]
    async fn test_append_daily_log_creates_file() {
        let dir = tempdir().unwrap();
        let store = FileMemoryStore::new(dir.path().to_path_buf());
        let persona = tempdir().unwrap();
        store
            .append_daily_log(persona.path(), &scope(), "[user]: hello\n[bot]: hi")
            .await
            .unwrap();
        let logs = store
            .load_recent_logs(persona.path(), &scope(), 7)
            .await
            .unwrap();
        assert!(logs.contains("hello"));
    }

    #[tokio::test]
    async fn test_load_recent_logs_skips_old_files() {
        let dir = tempdir().unwrap();
        let store = FileMemoryStore::new(dir.path().to_path_buf());
        let persona = tempdir().unwrap();
        let scope = scope();
        let scope_key = render_scope_storage_key(&scope);
        let old_file = persona
            .path()
            .join("logs")
            .join(format!("{scope_key}_2020-01-01.md"));
        std::fs::create_dir_all(old_file.parent().unwrap()).unwrap();
        std::fs::write(&old_file, "old content").unwrap();
        let logs = store
            .load_recent_logs(persona.path(), &scope, 7)
            .await
            .unwrap();
        assert!(!logs.contains("old content"));
    }

    #[tokio::test]
    async fn test_shared_last_modified_none_when_missing() {
        let dir = tempdir().unwrap();
        let store = FileMemoryStore::new(dir.path().to_path_buf());
        let result = store.shared_last_modified(&scope()).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_shared_last_modified_some_after_write() {
        let dir = tempdir().unwrap();
        let store = FileMemoryStore::new(dir.path().to_path_buf());
        store.append_shared(&scope(), "something").await.unwrap();
        let result = store.shared_last_modified(&scope()).await.unwrap();
        assert!(result.is_some());
    }
}
