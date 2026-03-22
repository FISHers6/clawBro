use crate::agent_core::skill_paths::{
    agent_scoped_skills_dir, project_universal_skills_dir, workspace_private_skills_dir,
};
use crate::cli::args::{
    SkillAddArgs, SkillArgs, SkillCheckArgs, SkillCommands, SkillHubArgs, SkillHubCommands,
    SkillHubInstallArgs, SkillHubListArgs, SkillHubSearchArgs, SkillHubSyncArgs,
    SkillHubUpdateArgs, SkillListArgs, SkillRemoveArgs, SkillScopeArg, SkillSyncArgs,
};
use crate::config::GatewayConfig;
use crate::skills_internal::{
    check_default_skills, reconcile_default_skills, DefaultSkillsCheckReport,
    DefaultSkillsReconcileReport, LoadedSkill, SkillLoader,
};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

const CLAWBRO_SKILLS_AGENT: &str = "clawbro";

pub async fn run(args: SkillArgs) -> Result<()> {
    let cfg = GatewayConfig::load()?;
    match args.command {
        SkillCommands::Add(add) => cmd_add(&cfg, add),
        SkillCommands::Check(check) => cmd_check(&cfg, check),
        SkillCommands::Hub(hub) => cmd_hub(hub),
        SkillCommands::List(list) => cmd_list(&cfg, list),
        SkillCommands::Remove(remove) => cmd_remove(&cfg, remove),
        SkillCommands::Sync(sync) => cmd_sync(&cfg, sync),
    }
}

fn cmd_add(cfg: &GatewayConfig, args: SkillAddArgs) -> Result<()> {
    if let Some(invocation) = delegated_add_invocation(&args)? {
        return run_external_cli(invocation);
    }

    let target_root = resolve_scope_root(
        cfg,
        args.scope,
        args.workspace.as_deref(),
        args.agent.as_deref(),
    )?;
    std::fs::create_dir_all(&target_root)
        .with_context(|| format!("create target skill root {}", target_root.display()))?;

    let resolved_sources = resolve_scoped_install_sources(&args)?;
    let mut installed_paths = Vec::new();
    for source in resolved_sources.paths {
        let install_name = source
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| anyhow::anyhow!("source must be a named directory"))?;
        let destination = target_root.join(install_name);
        if destination.exists() {
            if !args.force {
                bail!(
                    "destination already exists: {} (pass --force to replace)",
                    destination.display()
                );
            }
            std::fs::remove_dir_all(&destination).with_context(|| {
                format!("remove existing destination {}", destination.display())
            })?;
        }

        copy_dir_recursive(&source, &destination)?;
        installed_paths.push(destination);
    }
    for path in installed_paths {
        println!("{}", path.display());
    }
    Ok(())
}

fn format_check_report_text(report: &DefaultSkillsCheckReport) -> String {
    let mut lines = Vec::new();
    for root in report.roots() {
        lines.push(format!("[{}] {}", root.label(), root.root().display()));
        if let Some(error) = root.error() {
            lines.push(format!("  error  {error}"));
            continue;
        }
        for skill in root.skills() {
            lines.push(format!("  {}  {}", skill.name(), skill.status().as_str()));
        }
    }
    lines.join("\n")
}

fn cmd_list(cfg: &GatewayConfig, args: SkillListArgs) -> Result<()> {
    let report = build_skill_list_report(cfg, &args)?;
    let output = if args.json {
        serde_json::to_string_pretty(&report)?
    } else {
        format_skill_list_report_text(&report)
    };
    println!("{output}");
    Ok(())
}

fn cmd_remove(cfg: &GatewayConfig, args: SkillRemoveArgs) -> Result<()> {
    let target_root = resolve_scope_root(
        cfg,
        args.scope,
        args.workspace.as_deref(),
        args.agent.as_deref(),
    )?;
    let destination = target_root.join(&args.name);
    if !destination.exists() {
        bail!("skill does not exist: {}", destination.display());
    }
    if !destination.is_dir() {
        bail!("skill path is not a directory: {}", destination.display());
    }

    std::fs::remove_dir_all(&destination)
        .with_context(|| format!("remove installed skill {}", destination.display()))?;
    println!("{}", destination.display());
    Ok(())
}

