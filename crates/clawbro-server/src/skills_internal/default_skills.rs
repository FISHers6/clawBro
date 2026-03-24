use crate::config::{BackendFamilyConfig, GatewayConfig};
use crate::runtime::AcpBackend;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const GENERATED_MARKER_FILE: &str = ".clawbro-default-skill";

struct EmbeddedSkillFile {
    relative_path: &'static str,
    contents: &'static str,
}

struct EmbeddedSkill {
    name: &'static str,
    files: &'static [EmbeddedSkillFile],
}

#[derive(Debug, Default)]
pub struct DefaultSkillsReconcileReport {
    warnings: Vec<String>,
}

impl DefaultSkillsReconcileReport {
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    fn push_warning(&mut self, warning: String) {
        self.warnings.push(warning);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DefaultSkillsCheckReport {
    roots: Vec<DefaultSkillsRootReport>,
}

impl DefaultSkillsCheckReport {
    pub fn roots(&self) -> &[DefaultSkillsRootReport] {
        &self.roots
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DefaultSkillsRootReport {
    label: String,
    root: PathBuf,
    error: Option<String>,
    skills: Vec<DefaultSkillCheck>,
}

impl DefaultSkillsRootReport {
    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn skills(&self) -> &[DefaultSkillCheck] {
        &self.skills
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DefaultSkillCheck {
    name: String,
    status: DefaultSkillStatus,
}

impl DefaultSkillCheck {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn status(&self) -> DefaultSkillStatus {
        self.status
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DefaultSkillStatus {
    Missing,
    UpToDate,
    UserOwned,
    UserModified,
    UpdateAvailable,
    Blocked,
}

impl DefaultSkillStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            DefaultSkillStatus::Missing => "missing",
            DefaultSkillStatus::UpToDate => "up-to-date",
            DefaultSkillStatus::UserOwned => "user-owned",
            DefaultSkillStatus::UserModified => "user-modified",
            DefaultSkillStatus::UpdateAvailable => "update-available",
            DefaultSkillStatus::Blocked => "blocked",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct GeneratedSkillState {
    format: u32,
    installed_manifest_sha256: String,
}

enum GeneratedOwnership {
    Unmanaged,
    Managed(GeneratedSkillState),
}

const FIND_SKILLS_FILES: &[EmbeddedSkillFile] = &[EmbeddedSkillFile {
    relative_path: "SKILL.md",
    contents: include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/default-skills/find-skills/SKILL.md"
    )),
}];

const SKILL_CREATOR_FILES: &[EmbeddedSkillFile] = &[
    EmbeddedSkillFile {
        relative_path: "SKILL.md",
        contents: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/default-skills/skill-creator/SKILL.md"
        )),
    },
    EmbeddedSkillFile {
        relative_path: "scripts/init_skill.py",
        contents: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/default-skills/skill-creator/scripts/init_skill.py"
        )),
    },
    EmbeddedSkillFile {
        relative_path: "scripts/package_skill.py",
        contents: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/default-skills/skill-creator/scripts/package_skill.py"
        )),
    },
    EmbeddedSkillFile {
        relative_path: "scripts/quick_validate.py",
        contents: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/default-skills/skill-creator/scripts/quick_validate.py"
        )),
    },
];

const WEATHER_FILES: &[EmbeddedSkillFile] = &[EmbeddedSkillFile {
    relative_path: "SKILL.md",
    contents: include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/default-skills/weather/SKILL.md"
    )),
}];

const BASELINE_SKILLS: &[EmbeddedSkill] = &[
    EmbeddedSkill {
        name: "find-skills",
        files: FIND_SKILLS_FILES,
    },
    EmbeddedSkill {
        name: "skill-creator",
        files: SKILL_CREATOR_FILES,
    },
    EmbeddedSkill {
        name: "weather",
        files: WEATHER_FILES,
    },
];

pub fn reconcile_default_skills(cfg: &GatewayConfig) -> Result<DefaultSkillsReconcileReport> {
    let home_dir = home_dir()?;
    let xdg_config_home = xdg_config_home(&home_dir);
    reconcile_default_skills_with_roots(cfg, &home_dir, &xdg_config_home)
}

pub fn check_default_skills(cfg: &GatewayConfig) -> Result<DefaultSkillsCheckReport> {
    let home_dir = home_dir()?;
    let xdg_config_home = xdg_config_home(&home_dir);
    check_default_skills_with_roots(cfg, &home_dir, &xdg_config_home)
}

