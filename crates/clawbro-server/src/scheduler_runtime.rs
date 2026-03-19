use crate::agent_core::{memory::MemorySystem, MemoryEvent, SessionRegistry, TurnExecutionContext};
use crate::channel_registry::ChannelRegistry;
use crate::config;
use crate::delivery_resolver::resolve_delivery;
use crate::protocol::{parse_session_key_text, InboundMsg, MsgContent, MsgSource, SessionKey};
use crate::scheduler::{
    DeliveryMessageTarget, ExecutionFn, ExecutionOutcome, ExecutionPrecondition, RunStatus,
    ScheduledJob, ScheduledTarget, Scheduler, SchedulerConfig, SchedulerService, SchedulerStore,
    StoreConfig,
};
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;

pub fn resolve_scheduler_db_path(cfg: &config::GatewayConfig) -> PathBuf {
    cfg.scheduler.db_path.clone().unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".clawbro")
            .join("scheduler.db")
    })
}

pub async fn build_scheduler_service(
    cfg: &config::GatewayConfig,
) -> Result<(Arc<SchedulerService>, PathBuf)> {
    let db_path = resolve_scheduler_db_path(cfg);
    if let Some(parent) = db_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let store = Arc::new(SchedulerStore::open(
        &db_path,
        StoreConfig {
            default_timezone: cfg.scheduler.default_timezone.clone(),
        },
    )?);
    Ok((Arc::new(SchedulerService::new(store)), db_path))
}

pub fn build_test_scheduler_service() -> Arc<SchedulerService> {
    Arc::new(SchedulerService::new(Arc::new(
        SchedulerStore::in_memory().expect("in-memory scheduler store"),
    )))
}

pub fn spawn_scheduler_runtime(
    service: Arc<SchedulerService>,
    registry: Arc<SessionRegistry>,
    memory: Option<Arc<MemorySystem>>,
    channels: Arc<ChannelRegistry>,
    cfg: Arc<config::GatewayConfig>,
) {
    if !cfg.scheduler.enabled {
        tracing::info!("Scheduler runtime disabled by config");
        return;
    }

    let cfg_for_exec = cfg.clone();
    let executor: ExecutionFn = Arc::new(move |job: ScheduledJob| {
        let registry = registry.clone();
        let memory = memory.clone();
        let channels = channels.clone();
        let cfg = cfg_for_exec.clone();
        Box::pin(async move { execute_agent_turn(job, registry, memory, channels, cfg).await })
    });

    let scheduler = Scheduler::new(
        (*service).clone(),
        executor,
        SchedulerConfig {
            poll_interval: std::time::Duration::from_secs(cfg.scheduler.poll_secs.max(1)),
            max_fetch_per_tick: cfg.scheduler.max_fetch_per_tick.max(1),
            max_concurrent: cfg.scheduler.max_concurrent.max(1),
            lease_secs: cfg.scheduler.lease_secs.max(1),
        },
    );
    tokio::spawn(async move { scheduler.run().await });
}

async fn execute_agent_turn(
    job: ScheduledJob,
    registry: Arc<SessionRegistry>,
    memory: Option<Arc<MemorySystem>>,
    channels: Arc<ChannelRegistry>,
    cfg: Arc<config::GatewayConfig>,
) -> Result<ExecutionOutcome> {
    match &job.target {
        ScheduledTarget::AgentTurn(target) => {
            for precondition in &target.preconditions {
                match precondition {
                    ExecutionPrecondition::IdleGtSeconds { threshold_seconds } => {
                        let idle = registry
                            .session_idle_seconds(&target.session_key)
                            .unwrap_or(0);
                        if idle < *threshold_seconds {
                            return Ok(ExecutionOutcome {
                                status: RunStatus::Skipped,
                                summary: Some(format!(
                                    "skipped: idle {idle}s < threshold {threshold_seconds}s"
                                )),
                                error: None,
                            });
                        }
                    }
                }
            }

            let session_key = parse_scheduler_session_key(&target.session_key);
            let msg = InboundMsg {
                id: uuid::Uuid::new_v4().to_string(),
                session_key: session_key.clone(),
                content: MsgContent::text(render_agent_turn_prompt(target.prompt.as_str())),
                sender: "scheduler".to_string(),
                channel: "scheduler".to_string(),
                timestamp: chrono::Utc::now(),
                thread_ts: None,
                target_agent: target.agent.clone(),
                source: MsgSource::Cron,
            };

            match registry
                .handle_with_context(msg, TurnExecutionContext::default())
                .await
            {
                Ok(Some(result)) => {
                    deliver_scheduler_output(
                        cfg.as_ref(),
                        channels.as_ref(),
                        &session_key,
                        &result,
                    )
                    .await;
                    if let Some(memory) = &memory {
                        let summary: String = result.chars().take(300).collect();
                        memory.emit(MemoryEvent::CronJobCompleted {
                            scope: session_key,
                            agent: "scheduler".to_string(),
                            persona_dir: std::path::PathBuf::new(),
                            result_summary: summary,
                        });
                    }
                    Ok(ExecutionOutcome::succeeded(result))
                }
                Ok(None) => Ok(ExecutionOutcome {
                    status: RunStatus::Succeeded,
                    summary: Some("completed with no textual reply".to_string()),
                    error: None,
                }),
                Err(err) => Err(err),
            }
        }
        ScheduledTarget::DeliveryMessage(target) => {
            execute_delivery_message(target, channels, cfg).await
        }
    }
}

