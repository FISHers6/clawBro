use crate::cli::args::{
    ScheduleAddAtArgs, ScheduleAddCronArgs, ScheduleAddDelayArgs, ScheduleAddEveryArgs,
    ScheduleArgs, ScheduleCommands, ScheduleDeleteAllArgs, ScheduleDeleteArgs,
    ScheduleHistoryArgs, ScheduleListArgs, ScheduleSessionFilterArgs, ScheduleTargetArgs,
    ScheduleTargetKindArg,
};
use crate::config::GatewayConfig;
use crate::protocol::parse_session_key_text;
use crate::scheduler_runtime;
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use clawbro_cron::{
    CreateJobRequest, CreateTargetRequest, ExecutionPrecondition, JobQuery,
    RequestedTargetKind, ScheduleInput, SessionTargetRequest, SourceKind,
};
use serde::Serialize;
use serde_json::{json, Value};

pub async fn run(args: ScheduleArgs) -> Result<()> {
    let json_mode = args.json;
    let op = schedule_op_name(&args.command);
    let result = match args.command {
        ScheduleCommands::AddCron(args) => add_cron(args).await,
        ScheduleCommands::AddAt(args) => add_at(args).await,
        ScheduleCommands::AddEvery(args) => add_every(args).await,
        ScheduleCommands::AddDelay(args) => add_delay(args).await,
        ScheduleCommands::List(args) => list_jobs(args).await,
        ScheduleCommands::Pause(args) => pause_job(&args.job_id).await,
        ScheduleCommands::Resume(args) => resume_job(&args.job_id).await,
        ScheduleCommands::Delete(args) => delete_jobs(args).await,
        ScheduleCommands::DeleteAll(args) => delete_all_jobs(args).await,
        ScheduleCommands::RunNow(args) => run_now_job(&args.job_id).await,
        ScheduleCommands::History(args) => history(args).await,
    };

    match (json_mode, result) {
        (false, Ok(out)) => {
            print_human_output(out);
            Ok(())
        }
        (true, Ok(out)) => {
            println!("{}", render_json_success(op, out.message, out.data)?);
            Ok(())
        }
        (true, Err(err)) => {
            println!("{}", render_json_failure(op, &err)?);
            Err(err)
        }
        (false, Err(err)) => Err(err),
    }
}

#[derive(Debug, Serialize)]
struct ScheduleJsonEnvelope {
    ok: bool,
    op: &'static str,
    message: String,
    data: Value,
}

impl ScheduleJsonEnvelope {
    fn success(op: &'static str, message: String, data: Value) -> Self {
        Self {
            ok: true,
            op,
            message,
            data,
        }
    }

    fn failure(op: &'static str, message: String) -> Self {
        Self {
            ok: false,
            op,
            message,
            data: Value::Null,
        }
    }
}

fn render_json_success(op: &'static str, message: String, data: Value) -> Result<String> {
    Ok(serde_json::to_string(&ScheduleJsonEnvelope::success(
        op, message, data,
    ))?)
}

fn render_json_failure(op: &'static str, err: &anyhow::Error) -> Result<String> {
    Ok(serde_json::to_string(&ScheduleJsonEnvelope::failure(
        op,
        err.to_string(),
    ))?)
}

struct ScheduleCommandOutput {
    human_lines: Vec<String>,
    message: String,
    data: Value,
}

fn print_human_output(out: ScheduleCommandOutput) {
    for line in out.human_lines {
        println!("{line}");
    }
}

fn schedule_op_name(command: &ScheduleCommands) -> &'static str {
    match command {
        ScheduleCommands::AddCron(_) => "add-cron",
        ScheduleCommands::AddAt(_) => "add-at",
        ScheduleCommands::AddEvery(_) => "add-every",
        ScheduleCommands::AddDelay(_) => "add-delay",
        ScheduleCommands::List(_) => "list",
        ScheduleCommands::Pause(_) => "pause",
        ScheduleCommands::Resume(_) => "resume",
        ScheduleCommands::Delete(_) => "delete",
        ScheduleCommands::DeleteAll(_) => "delete-all",
        ScheduleCommands::RunNow(_) => "run-now",
        ScheduleCommands::History(_) => "history",
    }
}