pub fn sync_default_skills_into_codex_home(codex_home: &Path) -> Result<()> {
    install_embedded_skill_set(&codex_home.join("skills"))
}

fn reconcile_default_skills_with_roots(
    cfg: &GatewayConfig,
    home_dir: &Path,
    xdg_config_home: &Path,
) -> Result<DefaultSkillsReconcileReport> {
    install_embedded_skill_set(&cfg.skills.dir)?;

    let mut report = DefaultSkillsReconcileReport::default();
    let mirror_roots = collect_backend_mirror_roots(cfg, home_dir, xdg_config_home);
    for root in mirror_roots {
        if root == cfg.skills.dir {
            continue;
        }
        if let Err(err) = install_embedded_skill_set(&root) {
            report.push_warning(format!(
                "default skill mirror skipped for {}: {}",
                root.display(),
                err
            ));
        }
    }
    Ok(report)
}

fn check_default_skills_with_roots(
    cfg: &GatewayConfig,
    home_dir: &Path,
    xdg_config_home: &Path,
) -> Result<DefaultSkillsCheckReport> {
    let mut roots = vec![inspect_default_skills_root(
        "source".to_string(),
        cfg.skills.dir.clone(),
    )?];
    let mirror_roots = collect_backend_mirror_roots(cfg, home_dir, xdg_config_home);
    for root in mirror_roots {
        if root == cfg.skills.dir {
            continue;
        }
        roots.push(inspect_default_skills_root("mirror".to_string(), root)?);
    }
    Ok(DefaultSkillsCheckReport { roots })
}

fn install_embedded_skill_set(root: &Path) -> Result<()> {
    std::fs::create_dir_all(root)
        .with_context(|| format!("create default skill root {}", root.display()))?;

    for skill in BASELINE_SKILLS {
        install_embedded_skill(root, skill)?;
    }
    Ok(())
}

fn install_embedded_skill(root: &Path, skill: &EmbeddedSkill) -> Result<()> {
    let target_dir = root.join(skill.name);
    let expected_manifest = embedded_manifest_sha256(skill);

    if target_dir.exists() {
        if !target_dir.is_dir() {
            anyhow::bail!(
                "default skill path is not a directory: {}",
                target_dir.display()
            );
        }

        match load_generated_ownership(&target_dir)? {
            GeneratedOwnership::Unmanaged => return Ok(()),
            GeneratedOwnership::Managed(state) => {
                let actual_manifest = directory_manifest_sha256(&target_dir)?;
                if actual_manifest != state.installed_manifest_sha256 {
                    return Ok(());
                }
                if state.installed_manifest_sha256 == expected_manifest {
                    return Ok(());
                }
                std::fs::remove_dir_all(&target_dir).with_context(|| {
                    format!("remove generated default skill {}", target_dir.display())
                })?;
            }
        }
    }

    std::fs::create_dir_all(&target_dir)
        .with_context(|| format!("create default skill dir {}", target_dir.display()))?;
    for file in skill.files {
        let target_path = target_dir.join(file.relative_path);
        if let Some(parent) = target_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create directory {}", parent.display()))?;
        }
        std::fs::write(&target_path, file.contents)
            .with_context(|| format!("write {}", target_path.display()))?;
    }
    write_generated_state(&target_dir, &expected_manifest)?;
    Ok(())
}

fn inspect_default_skills_root(label: String, root: PathBuf) -> Result<DefaultSkillsRootReport> {
    if root.exists() && !root.is_dir() {
        return Ok(DefaultSkillsRootReport {
            label,
            root,
            error: Some("root path is not a directory".to_string()),
            skills: vec![],
        });
    }

    let mut skills = Vec::with_capacity(BASELINE_SKILLS.len());
    for skill in BASELINE_SKILLS {
        skills.push(DefaultSkillCheck {
            name: skill.name.to_string(),
            status: inspect_embedded_skill(&root, skill)?,
        });
    }
    Ok(DefaultSkillsRootReport {
        label,
        root,
        error: None,
        skills,
    })
}

