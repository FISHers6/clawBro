use crate::agent_core::{
    roster::AgentEntry,
    skill_paths::{agent_scoped_skills_dir, project_universal_skills_dir, workspace_private_skills_dir},
};
use crate::config::GatewayConfig;
use crate::runtime::BackendSpec;
use crate::skills_internal::{check_default_skills, DefaultSkillsCheckReport, LoadedSkill, SkillLoader};
use crate::state::AppState;
use axum::{
    extract::{Path as AxumPath, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::types::{derive_agent_identities, ApiErrorBody};

const HOST_SCHEDULER_SKILL_MD: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/host-skills/scheduler/SKILL.md"
));
const HOST_TEAM_LEAD_SKILL_MD: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/host-skills/team-lead/SKILL.md"
));
const HOST_TEAM_SPECIALIST_SKILL_MD: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/host-skills/team-specialist/SKILL.md"
));

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SkillRootView {
    pub label: String,
    pub path: String,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SkillEntryView {
    pub name: String,
    pub version: String,
    pub source_label: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillsOverviewView {
    pub host_skills: Vec<SkillEntryView>,
    pub effective_skills: Vec<SkillEntryView>,
    pub roots: Vec<SkillRootView>,
    pub default_skills: DefaultSkillsCheckReport,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentSkillsView {
    pub agent_id: String,
    pub role: String,
    pub backend_id: String,
    pub supports_native_local_skills: bool,
    pub host_skills: Vec<SkillEntryView>,
    pub effective_skills: Vec<SkillEntryView>,
    pub roots: Vec<SkillRootView>,
}

#[derive(Debug, Clone)]
struct LabeledRoot {
    label: String,
    path: PathBuf,
}

pub async fn list_skills(
    State(state): State<AppState>,
) -> Result<Json<SkillsOverviewView>, (StatusCode, Json<ApiErrorBody>)> {
    let roots = resolve_global_roots(state.cfg.as_ref());
    let root_paths: Vec<PathBuf> = roots.iter().map(|root| root.path.clone()).collect();
    let loader = SkillLoader::with_dirs(root_paths);
    let effective_skills = map_loaded_skills(&roots, loader.load_all());
    let default_skills = check_default_skills(state.cfg.as_ref()).map_err(internal_error)?;

    Ok(Json(SkillsOverviewView {
        host_skills: load_host_skills(None),
        effective_skills,
        roots: roots_to_views(roots),
        default_skills,
    }))
}

pub async fn get_agent_skills(
    AxumPath(name): AxumPath<String>,
    State(state): State<AppState>,
) -> Result<Json<AgentSkillsView>, (StatusCode, Json<ApiErrorBody>)> {
    let roster = state.registry.roster.as_ref().ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ApiErrorBody {
                error: "agent roster not configured".to_string(),
            }),
        )
    })?;
    let entry = roster.find_by_name(&name).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ApiErrorBody {
                error: format!("agent '{}' not found", name),
            }),
        )
    })?;

    let backend_spec = state
        .runtime_registry
        .backend_spec(entry.runtime_backend_id())
        .await;
    let role = derive_agent_role(state.cfg.as_ref(), &entry.name);
    let roots = resolve_agent_roots(state.cfg.as_ref(), entry);
    let root_paths: Vec<PathBuf> = roots.iter().map(|root| root.path.clone()).collect();
    let loader = SkillLoader::with_dirs(root_paths);

    Ok(Json(AgentSkillsView {
        agent_id: entry.name.clone(),
        role: role.as_str().to_string(),
        backend_id: entry.backend_id.clone(),
        supports_native_local_skills: backend_spec
            .as_ref()
            .is_some_and(BackendSpec::supports_native_local_skills),
        host_skills: load_host_skills(Some(role)),
        effective_skills: map_loaded_skills(&roots, loader.load_all()),
        roots: roots_to_views(roots),
    }))
}