fn cmd_sync(cfg: &GatewayConfig, _args: SkillSyncArgs) -> Result<()> {
    let report = reconcile_default_skills(cfg)?;
    if _args.json {
        println!("{}", format_sync_json(cfg.skills.dir.clone(), &report)?);
    } else {
        println!("baseline source {}", cfg.skills.dir.display());
        if report.warnings().is_empty() {
            println!("mirrors ok");
        } else {
            for warning in report.warnings() {
                eprintln!("warning: {warning}");
            }
        }
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct SkillSyncJsonOutput {
    source_root: PathBuf,
    warnings: Vec<String>,
}

impl SkillSyncJsonOutput {
    fn from_report(source_root: PathBuf, report: &DefaultSkillsReconcileReport) -> Self {
        Self {
            source_root,
            warnings: report.warnings().to_vec(),
        }
    }
}

fn format_sync_json(source_root: PathBuf, report: &DefaultSkillsReconcileReport) -> Result<String> {
    Ok(serde_json::to_string_pretty(
        &SkillSyncJsonOutput::from_report(source_root, report),
    )?)
}

fn cmd_check(cfg: &GatewayConfig, args: SkillCheckArgs) -> Result<()> {
    let report = check_default_skills(cfg)?;
    let output = if args.json {
        serde_json::to_string_pretty(&report)?
    } else {
        format_check_report_text(&report)
    };
    println!("{output}");
    Ok(())
}

struct ResolvedInstallSources {
    paths: Vec<PathBuf>,
    _staging: Option<tempfile::TempDir>,
}

fn resolve_scoped_install_sources(args: &SkillAddArgs) -> Result<ResolvedInstallSources> {
    if let Some(source) = try_single_local_skill_dir(&args.source)? {
        return Ok(ResolvedInstallSources {
            paths: vec![source],
            _staging: None,
        });
    }
    stage_remote_skill_sources(args.source.to_string_lossy().into_owned())
}

fn try_single_local_skill_dir(source: &Path) -> Result<Option<PathBuf>> {
    if !source.exists() {
        return Ok(None);
    }
    if !source.is_dir() {
        bail!(
            "source must be a directory or an installable skills source: {}",
            source.display()
        );
    }
    let skill_md = source.join("SKILL.md");
    if !skill_md.exists() {
        return Ok(None);
    }
    Ok(Some(
        source
            .canonicalize()
            .unwrap_or_else(|_| source.to_path_buf()),
    ))
}

fn stage_remote_skill_sources(source: String) -> Result<ResolvedInstallSources> {
    let staging = tempfile::TempDir::new().context("create temporary staging workspace")?;
    let invocation = build_skills_stage_invocation(source, staging.path().to_path_buf())?;
    run_external_cli(invocation)?;

    let staged_root = project_universal_skills_dir(staging.path());
    let mut staged_sources = Vec::new();
    if staged_root.is_dir() {
        for entry in std::fs::read_dir(&staged_root)
            .with_context(|| format!("read staged skills {}", staged_root.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() && path.join("SKILL.md").is_file() {
                staged_sources.push(path);
            }
        }
    }
    staged_sources.sort();
    if staged_sources.is_empty() {
        bail!("no skills were installed from source into staging workspace");
    }
    Ok(ResolvedInstallSources {
        paths: staged_sources,
        _staging: Some(staging),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExternalCliInvocation {
    program: String,
    args: Vec<String>,
    cwd: Option<PathBuf>,
}

fn delegated_add_invocation(args: &SkillAddArgs) -> Result<Option<ExternalCliInvocation>> {
    if args.force {
        return Ok(None);
    }

    let scope = match args.scope {
        SkillScopeArg::Managed => DelegatedScope::Global,
        SkillScopeArg::Project => DelegatedScope::Project(
            args.workspace
                .clone()
                .ok_or_else(|| anyhow::anyhow!("--workspace is required for project scope"))?,
        ),
        SkillScopeArg::Private | SkillScopeArg::Agent => return Ok(None),
    };

    Ok(Some(build_skills_add_invocation(
        args.source.to_string_lossy().into_owned(),
        scope,
    )?))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DelegatedScope {
    Global,
    Project(PathBuf),
}

fn build_skills_add_invocation(
    source: String,
    scope: DelegatedScope,
) -> Result<ExternalCliInvocation> {
    let mut invocation = build_skills_invocation(source, scope.clone())?;
    append_delegated_scope_flag(&mut invocation.args, &scope);
    Ok(invocation)
}

fn build_skills_stage_invocation(
    source: String,
    workspace: PathBuf,
) -> Result<ExternalCliInvocation> {
    let mut invocation = build_skills_invocation(source, DelegatedScope::Project(workspace))?;
    invocation.args.push("--copy".to_string());
    Ok(invocation)
}

fn build_skills_invocation(source: String, scope: DelegatedScope) -> Result<ExternalCliInvocation> {
    let cwd = delegated_cwd(&scope);
    let (program, mut args) = delegated_skills_base_args()?;
    let agent = delegated_skills_agent();
    args.extend([
        "add".to_string(),
        source,
        "--agent".to_string(),
        agent.to_string(),
        "-y".to_string(),
    ]);
    Ok(ExternalCliInvocation { program, args, cwd })
}

fn delegated_skills_base_args() -> Result<(String, Vec<String>)> {
    if let Some(local_cli) = local_skills_cli_path().filter(|_| command_available("node")) {
        return Ok((
            "node".to_string(),
            vec![local_cli.to_string_lossy().into_owned()],
        ));
    }
    if command_available("npx") {
        return Ok((
            "npx".to_string(),
            vec!["--yes".to_string(), "skills".to_string()],
        ));
    }
    bail!("skills delegation requires either `node` with local skills sources or `npx` on PATH")
}

fn append_delegated_scope_flag(args: &mut Vec<String>, scope: &DelegatedScope) {
    if matches!(scope, DelegatedScope::Global) {
        args.push("--global".to_string());
    }
}

fn delegated_cwd(scope: &DelegatedScope) -> Option<PathBuf> {
    match scope {
        DelegatedScope::Global => None,
        DelegatedScope::Project(workspace) => Some(workspace.clone()),
    }
}

fn local_skills_cli_path() -> Option<PathBuf> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join("skills")
        .join("src")
        .join("cli.ts");
    path.is_file().then_some(path)
}

fn delegated_skills_agent() -> &'static str {
    CLAWBRO_SKILLS_AGENT
}

fn run_external_cli(invocation: ExternalCliInvocation) -> Result<()> {
    let mut command = Command::new(&invocation.program);
    command.args(&invocation.args);
    if let Some(cwd) = &invocation.cwd {
        command.current_dir(cwd);
    }

    let status = command.status().with_context(|| {
        format!(
            "run delegated external cli: {} {:?}",
            invocation.program, invocation.args
        )
    })?;
    if status.success() {
        Ok(())
    } else {
        bail!(
            "delegated external CLI failed with status {}",
            status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "terminated by signal".to_string())
        );
    }
}

fn cmd_hub(args: SkillHubArgs) -> Result<()> {
    let invocation = build_clawhub_invocation(args.command)?;
    run_external_cli(invocation)
}

fn build_clawhub_invocation(command: SkillHubCommands) -> Result<ExternalCliInvocation> {
    let (program, mut args) = clawhub_base_args()?;
    let cwd = clawhub_cwd(&command);
    match command {
        SkillHubCommands::Search(search) => append_clawhub_search_args(&mut args, search),
        SkillHubCommands::Install(install) => append_clawhub_install_args(&mut args, install),
        SkillHubCommands::List(list) => append_clawhub_list_args(&mut args, list),
        SkillHubCommands::Update(update) => append_clawhub_update_args(&mut args, update)?,
        SkillHubCommands::Sync(sync) => append_clawhub_sync_args(&mut args, sync),
    }
    Ok(ExternalCliInvocation { program, args, cwd })
}

fn clawhub_base_args() -> Result<(String, Vec<String>)> {
    if command_available("clawhub") {
        return Ok(("clawhub".to_string(), Vec::new()));
    }
    if command_available("npx") {
        return Ok((
            "npx".to_string(),
            vec!["--yes".to_string(), "clawhub".to_string()],
        ));
    }
    bail!("ClawHub delegation requires either `clawhub` or `npx` on PATH")
}

fn clawhub_cwd(command: &SkillHubCommands) -> Option<PathBuf> {
    match command {
        SkillHubCommands::Search(_) => None,
        SkillHubCommands::Install(args) => args.workspace.clone(),
        SkillHubCommands::List(args) => args.workspace.clone(),
        SkillHubCommands::Update(args) => args.workspace.clone(),
        SkillHubCommands::Sync(args) => args.workspace.clone(),
    }
}

fn append_clawhub_search_args(args: &mut Vec<String>, search: SkillHubSearchArgs) {
    args.push("search".to_string());
    args.push(search.query);
    if let Some(limit) = search.limit {
        args.push("--limit".to_string());
        args.push(limit.to_string());
    }
}

fn append_clawhub_install_args(args: &mut Vec<String>, install: SkillHubInstallArgs) {
    args.push("install".to_string());
    args.push(install.slug);
    append_optional_string_flag(
        args,
        "--dir",
        clawhub_dir_arg(install.workspace.as_deref(), install.dir),
    );
    append_optional_string_flag(args, "--version", install.version);
    if install.force {
        args.push("--force".to_string());
    }
}

fn append_clawhub_list_args(args: &mut Vec<String>, list: SkillHubListArgs) {
    args.push("list".to_string());
    append_optional_string_flag(
        args,
        "--dir",
        clawhub_dir_arg(list.workspace.as_deref(), list.dir),
    );
}

fn append_clawhub_update_args(args: &mut Vec<String>, update: SkillHubUpdateArgs) -> Result<()> {
    if update.all && update.slug.is_some() {
        bail!("use either <slug> or --all for `skill hub update`, not both");
    }
    if !update.all && update.slug.is_none() {
        bail!("`skill hub update` requires either <slug> or --all");
    }

    args.push("update".to_string());
    if update.all {
        args.push("--all".to_string());
    } else if let Some(slug) = update.slug {
        args.push(slug);
    }
    append_optional_string_flag(
        args,
        "--dir",
        clawhub_dir_arg(update.workspace.as_deref(), update.dir),
    );
    append_optional_string_flag(args, "--version", update.version);
    if update.force {
        args.push("--force".to_string());
    }
    Ok(())
}

fn append_clawhub_sync_args(args: &mut Vec<String>, sync: SkillHubSyncArgs) {
    args.push("sync".to_string());
    append_optional_string_flag(
        args,
        "--dir",
        clawhub_dir_arg(sync.workspace.as_deref(), sync.dir),
    );
    for root in sync.roots {
        args.push("--root".to_string());
        args.push(root.to_string_lossy().into_owned());
    }
    if sync.all {
        args.push("--all".to_string());
    }
    if sync.dry_run {
        args.push("--dry-run".to_string());
    }
    append_optional_string_flag(args, "--bump", sync.bump);
    append_optional_string_flag(args, "--changelog", sync.changelog);
    append_optional_string_flag(args, "--tags", sync.tags);
    if let Some(concurrency) = sync.concurrency {
        args.push("--concurrency".to_string());
        args.push(concurrency.to_string());
    }
}

fn clawhub_dir_arg(workspace: Option<&Path>, explicit_dir: Option<PathBuf>) -> Option<String> {
    explicit_dir
        .map(|value| value.to_string_lossy().into_owned())
        .or_else(|| {
            workspace.map(|workspace| {
                project_universal_skills_dir(workspace)
                    .to_string_lossy()
                    .into_owned()
            })
        })
}

fn append_optional_string_flag(args: &mut Vec<String>, flag: &str, value: Option<String>) {
    if let Some(value) = value {
        args.push(flag.to_string());
        args.push(value);
    }
}

fn command_available(name: &str) -> bool {
    let path_var = match std::env::var_os("PATH") {
        Some(value) => value,
        None => return false,
    };
    std::env::split_paths(&path_var).any(|dir| command_exists_in_dir(&dir, name))
}

fn command_exists_in_dir(dir: &Path, name: &str) -> bool {
    let direct = dir.join(name);
    if direct.is_file() {
        return true;
    }
    if cfg!(windows) {
        for ext in ["exe", "cmd", "bat"] {
            if dir.join(format!("{name}.{ext}")).is_file() {
                return true;
            }
        }
    }
    false
}

fn resolve_scope_root(
    cfg: &GatewayConfig,
    scope: SkillScopeArg,
    workspace: Option<&Path>,
    agent: Option<&str>,
) -> Result<PathBuf> {
    match scope {
        SkillScopeArg::Managed => Ok(cfg.skills.dir.clone()),
        SkillScopeArg::Project => {
            let workspace = workspace
                .ok_or_else(|| anyhow::anyhow!("--workspace is required for project scope"))?;
            Ok(project_universal_skills_dir(workspace))
        }
        SkillScopeArg::Private => {
            let workspace = workspace
                .ok_or_else(|| anyhow::anyhow!("--workspace is required for private scope"))?;
            Ok(workspace_private_skills_dir(workspace))
        }
        SkillScopeArg::Agent => {
            let workspace = workspace
                .ok_or_else(|| anyhow::anyhow!("--workspace is required for agent scope"))?;
            let agent =
                agent.ok_or_else(|| anyhow::anyhow!("--agent is required for agent scope"))?;
            Ok(agent_scoped_skills_dir(workspace, agent))
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SkillListRoot {
    label: String,
    path: PathBuf,
    exists: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct EffectiveSkillEntry {
    kind: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    source_label: String,
    path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
struct SkillListReport {
    effective: Vec<EffectiveSkillEntry>,
    roots: Vec<SkillListRoot>,
    default_skills: DefaultSkillsCheckReport,
}

fn build_skill_list_report(cfg: &GatewayConfig, args: &SkillListArgs) -> Result<SkillListReport> {
    let roots = resolve_effective_list_roots(cfg, args)?;
    let root_paths: Vec<PathBuf> = roots.iter().map(|root| root.path.clone()).collect();
    let loader = SkillLoader::with_dirs(root_paths);
    let mut effective = Vec::new();
    effective.push(EffectiveSkillEntry {
        kind: "skill".to_string(),
        name: "scheduler".to_string(),
        version: Some("builtin".to_string()),
        source_label: "builtin".to_string(),
        path: PathBuf::from("[builtin]/scheduler"),
    });
    effective.extend(
        loader
            .load_all()
            .into_iter()
            .map(|skill| effective_skill_from_loaded(&roots, skill)),
    );
    effective.extend(effective_personas_from_roots(&roots));

    let roots = roots
        .into_iter()
        .map(|root| SkillListRoot {
            exists: root.path.exists(),
            label: root.label,
            path: root.path,
        })
        .collect();

    Ok(SkillListReport {
        effective,
        roots,
        default_skills: check_default_skills(cfg)?,
    })
}

fn format_skill_list_report_text(report: &SkillListReport) -> String {
    let mut lines = Vec::new();
    lines.push("[effective]".to_string());
    if report.effective.is_empty() {
        lines.push("  (empty)".to_string());
    } else {
        for entry in &report.effective {
            match &entry.version {
                Some(version) => lines.push(format!(
                    "  {}  {}  v{}  [{}] {}",
                    entry.kind,
                    entry.name,
                    version,
                    entry.source_label,
                    entry.path.display()
                )),
                None => lines.push(format!(
                    "  {}  {}  [{}] {}",
                    entry.kind,
                    entry.name,
                    entry.source_label,
                    entry.path.display()
                )),
            }
        }
    }
    lines.push(String::new());
    lines.push("[roots]".to_string());
    for root in &report.roots {
        let status = if root.exists { "present" } else { "missing" };
        lines.push(format!(
            "  [{}] {}  {}",
            root.label,
            root.path.display(),
            status
        ));
    }
    lines.push(String::new());
    lines.push("[default-skills]".to_string());
    lines.push(format_check_report_text(&report.default_skills));
    lines.join("\n")
}

#[derive(Debug, Clone)]
struct LabeledRoot {
    label: String,
    path: PathBuf,
}

fn resolve_effective_list_roots(
    cfg: &GatewayConfig,
    args: &SkillListArgs,
) -> Result<Vec<LabeledRoot>> {
    if let Some(scope) = args.scope {
        let scoped =
            resolve_scope_root(cfg, scope, args.workspace.as_deref(), args.agent.as_deref())?;
        return Ok(vec![LabeledRoot {
            label: scope_label(scope).to_string(),
            path: scoped,
        }]);
    }

    let mut roots = Vec::new();
    let mut seen = HashSet::new();
    let mut configured_global_dirs = vec![cfg.skills.dir.clone()];
    configured_global_dirs.extend(cfg.skills.global_dirs.iter().cloned());
    let global_loader = SkillLoader::new(configured_global_dirs.clone());
    for path in global_loader.search_dirs() {
        let label = classify_global_root(cfg, path);
        push_labeled_root(&mut roots, &mut seen, label, path.clone());
    }

    if let Some(workspace) = args.workspace.as_deref() {
        push_labeled_root(
            &mut roots,
            &mut seen,
            "project".to_string(),
            project_universal_skills_dir(workspace),
        );
        push_labeled_root(
            &mut roots,
            &mut seen,
            "private".to_string(),
            workspace_private_skills_dir(workspace),
        );
        if let Some(agent) = args.agent.as_deref() {
            push_labeled_root(
                &mut roots,
                &mut seen,
                format!("agent:{agent}"),
                agent_scoped_skills_dir(workspace, agent),
            );
        }
    }
    Ok(roots)
}

fn classify_global_root(cfg: &GatewayConfig, path: &Path) -> String {
    if paths_equivalent(path, &cfg.skills.dir) {
        return "managed".to_string();
    }
    if let Some(home) = dirs::home_dir() {
        let universal = home.join(".agents").join("skills");
        if paths_equivalent(path, &universal) {
            return "universal-global".to_string();
        }
    }
    if let Some(index) = cfg
        .skills
        .global_dirs
        .iter()
        .position(|candidate| paths_equivalent(candidate, path))
    {
        return format!("global:{index}");
    }
    "global".to_string()
}

fn push_labeled_root(
    roots: &mut Vec<LabeledRoot>,
    seen: &mut HashSet<PathBuf>,
    label: String,
    path: PathBuf,
) {
    let normalized = path.canonicalize().unwrap_or_else(|_| path.clone());
    if seen.insert(normalized) {
        roots.push(LabeledRoot { label, path });
    }
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn effective_skill_from_loaded(roots: &[LabeledRoot], skill: LoadedSkill) -> EffectiveSkillEntry {
    EffectiveSkillEntry {
        kind: "skill".to_string(),
        name: skill.manifest.name,
        version: Some(skill.manifest.version),
        source_label: source_label_for_path(roots, &skill.dir),
        path: skill.dir,
    }
}

fn effective_personas_from_roots(roots: &[LabeledRoot]) -> Vec<EffectiveSkillEntry> {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for root in roots.iter().rev() {
        let loader = SkillLoader::with_dirs(vec![root.path.clone()]);
        for persona in loader.load_personas() {
            let key = persona.identity.name.to_ascii_lowercase();
            if seen.insert(key) {
                entries.push(EffectiveSkillEntry {
                    kind: "persona".to_string(),
                    name: persona.identity.name,
                    version: None,
                    source_label: root.label.clone(),
                    path: root.path.clone(),
                });
            }
        }
    }
    entries.reverse();
    entries
}

fn source_label_for_path(roots: &[LabeledRoot], path: &Path) -> String {
    roots
        .iter()
        .find(|root| path.starts_with(&root.path))
        .map(|root| root.label.clone())
        .unwrap_or_else(|| "unknown".to_string())
}

fn scope_label(scope: SkillScopeArg) -> &'static str {
    match scope {
        SkillScopeArg::Managed => "managed",
        SkillScopeArg::Project => "project",
        SkillScopeArg::Private => "private",
        SkillScopeArg::Agent => "agent",
    }
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir_all(destination)
        .with_context(|| format!("create directory {}", destination.display()))?;
    for entry in std::fs::read_dir(source)
        .with_context(|| format!("read source directory {}", source.display()))?
    {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_dir_recursive(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            std::fs::copy(&source_path, &destination_path).with_context(|| {
                format!(
                    "copy file {} -> {}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::{
        SkillCheckArgs, SkillHubInstallArgs, SkillHubListArgs, SkillHubSearchArgs,
        SkillHubSyncArgs, SkillHubUpdateArgs, SkillSyncArgs,
    };

    #[test]
    fn resolve_scope_root_returns_project_private_and_agent_paths() {
        let cfg = GatewayConfig::default();
        let workspace = Path::new("/tmp/workspace");

        assert_eq!(
            resolve_scope_root(&cfg, SkillScopeArg::Project, Some(workspace), None).unwrap(),
            PathBuf::from("/tmp/workspace/.agents/skills")
        );
        assert_eq!(
            resolve_scope_root(&cfg, SkillScopeArg::Private, Some(workspace), None).unwrap(),
            PathBuf::from("/tmp/workspace/skills")
        );
        assert_eq!(
            resolve_scope_root(&cfg, SkillScopeArg::Agent, Some(workspace), Some("alpha")).unwrap(),
            PathBuf::from("/tmp/workspace/.agents/agents/alpha/skills")
        );
    }

    #[test]
    fn delegated_add_invocation_uses_npx_skills_for_managed_scope() {
        let invocation = delegated_add_invocation(&SkillAddArgs {
            source: PathBuf::from("vercel-labs/skills@find-skills"),
            scope: SkillScopeArg::Managed,
            workspace: None,
            agent: None,
            force: false,
        })
        .unwrap()
        .unwrap();

        assert_eq!(invocation.program, "node");
        assert!(invocation.args[0].ends_with("/skills/src/cli.ts"));
        assert_eq!(
            invocation.args[1..],
            vec![
                "add",
                "vercel-labs/skills@find-skills",
                "--agent",
                "clawbro",
                "-y",
                "--global",
            ]
        );
        assert!(invocation.cwd.is_none());
    }

    #[test]
    fn delegated_add_invocation_uses_workspace_for_project_scope() {
        let invocation = delegated_add_invocation(&SkillAddArgs {
            source: PathBuf::from("https://github.com/vercel-labs/skills"),
            scope: SkillScopeArg::Project,
            workspace: Some(PathBuf::from("/tmp/ws")),
            agent: None,
            force: false,
        })
        .unwrap()
        .unwrap();

        assert_eq!(
            invocation.args[1..],
            vec![
                "add",
                "https://github.com/vercel-labs/skills",
                "--agent",
                "clawbro",
                "-y",
            ]
        );
        assert_eq!(invocation.cwd, Some(PathBuf::from("/tmp/ws")));
    }

    #[test]
    fn delegated_add_invocation_skips_private_and_agent_scopes() {
        let private_invocation = delegated_add_invocation(&SkillAddArgs {
            source: PathBuf::from("owner/repo"),
            scope: SkillScopeArg::Private,
            workspace: Some(PathBuf::from("/tmp/ws")),
            agent: None,
            force: false,
        })
        .unwrap();
        let agent_invocation = delegated_add_invocation(&SkillAddArgs {
            source: PathBuf::from("owner/repo"),
            scope: SkillScopeArg::Agent,
            workspace: Some(PathBuf::from("/tmp/ws")),
            agent: Some("alpha".into()),
            force: false,
        })
        .unwrap();

        assert!(private_invocation.is_none());
        assert!(agent_invocation.is_none());
    }

    #[test]
    fn delegated_skills_agent_is_clawbro() {
        assert_eq!(delegated_skills_agent(), "clawbro");
    }

    #[test]
    fn build_skills_stage_invocation_forces_copy_into_temp_workspace() {
        let invocation = build_skills_stage_invocation(
            "vercel-labs/skills@find-skills".into(),
            PathBuf::from("/tmp/ws"),
        )
        .unwrap();

        assert_eq!(invocation.cwd, Some(PathBuf::from("/tmp/ws")));
        assert!(invocation.args.ends_with(&["-y".into(), "--copy".into()]));
    }

    #[test]
    fn clawhub_base_args_falls_back_to_npx_when_binary_is_missing() {
        let (program, args) = clawhub_base_args().unwrap();

        assert_eq!(program, "npx");
        assert_eq!(args, vec!["--yes", "clawhub"]);
    }

    #[test]
    fn build_clawhub_install_invocation_defaults_to_project_universal_dir() {
        let invocation = build_clawhub_invocation(SkillHubCommands::Install(SkillHubInstallArgs {
            slug: "weather-pack".into(),
            workspace: Some(PathBuf::from("/tmp/ws")),
            dir: None,
            version: Some("1.2.3".into()),
            force: true,
        }))
        .unwrap();

        assert_eq!(invocation.program, "npx");
        assert_eq!(invocation.cwd, Some(PathBuf::from("/tmp/ws")));
        assert_eq!(
            invocation.args,
            vec![
                "--yes",
                "clawhub",
                "install",
                "weather-pack",
                "--dir",
                "/tmp/ws/.agents/skills",
                "--version",
                "1.2.3",
                "--force",
            ]
        );
    }

    #[test]
    fn build_clawhub_search_invocation_stays_outside_workspace() {
        let invocation = build_clawhub_invocation(SkillHubCommands::Search(SkillHubSearchArgs {
            query: "calendar".into(),
            limit: Some(5),
        }))
        .unwrap();

        assert!(invocation.cwd.is_none());
        assert_eq!(
            invocation.args,
            vec!["--yes", "clawhub", "search", "calendar", "--limit", "5"]
        );
    }

    #[test]
    fn build_clawhub_update_invocation_validates_slug_or_all() {
        let err = build_clawhub_invocation(SkillHubCommands::Update(SkillHubUpdateArgs {
            slug: None,
            all: false,
            workspace: Some(PathBuf::from("/tmp/ws")),
            dir: Some(PathBuf::from("skills")),
            version: None,
            force: false,
        }))
        .unwrap_err()
        .to_string();

        assert!(err.contains("requires either <slug> or --all"));
    }

    #[test]
    fn build_clawhub_sync_invocation_preserves_optional_flags() {
        let invocation = build_clawhub_invocation(SkillHubCommands::Sync(SkillHubSyncArgs {
            workspace: Some(PathBuf::from("/tmp/ws")),
            dir: Some(PathBuf::from("skills")),
            roots: vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")],
            all: true,
            dry_run: true,
            bump: Some("minor".into()),
            changelog: Some("update".into()),
            tags: Some("latest,beta".into()),
            concurrency: Some(8),
        }))
        .unwrap();

        assert_eq!(invocation.cwd, Some(PathBuf::from("/tmp/ws")));
        assert_eq!(
            invocation.args,
            vec![
                "--yes",
                "clawhub",
                "sync",
                "--dir",
                "skills",
                "--root",
                "/tmp/a",
                "--root",
                "/tmp/b",
                "--all",
                "--dry-run",
                "--bump",
                "minor",
                "--changelog",
                "update",
                "--tags",
                "latest,beta",
                "--concurrency",
                "8",
            ]
        );
    }

    #[test]
    fn build_clawhub_list_invocation_uses_explicit_dir_when_provided() {
        let invocation = build_clawhub_invocation(SkillHubCommands::List(SkillHubListArgs {
            workspace: Some(PathBuf::from("/tmp/ws")),
            dir: Some(PathBuf::from("vendor-skills")),
        }))
        .unwrap();

        assert_eq!(invocation.cwd, Some(PathBuf::from("/tmp/ws")));
        assert_eq!(
            invocation.args,
            vec!["--yes", "clawhub", "list", "--dir", "vendor-skills"]
        );
    }

    #[test]
    fn build_clawhub_list_invocation_defaults_to_project_universal_dir() {
        let invocation = build_clawhub_invocation(SkillHubCommands::List(SkillHubListArgs {
            workspace: Some(PathBuf::from("/tmp/ws")),
            dir: None,
        }))
        .unwrap();

        assert_eq!(invocation.cwd, Some(PathBuf::from("/tmp/ws")));
        assert_eq!(
            invocation.args,
            vec![
                "--yes",
                "clawhub",
                "list",
                "--dir",
                "/tmp/ws/.agents/skills"
            ]
        );
    }

    #[test]
    fn try_single_local_skill_dir_returns_none_when_skill_md_is_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert_eq!(try_single_local_skill_dir(tmp.path()).unwrap(), None);
    }

    #[test]
    fn try_single_local_skill_dir_returns_none_for_repo_style_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("skills/demo")).unwrap();

        assert_eq!(try_single_local_skill_dir(tmp.path()).unwrap(), None);
    }

    #[test]
    fn copy_dir_recursive_copies_nested_skill_contents() {
        let tmp = tempfile::TempDir::new().unwrap();
        let source = tmp.path().join("source-skill");
        let destination = tmp.path().join("dest-skill");
        std::fs::create_dir_all(source.join("scripts")).unwrap();
        std::fs::write(source.join("SKILL.md"), "---\nname: demo\n---\nBody").unwrap();
        std::fs::write(source.join("scripts/tool.sh"), "echo hi").unwrap();

        copy_dir_recursive(&source, &destination).unwrap();

        assert!(destination.join("SKILL.md").exists());
        assert!(destination.join("scripts/tool.sh").exists());
    }

    #[test]
    fn resolve_effective_list_roots_includes_agent_scope_when_requested() {
        let cfg = GatewayConfig::default();
        let roots = resolve_effective_list_roots(
            &cfg,
            &SkillListArgs {
                scope: None,
                workspace: Some(PathBuf::from("/tmp/workspace")),
                agent: Some("alpha".into()),
                json: false,
            },
        )
        .unwrap();

        let labels: Vec<&str> = roots.iter().map(|root| root.label.as_str()).collect();
        assert!(labels.contains(&"managed"));
        assert!(labels.contains(&"project"));
        assert!(labels.contains(&"private"));
        assert!(labels.contains(&"agent:alpha"));
    }

    #[test]
    fn cmd_remove_deletes_installed_skill_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = GatewayConfig::default();
        let workspace = tmp.path().join("workspace");
        let skill_dir = workspace.join(".agents/skills/demo");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "---\nname: demo\n---\nBody").unwrap();

        cmd_remove(
            &cfg,
            SkillRemoveArgs {
                name: "demo".into(),
                scope: SkillScopeArg::Project,
                workspace: Some(workspace),
                agent: None,
            },
        )
        .unwrap();

        assert!(!skill_dir.exists());
    }

    #[test]
    fn cmd_remove_errors_when_skill_is_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut cfg = GatewayConfig::default();
        cfg.skills.dir = tmp.path().join("managed-skills");
        let err = cmd_remove(
            &cfg,
            SkillRemoveArgs {
                name: "missing".into(),
                scope: SkillScopeArg::Managed,
                workspace: Some(tmp.path().join("ignored")),
                agent: None,
            },
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("skill does not exist"));
    }

    #[test]
    fn cmd_sync_populates_managed_default_skills() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut cfg = GatewayConfig::default();
        cfg.skills.dir = tmp.path().join("managed-skills");
        cfg.backends.clear();

        cmd_sync(&cfg, SkillSyncArgs::default()).unwrap();

        assert!(cfg.skills.dir.join("find-skills/SKILL.md").exists());
        assert!(cfg.skills.dir.join("skill-creator/SKILL.md").exists());
        assert!(cfg.skills.dir.join("weather/SKILL.md").exists());
    }

    #[test]
    fn cmd_check_reads_default_skill_status_without_writing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut cfg = GatewayConfig::default();
        cfg.skills.dir = tmp.path().join("managed-skills");
        cfg.backends.clear();

        cmd_check(&cfg, SkillCheckArgs::default()).unwrap();

        assert!(!cfg.skills.dir.exists());
    }

    #[test]
    fn format_check_report_text_includes_status_lines() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut cfg = GatewayConfig::default();
        cfg.skills.dir = tmp.path().join("managed-skills");
        cfg.backends.clear();

        let report = check_default_skills(&cfg).unwrap();

        let output = format_check_report_text(&report);

        assert!(output.contains(&format!("[source] {}", cfg.skills.dir.display())));
        assert!(output.contains("weather  missing"));
    }

    #[test]
    fn build_skill_list_report_includes_builtin_scheduler_and_effective_workspace_override() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut cfg = GatewayConfig::default();
        cfg.skills.dir = tmp.path().join("managed-skills");
        cfg.backends.clear();
        let managed_skill = cfg.skills.dir.join("shared");
        std::fs::create_dir_all(&managed_skill).unwrap();
        std::fs::write(
            managed_skill.join("SKILL.md"),
            "---\nname: shared\nmetadata:\n  version: '1.0.0'\n---\nManaged body",
        )
        .unwrap();

        let workspace = tmp.path().join("workspace");
        let private_skill = workspace.join("skills/shared");
        std::fs::create_dir_all(&private_skill).unwrap();
        std::fs::write(
            private_skill.join("SKILL.md"),
            "---\nname: shared\nmetadata:\n  version: '2.0.0'\n---\nPrivate body",
        )
        .unwrap();

        let report = build_skill_list_report(
            &cfg,
            &SkillListArgs {
                scope: None,
                workspace: Some(workspace),
                agent: None,
                json: false,
            },
        )
        .unwrap();

        assert_eq!(report.effective[0].name, "scheduler");
        let shared = report
            .effective
            .iter()
            .find(|entry| entry.name == "shared" && entry.kind == "skill")
            .unwrap();
        assert_eq!(shared.version.as_deref(), Some("2.0.0"));
        assert_eq!(shared.source_label, "private");
    }

    #[test]
    fn format_skill_list_report_text_includes_effective_and_default_sections() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut cfg = GatewayConfig::default();
        cfg.skills.dir = tmp.path().join("managed-skills");
        cfg.backends.clear();

        let report = build_skill_list_report(
            &cfg,
            &SkillListArgs {
                scope: None,
                workspace: None,
                agent: None,
                json: false,
            },
        )
        .unwrap();

        let output = format_skill_list_report_text(&report);
        assert!(output.contains("[effective]"));
        assert!(output.contains("scheduler"));
        assert!(output.contains("[default-skills]"));
    }

    #[test]
    fn skill_list_report_serializes_effective_entries_to_json() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut cfg = GatewayConfig::default();
        cfg.skills.dir = tmp.path().join("managed-skills");
        cfg.backends.clear();

        let report = build_skill_list_report(
            &cfg,
            &SkillListArgs {
                scope: None,
                workspace: None,
                agent: None,
                json: true,
            },
        )
        .unwrap();

        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["effective"][0]["name"], "scheduler");
        assert_eq!(json["effective"][0]["source_label"], "builtin");
        assert!(json["default_skills"]["roots"].is_array());
    }

    #[test]
    fn format_sync_json_serializes_source_and_warnings() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut cfg = GatewayConfig::default();
        cfg.skills.dir = tmp.path().join("managed-skills");
        cfg.backends.clear();

        let report = reconcile_default_skills(&cfg).unwrap();

        let output = format_sync_json(cfg.skills.dir.clone(), &report).unwrap();
        let json: serde_json::Value = serde_json::from_str(&output).unwrap();

        assert_eq!(json["source_root"], cfg.skills.dir.display().to_string());
        assert_eq!(json["warnings"], serde_json::json!([]));
    }
}