async fn execute_delivery_message(
    target: &DeliveryMessageTarget,
    channels: Arc<ChannelRegistry>,
    cfg: Arc<config::GatewayConfig>,
) -> Result<ExecutionOutcome> {
    let session_key = parse_scheduler_session_key(&target.session_key);
    deliver_scheduler_output(
        cfg.as_ref(),
        channels.as_ref(),
        &session_key,
        target.message.as_str(),
    )
    .await;
    Ok(ExecutionOutcome::succeeded(target.message.clone()))
}

async fn deliver_scheduler_output(
    cfg: &config::GatewayConfig,
    channels: &ChannelRegistry,
    session_key: &SessionKey,
    text: &str,
) {
    if let Some(resolved) = resolve_delivery(
        cfg,
        channels,
        config::DeliveryPurposeConfig::Cron,
        session_key,
        None,
        None,
        None,
        None,
        None,
    ) {
        let outbound = resolved.outbound_text(text);
        if let Err(err) = resolved.sender.send(&outbound).await {
            tracing::error!("scheduler output send failed: {err}");
        }
    } else {
        tracing::warn!(session = ?session_key, "scheduler could not resolve delivery target");
    }
}

fn render_agent_turn_prompt(task_prompt: &str) -> String {
    format!(
        "This is a scheduled task executing now. Do not reinterpret it as a new scheduling request. Do not ask whether to schedule it. Execute it now and reply with the result for the target conversation.\n\nScheduled task:\n{}",
        task_prompt.trim()
    )
}

fn parse_scheduler_session_key(raw: &str) -> SessionKey {
    if raw.contains(':') {
        parse_session_key_text(raw).unwrap_or_else(|_| SessionKey::new("scheduler", raw))
    } else {
        SessionKey::new("scheduler", raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel_registry::ChannelRegistry;
    use crate::channels_internal::Channel;
    use crate::protocol::{InboundMsg, MsgContent, OutboundMsg};
    use anyhow::Result;
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc;

    #[derive(Default)]
    struct RecordingChannel {
        sent: Mutex<Vec<OutboundMsg>>,
    }

    #[async_trait]
    impl Channel for RecordingChannel {
        fn name(&self) -> &str {
            "recording"
        }

        async fn send(&self, msg: &OutboundMsg) -> Result<()> {
            self.sent.lock().unwrap().push(msg.clone());
            Ok(())
        }

        async fn listen(&self, _tx: mpsc::Sender<InboundMsg>) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn delivery_message_target_sends_directly_to_real_channel() {
        let channel = Arc::new(RecordingChannel::default());
        let mut registry = ChannelRegistry::new();
        registry.register("dingtalk", Option::<String>::None, channel.clone(), true);
        deliver_scheduler_output(
            &config::GatewayConfig::default(),
            &registry,
            &SessionKey::new("dingtalk", "user:alice"),
            "刷牙时间到了",
        )
        .await;

        let sent = channel.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].session_key.channel, "dingtalk");
        assert_eq!(sent[0].session_key.scope, "user:alice");
        match &sent[0].content {
            MsgContent::Text { text } => assert_eq!(text, "刷牙时间到了"),
            other => panic!("expected text content, got {other:?}"),
        }
    }

    #[test]
    fn agent_turn_prompt_is_marked_as_scheduled_execution() {
        let rendered = render_agent_turn_prompt("每天 18 点总结 issue 进展");
        assert!(rendered.contains("scheduled task executing now"));
        assert!(rendered.contains("Do not reinterpret it as a new scheduling request"));
        assert!(rendered.contains("每天 18 点总结 issue 进展"));
    }
}