async fn add_cron(args: ScheduleAddCronArgs) -> Result<ScheduleCommandOutput> {
    let req = build_request(args.target, ScheduleInput::Cron { expr: args.expr })?;
    let service = load_scheduler_service().await?;
    let job = service.create_job(req, Utc::now())?;
    Ok(ScheduleCommandOutput {
        human_lines: vec![format!("created schedule {} ({})", job.name, job.id)],
        message: format!("created schedule {}", job.id),
        data: json!({ "job": job }),
    })
}

async fn add_at(args: ScheduleAddAtArgs) -> Result<ScheduleCommandOutput> {
    let run_at = DateTime::parse_from_rfc3339(&args.at)
        .with_context(|| format!("invalid --at '{}'; expected RFC3339 timestamp", args.at))?
        .with_timezone(&Utc);
    let req = build_request(args.target, ScheduleInput::At { run_at })?;
    let service = load_scheduler_service().await?;
    let job = service.create_job(req, Utc::now())?;
    Ok(ScheduleCommandOutput {
        human_lines: vec![format!("created schedule {} ({})", job.name, job.id)],
        message: format!("created schedule {}", job.id),
        data: json!({ "job": job }),
    })
}

async fn add_every(args: ScheduleAddEveryArgs) -> Result<ScheduleCommandOutput> {
    let every_ms = parse_duration_millis(&args.every)?;
    let req = build_request(
        args.target,
        ScheduleInput::Every {
            interval_ms: every_ms,
        },
    )?;
    let service = load_scheduler_service().await?;
    let job = service.create_job(req, Utc::now())?;
    Ok(ScheduleCommandOutput {
        human_lines: vec![format!("created schedule {} ({})", job.name, job.id)],
        message: format!("created schedule {}", job.id),
        data: json!({ "job": job }),
    })
}

async fn add_delay(args: ScheduleAddDelayArgs) -> Result<ScheduleCommandOutput> {
    let delay_ms = parse_duration_millis(&args.delay)?;
    let req = build_request(args.target, ScheduleInput::Delay { delay_ms })?;
    let service = load_scheduler_service().await?;
    let job = service.create_job(req, Utc::now())?;
    Ok(ScheduleCommandOutput {
        human_lines: vec![format!("created schedule {} ({})", job.name, job.id)],
        message: format!("created schedule {}", job.id),
        data: json!({ "job": job }),
    })
}

async fn list_jobs(args: ScheduleListArgs) -> Result<ScheduleCommandOutput> {
    let service = load_scheduler_service().await?;
    let jobs = service.list_jobs_matching(&build_job_query(
        args.name,
        args.name_contains,
        args.session,
    )?)?;
    let human_lines = jobs
        .iter()
        .map(|job| {
            format!(
                "{}\t{}\t{:?}\tenabled={}\tnext_run_at={}",
                job.id,
                job.name,
                job.schedule.kind(),
                job.enabled,
                job.next_run_at
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_else(|| "-".to_string())
            )
        })
        .collect();
    Ok(ScheduleCommandOutput {
        human_lines,
        message: format!("listed {} schedule(s)", jobs.len()),
        data: json!({ "jobs": jobs }),
    })
}

async fn pause_job(job_id: &str) -> Result<ScheduleCommandOutput> {
    let service = load_scheduler_service().await?;
    if service.pause_job(job_id, Utc::now())? {
        Ok(ScheduleCommandOutput {
            human_lines: vec![format!("paused {job_id}")],
            message: format!("paused {job_id}"),
            data: json!({ "job_id": job_id }),
        })
    } else {
        bail!("job not found: {job_id}")
    }
}

async fn resume_job(job_id: &str) -> Result<ScheduleCommandOutput> {
    let service = load_scheduler_service().await?;
    if service.resume_job(job_id, Utc::now())? {
        Ok(ScheduleCommandOutput {
            human_lines: vec![format!("resumed {job_id}")],
            message: format!("resumed {job_id}"),
            data: json!({ "job_id": job_id }),
        })
    } else {
        bail!("job not found: {job_id}")
    }
}