fn load_host_skills(role: Option<AgentRole>) -> Vec<SkillEntryView> {
    let mut entries = host_skill_entries()
        .into_iter()
        .filter(|skill| match role {
            Some(AgentRole::Lead) => {
                skill.name == "scheduler" || skill.name == "canonical-team-lead"
            }
            Some(AgentRole::Specialist) => skill.name == "canonical-team-specialist",
            Some(AgentRole::Solo) => skill.name == "scheduler",
            None => true,
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

fn host_skill_entries() -> Vec<SkillEntryView> {
    [
        (
            "scheduler",
            "[host]/scheduler/SKILL.md",
            HOST_SCHEDULER_SKILL_MD,
        ),
        (
            "team-lead",
            "[host]/team-lead/SKILL.md",
            HOST_TEAM_LEAD_SKILL_MD,
        ),
        (
            "team-specialist",
            "[host]/team-specialist/SKILL.md",
            HOST_TEAM_SPECIALIST_SKILL_MD,
        ),
    ]
    .into_iter()
    .map(|(dir_name, path, skill_md)| {
        let (name, version) = parse_skill_frontmatter(skill_md, dir_name);
        SkillEntryView {
            name,
            version,
            source_label: "host".to_string(),
            path: path.to_string(),
        }
    })
    .collect()
}

fn parse_skill_frontmatter(content: &str, dir_name_hint: &str) -> (String, String) {
    if !content.starts_with("---\n") {
        return (dir_name_hint.to_string(), "0.0.0".to_string());
    }

    let rest = &content[4..];
    let end = rest
        .find("\n---\n")
        .map(|p| (p, p + 5))
        .or_else(|| rest.find("\n---").map(|p| (p, p + 4)));
    let frontmatter = match end {
        Some((fm_end, _)) => &rest[..fm_end],
        None => rest,
    };

    let mut name = dir_name_hint.to_string();
    let mut version = "0.0.0".to_string();
    for line in frontmatter.lines() {
        if let Some((key, val)) = line.split_once(':') {
            let key = key.trim();
            let val = val.trim().trim_matches('\'').trim_matches('"');
            if val.is_empty() {
                continue;
            }
            match key {
                "name" => name = val.to_string(),
                "version" => version = val.to_string(),
                _ => {}
            }
        }
    }
    (name, version)
}

fn map_loaded_skills(roots: &[LabeledRoot], skills: Vec<LoadedSkill>) -> Vec<SkillEntryView> {
    skills
        .into_iter()
        .map(|skill| SkillEntryView {
            name: skill.manifest.name,
            version: skill.manifest.version,
            source_label: source_label_for_path(roots, &skill.dir),
            path: display_skill_path(roots, &skill.dir),
        })
        .collect()
}

fn roots_to_views(roots: Vec<LabeledRoot>) -> Vec<SkillRootView> {
    roots
        .into_iter()
        .map(|root| {
            let display_path = display_root_path(&root.label);
            SkillRootView {
                label: root.label,
                exists: root.path.exists(),
                path: display_path,
            }
        })
        .collect()
}

fn resolve_global_roots(cfg: &GatewayConfig) -> Vec<LabeledRoot> {
    let mut roots = Vec::new();
    let mut seen = HashSet::new();
    let mut configured_global_dirs = vec![cfg.skills.dir.clone()];
    configured_global_dirs.extend(cfg.skills.global_dirs.iter().cloned());
    let global_loader = SkillLoader::new(configured_global_dirs);
    for path in global_loader.search_dirs() {
        let label = classify_global_root(cfg, path);
        push_labeled_root(&mut roots, &mut seen, label, path.clone());
    }
    roots
}

fn resolve_agent_roots(cfg: &GatewayConfig, entry: &AgentEntry) -> Vec<LabeledRoot> {
    let mut roots = resolve_global_roots(cfg);
    let mut seen = roots
        .iter()
        .map(|root| root.path.canonicalize().unwrap_or_else(|_| root.path.clone()))
        .collect::<HashSet<_>>();
    if let Some(workspace) = entry.workspace_dir.as_deref() {
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
        push_labeled_root(
            &mut roots,
            &mut seen,
            format!("agent:{}", entry.name),
            agent_scoped_skills_dir(workspace, &entry.name),
        );
    }
    for path in &entry.extra_skills_dirs {
        push_labeled_root(&mut roots, &mut seen, "agent-extra".to_string(), path.clone());
    }
    roots
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

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
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

fn source_label_for_path(roots: &[LabeledRoot], path: &Path) -> String {
    roots
        .iter()
        .find(|root| path.starts_with(&root.path))
        .map(|root| root.label.clone())
        .unwrap_or_else(|| "unknown".to_string())
}

fn display_root_path(label: &str) -> String {
    format!("[{label}]")
}

fn display_skill_path(roots: &[LabeledRoot], path: &Path) -> String {
    if let Some(root) = roots.iter().find(|root| path.starts_with(&root.path)) {
        let relative = path
            .strip_prefix(&root.path)
            .ok()
            .filter(|relative| !relative.as_os_str().is_empty())
            .map(|relative| relative.display().to_string())
            .unwrap_or_else(|| ".".to_string());
        return format!("{}/{}", display_root_path(&root.label), relative);
    }

    let leaf = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown");
    format!("{}/{}", display_root_path("unknown"), leaf)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentRole {
    Lead,
    Specialist,
    Solo,
}

impl AgentRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Lead => "lead",
            Self::Specialist => "specialist",
            Self::Solo => "solo",
        }
    }
}

fn derive_agent_role(cfg: &GatewayConfig, agent_name: &str) -> AgentRole {
    let identities = derive_agent_identities(cfg, agent_name);
    if identities.iter().any(|identity| identity == "front_bot") {
        AgentRole::Lead
    } else if identities.iter().any(|identity| identity == "roster_member") {
        AgentRole::Specialist
    } else {
        AgentRole::Solo
    }
}

fn internal_error(err: anyhow::Error) -> (StatusCode, Json<ApiErrorBody>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiErrorBody {
            error: err.to_string(),
        }),
    )
}