fn inspect_embedded_skill(root: &Path, skill: &EmbeddedSkill) -> Result<DefaultSkillStatus> {
    let target_dir = root.join(skill.name);
    if !target_dir.exists() {
        return Ok(DefaultSkillStatus::Missing);
    }
    if !target_dir.is_dir() {
        return Ok(DefaultSkillStatus::Blocked);
    }

    let expected_manifest = embedded_manifest_sha256(skill);
    match load_generated_ownership(&target_dir)? {
        GeneratedOwnership::Unmanaged => Ok(DefaultSkillStatus::UserOwned),
        GeneratedOwnership::Managed(state) => {
            let actual_manifest = directory_manifest_sha256(&target_dir)?;
            if actual_manifest != state.installed_manifest_sha256 {
                Ok(DefaultSkillStatus::UserModified)
            } else if state.installed_manifest_sha256 == expected_manifest {
                Ok(DefaultSkillStatus::UpToDate)
            } else {
                Ok(DefaultSkillStatus::UpdateAvailable)
            }
        }
    }
}

fn load_generated_ownership(dir: &Path) -> Result<GeneratedOwnership> {
    let marker_path = dir.join(GENERATED_MARKER_FILE);
    if !marker_path.exists() {
        return Ok(GeneratedOwnership::Unmanaged);
    }

    let marker_contents = std::fs::read_to_string(&marker_path)
        .with_context(|| format!("read {}", marker_path.display()))?;
    let state: GeneratedSkillState = serde_json::from_str(&marker_contents)
        .with_context(|| format!("parse {}", marker_path.display()))?;
    Ok(GeneratedOwnership::Managed(state))
}

fn write_generated_state(dir: &Path, manifest_sha256: &str) -> Result<()> {
    let state = GeneratedSkillState {
        format: 2,
        installed_manifest_sha256: manifest_sha256.to_string(),
    };
    let marker_path = dir.join(GENERATED_MARKER_FILE);
    std::fs::write(&marker_path, serde_json::to_vec_pretty(&state)?)
        .with_context(|| format!("write {}", marker_path.display()))?;
    Ok(())
}

fn embedded_manifest_sha256(skill: &EmbeddedSkill) -> String {
    let mut hasher = Sha256::new();
    for file in skill.files {
        hasher.update(file.relative_path.as_bytes());
        hasher.update([0]);
        hasher.update(file.contents.as_bytes());
        hasher.update([0xff]);
    }
    format!("{:x}", hasher.finalize())
}

fn directory_manifest_sha256(dir: &Path) -> Result<String> {
    let mut files = Vec::new();
    collect_files_recursive(dir, dir, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Sha256::new();
    for (relative, path) in files {
        if relative == GENERATED_MARKER_FILE {
            continue;
        }
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        hasher.update(std::fs::read(&path).with_context(|| format!("read {}", path.display()))?);
        hasher.update([0xff]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn collect_files_recursive(
    root: &Path,
    dir: &Path,
    files: &mut Vec<(String, PathBuf)>,
) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_files_recursive(root, &path, files)?;
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)
                .expect("file path should stay within root")
                .to_string_lossy()
                .replace('\\', "/");
            files.push((relative, path));
        }
    }
    Ok(())
}

fn collect_backend_mirror_roots(
    cfg: &GatewayConfig,
    home_dir: &Path,
    xdg_config_home: &Path,
) -> BTreeSet<PathBuf> {
    let mut roots = BTreeSet::new();
    for backend in &cfg.backends {
        match backend.family {
            BackendFamilyConfig::Acp => {
                if let Some(root) =
                    acp_backend_mirror_root(backend.acp_backend, home_dir, xdg_config_home)
                {
                    roots.insert(root);
                }
            }
            BackendFamilyConfig::OpenClawGateway => {
                roots.insert(openclaw_global_skills_dir(home_dir));
            }
            BackendFamilyConfig::ClawBroNative => {}
        }
    }
    roots
}