async fn delete_job(job_id: &str) -> Result<ScheduleCommandOutput> {
    let service = load_scheduler_service().await?;
    if service.delete_job(job_id)? {
        Ok(ScheduleCommandOutput {
            human_lines: vec![format!("deleted {job_id}")],
            message: format!("deleted {job_id}"),
            data: json!({ "job_id": job_id }),
        })
    } else {
        bail!("job not found: {job_id}")
    }
}

async fn delete_jobs(args: ScheduleDeleteArgs) -> Result<ScheduleCommandOutput> {
    if let Some(job_id) = args.job_id.as_deref() {
        return delete_job(job_id).await;
    }
    let query = build_job_query(args.name, args.name_contains, args.session)?;
    let service = load_scheduler_service().await?;
    let matches = service.list_jobs_matching(&query)?;
    if matches.is_empty() {
        bail!("no jobs matched the requested delete filter");
    }
    if matches.len() > 1 && !args.all_matches {
        let names = matches
            .iter()
            .map(|job| format!("{} ({})", job.name, job.id))
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "delete filter matched {} jobs: {}; rerun with --all-matches to delete them all",
            matches.len(),
            names
        );
    }
    let deleted = service.delete_jobs_matching(&query)?;
    Ok(ScheduleCommandOutput {
        human_lines: deleted
            .iter()
            .map(|job| format!("deleted {} ({})", job.name, job.id))
            .collect(),
        message: format!("deleted {} schedule(s)", deleted.len()),
        data: json!({
            "count": deleted.len(),
            "jobs": deleted,
        }),
    })
}

async fn delete_all_jobs(args: ScheduleDeleteAllArgs) -> Result<ScheduleCommandOutput> {
    let query = build_job_query(None, None, args.session)?;
    let service = load_scheduler_service().await?;
    let deleted = service.delete_jobs_matching(&query)?;
    Ok(ScheduleCommandOutput {
        human_lines: vec![format!("deleted {} schedule(s)", deleted.len())],
        message: format!("deleted {} schedule(s)", deleted.len()),
        data: json!({
            "count": deleted.len(),
            "jobs": deleted,
        }),
    })
}

async fn run_now_job(job_id: &str) -> Result<ScheduleCommandOutput> {
    let service = load_scheduler_service().await?;
    if service.request_run_now(job_id, Utc::now())? {
        Ok(ScheduleCommandOutput {
            human_lines: vec![format!("requested run-now for {job_id}")],
            message: format!("requested run-now for {job_id}"),
            data: json!({ "job_id": job_id }),
        })
    } else {
        bail!("job not found: {job_id}")
    }
}

async fn history(args: ScheduleHistoryArgs) -> Result<ScheduleCommandOutput> {
    let service = load_scheduler_service().await?;
    let runs = service.list_run_history(args.job_id.as_deref())?;
    let human_lines = runs
        .iter()
        .map(|run| {
            format!(
                "{}\t{}\t{:?}\tscheduled_at={}\tfinished_at={}",
                run.id,
                run.job_id,
                run.status,
                run.scheduled_at.to_rfc3339(),
                run.finished_at
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_else(|| "-".to_string())
            )
        })
        .collect();
    Ok(ScheduleCommandOutput {
        human_lines,
        message: format!("listed {} run(s)", runs.len()),
        data: json!({ "runs": runs }),
    })
}

fn build_job_query(
    name: Option<String>,
    name_contains: Option<String>,
    session: ScheduleSessionFilterArgs,
) -> Result<JobQuery> {
    let session_key = session
        .session_key
        .or(session.current_session_key)
        .map(|value| {
            parse_session_key_text(&value)
                .map(|_| value)
                .map_err(|err| anyhow::anyhow!("invalid target session key: {err}"))
        })
        .transpose()?;
    Ok(JobQuery {
        name,
        name_contains,
        session_key,
    })
}

