//! TeamMilestoneEvent — 类型化 Agent Swarm 里程碑事件
//!
//! 设计原则：
//!   - 事件本身不包含任何展示逻辑（emoji/文案/语言）
//!   - IM 渲染集中在 render_for_im()，与事件语义完全解耦
//!   - 测试断言仅需 matches! 枚举变体 + 字段值，不涉及字符串
//!
//! 调用链：
//!   orchestrator::emit_milestone(event)
//!       → MilestoneFn(scope, event)          ← 测试在此收集类型化事件
//!       → render_for_im(&event) → IM channel  ← 生产在此推送消息

use serde::{Deserialize, Serialize};

// ─── 事件类型 ─────────────────────────────────────────────────────────────────

/// 代表 Agent Swarm 生命周期中所有可观测的里程碑。
///
/// 每个变体携带足够断言语义正确性的结构化字段——
/// 测试不应依赖 render_for_im() 的输出。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TeamMilestoneEvent {
    /// Heartbeat 已将任务派发给 Specialist（仅在 dispatch 成功后触发）
    TaskDispatched {
        task_id: String,
        task_title: String,
        agent: String,
    },
    /// Specialist 报告中间进度
    TaskCheckpoint {
        task_id: String,
        agent: String,
        note: String,
    },
    /// Specialist 提交结果，等待 Lead 验收
    TaskSubmitted {
        task_id: String,
        task_title: String,
        agent: String,
    },
    /// Specialist 阻塞，无法继续
    TaskBlocked {
        task_id: String,
        task_title: String,
        agent: String,
        reason: String,
    },
    /// 单个任务完成（不是所有任务）
    TaskDone {
        task_id: String,
        task_title: String,
        agent: String,
        /// 当前已完成任务数（包含本次）
        done_count: usize,
        /// 任务总数
        total: usize,
    },
    /// 新任务因依赖已满足而解锁
    TasksUnlocked { task_ids: Vec<String> },
    /// 全部任务完成
    AllTasksDone,
    /// Lead 发布任意文字更新（通过 post_message / post_update 工具）
    LeadMessage { text: String },
}

impl TeamMilestoneEvent {
    /// 短标识符，用于日志和 tracing，不面向用户。
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::TaskDispatched { .. } => "task_dispatched",
            Self::TaskCheckpoint { .. } => "task_checkpoint",
            Self::TaskSubmitted { .. } => "task_submitted",
            Self::TaskBlocked { .. } => "task_blocked",
            Self::TaskDone { .. } => "task_done",
            Self::TasksUnlocked { .. } => "tasks_unlocked",
            Self::AllTasksDone => "all_tasks_done",
            Self::LeadMessage { .. } => "lead_message",
        }
    }
}

// ─── IM 渲染层 ────────────────────────────────────────────────────────────────

/// 将类型化事件渲染为面向人类的 IM 消息文本。
///
/// 与事件语义完全解耦：可在不修改事件定义的前提下更改措辞/emoji/语言。
/// **测试不应调用此函数**；断言应基于 `TeamMilestoneEvent` 枚举字段。
pub fn render_for_im(event: &TeamMilestoneEvent) -> String {
    match event {
        TeamMilestoneEvent::TaskDispatched {
            task_id,
            task_title,
            agent,
        } => format!("🚀 任务 **{task_id}**「{task_title}」已派发给 @{agent}"),

        TeamMilestoneEvent::TaskCheckpoint {
            task_id,
            agent,
            note,
        } => format!("📍 [{task_id}] @{agent} 进度：{note}"),

        TeamMilestoneEvent::TaskSubmitted {
            task_id,
            task_title,
            agent,
        } => format!("📨 任务 {task_id}「{task_title}」@{agent} 已提交待验收"),

        TeamMilestoneEvent::TaskBlocked {
            task_id,
            task_title,
            agent,
            reason,
        } => format!("🚧 任务 {task_id}「{task_title}」@{agent} 阻塞：{reason}"),

        TeamMilestoneEvent::TaskDone {
            task_id,
            task_title,
            agent,
            done_count,
            total,
        } => format!("✅ 任务 {task_id}「{task_title}」@{agent} 已完成（{done_count}/{total}）"),

        TeamMilestoneEvent::TasksUnlocked { task_ids } => {
            format!("🔓 新任务已解锁：{}", task_ids.join(", "))
        }

        TeamMilestoneEvent::AllTasksDone => "所有任务已完成 ✅".to_string(),

        TeamMilestoneEvent::LeadMessage { text } => text.clone(),
    }
}

// ─── 测试 ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kind_str_is_stable() {
        assert_eq!(
            TeamMilestoneEvent::AllTasksDone.kind_str(),
            "all_tasks_done"
        );
        assert_eq!(
            TeamMilestoneEvent::TaskDispatched {
                task_id: "T1".into(),
                task_title: "t".into(),
                agent: "a".into()
            }
            .kind_str(),
            "task_dispatched"
        );
    }

    #[test]
    fn test_render_does_not_affect_event_equality() {
        let ev = TeamMilestoneEvent::TaskDone {
            task_id: "T1".into(),
            task_title: "Setup DB".into(),
            agent: "codex".into(),
            done_count: 1,
            total: 3,
        };
        let rendered = render_for_im(&ev);
        // Rendering must not mutate the event — verify by cloning and comparing
        let ev2 = ev.clone();
        assert_eq!(ev, ev2, "event must be unchanged after render");
        // Rendered string contains structured data (not just checking emoji)
        assert!(rendered.contains("T1"), "rendered must contain task_id");
        assert!(rendered.contains("codex"), "rendered must contain agent");
        assert!(
            rendered.contains("1/3"),
            "rendered must contain progress ratio"
        );
    }

    #[test]
    fn test_serde_round_trip() {
        let ev = TeamMilestoneEvent::TaskBlocked {
            task_id: "T5".into(),
            task_title: "Deploy".into(),
            agent: "claude".into(),
            reason: "missing env var".into(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: TeamMilestoneEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }
}