fn acp_backend_mirror_root(
    backend: Option<AcpBackend>,
    home_dir: &Path,
    xdg_config_home: &Path,
) -> Option<PathBuf> {
    match backend {
        Some(AcpBackend::Claude) => Some(claude_config_dir(home_dir).join("skills")),
        Some(AcpBackend::Codex) => Some(codex_home_dir(home_dir).join("skills")),
        Some(AcpBackend::Codebuddy) => Some(home_dir.join(".codebuddy").join("skills")),
        Some(AcpBackend::Qwen) => Some(home_dir.join(".qwen").join("skills")),
        Some(AcpBackend::Iflow) => Some(home_dir.join(".iflow").join("skills")),
        Some(AcpBackend::Goose) => Some(xdg_config_home.join("goose").join("skills")),
        Some(AcpBackend::Kimi) => Some(home_dir.join(".config").join("agents").join("skills")),
        Some(AcpBackend::Opencode) => Some(xdg_config_home.join("opencode").join("skills")),
        Some(AcpBackend::Qoder) => Some(home_dir.join(".qoder").join("skills")),
        Some(AcpBackend::Vibe) => Some(home_dir.join(".vibe").join("skills")),
        Some(AcpBackend::Gemini) => Some(home_dir.join(".gemini").join("skills")),
        Some(AcpBackend::Custom) | None => None,
    }
}

fn codex_home_dir(home_dir: &Path) -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir.join(".codex"))
}

fn claude_config_dir(home_dir: &Path) -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir.join(".claude"))
}

fn openclaw_global_skills_dir(home_dir: &Path) -> PathBuf {
    for dir in [".openclaw", ".clawdbot", ".moltbot"] {
        let root = home_dir.join(dir);
        if root.exists() {
            return root.join("skills");
        }
    }
    home_dir.join(".openclaw").join("skills")
}

fn xdg_config_home(home_dir: &Path) -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir.join(".config"))
}

fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().ok_or_else(|| anyhow::anyhow!("HOME is required to resolve default skills"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BackendCatalogEntry, BackendLaunchConfig, SkillsSection};
    use tempfile::TempDir;

    fn dummy_backend(
        id: &str,
        family: BackendFamilyConfig,
        acp_backend: Option<AcpBackend>,
    ) -> BackendCatalogEntry {
        BackendCatalogEntry {
            id: id.into(),
            family,
            adapter_key: None,
            acp_backend,
            acp_auth_method: None,
            codex: None,
            provider_profile: None,
            approval: Default::default(),
            external_mcp_servers: vec![],
            launch: BackendLaunchConfig::BundledCommand,
        }
    }

    #[test]
    fn collect_backend_mirror_roots_maps_known_backends() {
        let mut cfg = GatewayConfig::default();
        cfg.backends = vec![
            dummy_backend(
                "claude-main",
                BackendFamilyConfig::Acp,
                Some(AcpBackend::Claude),
            ),
            dummy_backend(
                "codex-main",
                BackendFamilyConfig::Acp,
                Some(AcpBackend::Codex),
            ),
            dummy_backend("openclaw-main", BackendFamilyConfig::OpenClawGateway, None),
        ];
        let home = PathBuf::from("/tmp/home");
        let xdg = home.join(".config");

        let roots = collect_backend_mirror_roots(&cfg, &home, &xdg);

        assert!(roots.contains(&PathBuf::from("/tmp/home/.claude/skills")));
        assert!(roots.contains(&PathBuf::from("/tmp/home/.codex/skills")));
        assert!(roots.contains(&PathBuf::from("/tmp/home/.openclaw/skills")));
    }

    #[test]
    fn install_embedded_skill_set_creates_baseline_skills() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("skills");

        install_embedded_skill_set(&root).unwrap();

        assert!(root.join("find-skills/SKILL.md").exists());
        assert!(root.join("skill-creator/SKILL.md").exists());
        assert!(root.join("skill-creator/scripts/init_skill.py").exists());
        assert!(root.join("weather/SKILL.md").exists());
    }

    #[test]
    fn install_embedded_skill_set_does_not_overwrite_user_owned_skill_dir() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("skills");
        let user_skill = root.join("weather");
        std::fs::create_dir_all(&user_skill).unwrap();
        std::fs::write(user_skill.join("SKILL.md"), "user custom").unwrap();

        install_embedded_skill_set(&root).unwrap();

        let body = std::fs::read_to_string(user_skill.join("SKILL.md")).unwrap();
        assert_eq!(body, "user custom");
        assert!(!user_skill.join(GENERATED_MARKER_FILE).exists());
    }

    #[test]
    fn install_embedded_skill_set_preserves_user_edits_to_generated_skill() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("skills");

        install_embedded_skill_set(&root).unwrap();
        std::fs::write(root.join("weather/SKILL.md"), "customized").unwrap();

        install_embedded_skill_set(&root).unwrap();

        let body = std::fs::read_to_string(root.join("weather/SKILL.md")).unwrap();
        assert_eq!(body, "customized");
    }

    #[test]
    fn reconcile_default_skills_installs_source_and_backend_mirrors() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let xdg = home.join(".config");

        let mut cfg = GatewayConfig::default();
        cfg.skills = SkillsSection {
            dir: home.join(".clawbro").join("skills"),
            global_dirs: vec![],
        };
        cfg.backends = vec![dummy_backend(
            "claude-main",
            BackendFamilyConfig::Acp,
            Some(AcpBackend::Claude),
        )];

        let report = reconcile_default_skills_with_roots(&cfg, &home, &xdg).unwrap();

        assert!(report.warnings().is_empty());
        assert!(home.join(".clawbro/skills/find-skills/SKILL.md").exists());
        assert!(home.join(".claude/skills/skill-creator/SKILL.md").exists());
    }

    #[test]
    fn reconcile_default_skills_warns_when_mirror_root_is_blocked() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let xdg = home.join(".config");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(home.join(".claude/skills"), "blocked").unwrap();

        let mut cfg = GatewayConfig::default();
        cfg.skills = SkillsSection {
            dir: home.join(".clawbro").join("skills"),
            global_dirs: vec![],
        };
        cfg.backends = vec![dummy_backend(
            "claude-main",
            BackendFamilyConfig::Acp,
            Some(AcpBackend::Claude),
        )];

        let report = reconcile_default_skills_with_roots(&cfg, &home, &xdg).unwrap();

        assert_eq!(report.warnings().len(), 1);
        assert!(home.join(".clawbro/skills/find-skills/SKILL.md").exists());
    }

    #[test]
    fn check_default_skills_reports_missing_root_without_creating_it() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let xdg = home.join(".config");

        let mut cfg = GatewayConfig::default();
        cfg.skills = SkillsSection {
            dir: home.join(".clawbro").join("skills"),
            global_dirs: vec![],
        };
        cfg.backends.clear();

        let report = check_default_skills_with_roots(&cfg, &home, &xdg).unwrap();

        assert_eq!(report.roots().len(), 1);
        assert_eq!(report.roots()[0].label(), "source");
        assert_eq!(
            report.roots()[0].skills()[0].status(),
            DefaultSkillStatus::Missing
        );
        assert!(!cfg.skills.dir.exists());
    }

    #[test]
    fn check_default_skills_reports_user_modified_generated_skill() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let xdg = home.join(".config");

        let mut cfg = GatewayConfig::default();
        cfg.skills = SkillsSection {
            dir: home.join(".clawbro").join("skills"),
            global_dirs: vec![],
        };
        cfg.backends.clear();

        reconcile_default_skills_with_roots(&cfg, &home, &xdg).unwrap();
        std::fs::write(cfg.skills.dir.join("weather/SKILL.md"), "customized").unwrap();

        let report = check_default_skills_with_roots(&cfg, &home, &xdg).unwrap();
        let weather = report.roots()[0]
            .skills()
            .iter()
            .find(|skill| skill.name() == "weather")
            .unwrap();
        assert_eq!(weather.status(), DefaultSkillStatus::UserModified);
    }

    #[test]
    fn sync_default_skills_into_codex_home_writes_skills_subdir() {
        let tmp = TempDir::new().unwrap();
        sync_default_skills_into_codex_home(tmp.path()).unwrap();
        assert!(tmp.path().join("skills/weather/SKILL.md").exists());
    }

    #[test]
    fn reconcile_default_skills_mirrors_new_backend_root_after_backend_switch() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let xdg = home.join(".config");

        let mut cfg = GatewayConfig::default();
        cfg.skills = SkillsSection {
            dir: home.join(".clawbro").join("skills"),
            global_dirs: vec![],
        };
        cfg.backends = vec![dummy_backend(
            "qoder-main",
            BackendFamilyConfig::Acp,
            Some(AcpBackend::Qoder),
        )];

        reconcile_default_skills_with_roots(&cfg, &home, &xdg).unwrap();
        assert!(home.join(".qoder/skills/find-skills/SKILL.md").exists());

        cfg.backends = vec![dummy_backend(
            "claude-main",
            BackendFamilyConfig::Acp,
            Some(AcpBackend::Claude),
        )];

        reconcile_default_skills_with_roots(&cfg, &home, &xdg).unwrap();
        assert!(home.join(".claude/skills/skill-creator/SKILL.md").exists());
    }
}