fn build_request(target: ScheduleTargetArgs, schedule: ScheduleInput) -> Result<CreateJobRequest> {
    let session_key = target
        .session_key
        .or(target.current_session_key)
        .context("schedule target requires --session-key or current session context")?;
    parse_session_key_text(&session_key)
        .map_err(|err| anyhow::anyhow!("invalid target session key: {err}"))?;
    let mut preconditions = Vec::new();
    if let Some(threshold) = target.idle_gt_seconds {
        preconditions.push(ExecutionPrecondition::IdleGtSeconds {
            threshold_seconds: threshold,
        });
    }
    Ok(CreateJobRequest {
        name: target.name,
        schedule,
        timezone: target.timezone,
        target: CreateTargetRequest::Session(SessionTargetRequest {
            requested_kind: match target.target_kind {
                ScheduleTargetKindArg::Auto => RequestedTargetKind::Auto,
                ScheduleTargetKindArg::AgentTurn => RequestedTargetKind::AgentTurn,
                ScheduleTargetKindArg::DeliveryMessage => RequestedTargetKind::DeliveryMessage,
            },
            session_key,
            prompt: target.prompt,
            agent: target.agent,
            preconditions,
        }),
        max_retries: 0,
        source_kind: SourceKind::HumanCli,
        source_actor: std::env::var("USER").unwrap_or_else(|_| "clawbro".to_string()),
        source_session_key: None,
        created_via: "cli".to_string(),
        requested_by_role: Some("user".to_string()),
    })
}

async fn load_scheduler_service() -> Result<std::sync::Arc<clawbro_cron::SchedulerService>> {
    let cfg = GatewayConfig::load()?;
    let (service, _) = scheduler_runtime::build_scheduler_service(&cfg).await?;
    Ok(service)
}

fn parse_duration_millis(raw: &str) -> Result<i64> {
    let (number, unit) = split_duration(raw)?;
    let base: i64 = number
        .parse()
        .with_context(|| format!("invalid duration '{}'", raw))?;
    let factor = match unit {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => bail!("unsupported duration suffix '{}'", unit),
    };
    Ok(base * factor)
}

fn split_duration(raw: &str) -> Result<(&str, &str)> {
    let trimmed = raw.trim();
    let split_at = trimmed
        .find(|ch: char| !ch.is_ascii_digit())
        .context("duration must end with ms/s/m/h/d")?;
    let (number, unit) = trimmed.split_at(split_at);
    if number.is_empty() || unit.is_empty() {
        bail!("duration must look like 30s, 5m, 2h, 1d, or 500ms");
    }
    Ok((number, unit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use serde_json::Value;

    #[test]
    fn duration_parser_supports_common_suffixes() {
        assert_eq!(parse_duration_millis("500ms").unwrap(), 500);
        assert_eq!(parse_duration_millis("30s").unwrap(), 30_000);
        assert_eq!(parse_duration_millis("5m").unwrap(), 300_000);
        assert_eq!(parse_duration_millis("2h").unwrap(), 7_200_000);
    }

    #[test]
    fn json_failure_output_is_machine_parseable() {
        let rendered = render_json_failure("pause", &anyhow!("job not found: test-job")).unwrap();
        let value: Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value.get("ok").and_then(Value::as_bool), Some(false));
        assert_eq!(value.get("op").and_then(Value::as_str), Some("pause"));
        assert_eq!(
            value.get("message").and_then(Value::as_str),
            Some("job not found: test-job")
        );
        assert!(value.get("data").unwrap().is_null());
    }

    #[test]
    fn build_request_prefers_explicit_session_key_over_current_session() {
        let req = build_request(
            ScheduleTargetArgs {
                name: "test".into(),
                session_key: Some("dingtalk:user:alice".into()),
                current_session_key: Some("dingtalk:user:bob".into()),
                prompt: "ping".into(),
                agent: None,
                target_kind: ScheduleTargetKindArg::Auto,
                timezone: None,
                idle_gt_seconds: None,
            },
            ScheduleInput::Delay { delay_ms: 1_000 },
        )
        .unwrap();
        let clawbro_cron::CreateTargetRequest::Session(target) = req.target;
        assert_eq!(target.session_key, "dingtalk:user:alice");
    }

    #[test]
    fn build_job_query_prefers_explicit_session_key_over_current_session() {
        let query = build_job_query(
            Some("reminder".into()),
            None,
            ScheduleSessionFilterArgs {
                session_key: Some("dingtalk:user:alice".into()),
                current_session_key: Some("dingtalk:user:bob".into()),
            },
        )
        .unwrap();
        assert_eq!(query.name.as_deref(), Some("reminder"));
        assert_eq!(query.session_key.as_deref(), Some("dingtalk:user:alice"));
    }
}
