use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use types::{HookRegistration, ToolName};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkillRootKind {
    Managed,
    External,
}

impl SkillRootKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Managed => "managed",
            Self::External => "external",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillRoot {
    pub path: PathBuf,
    pub kind: SkillRootKind,
}

impl SkillRoot {
    #[must_use]
    pub fn managed(path: PathBuf) -> Self {
        Self {
            path,
            kind: SkillRootKind::Managed,
        }
    }

    #[must_use]
    pub fn external(path: PathBuf) -> Self {
        Self {
            path,
            kind: SkillRootKind::External,
        }
    }

    #[must_use]
    pub fn writable(&self) -> bool {
        matches!(self.kind, SkillRootKind::Managed)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkillActivation {
    pub platforms: Vec<String>,
    pub requires_tools: Vec<ToolName>,
    pub fallback_for_tools: Vec<ToolName>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillProvenance {
    pub root: SkillRoot,
    pub skill_dir: PathBuf,
    pub hub: Option<SkillHubProvenance>,
    pub shadowed_copies: Vec<SkillShadow>,
}

impl SkillProvenance {
    #[must_use]
    pub fn skill_path(&self) -> PathBuf {
        self.skill_dir.join("SKILL.md")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillTrustLevel {
    Builtin,
    Official,
    Trusted,
    Community,
}

impl SkillTrustLevel {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Official => "official",
            Self::Trusted => "trusted",
            Self::Community => "community",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillUpdateState {
    UpToDate,
    UpdateAvailable,
    Drifted,
}

impl SkillUpdateState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UpToDate => "up_to_date",
            Self::UpdateAvailable => "update_available",
            Self::Drifted => "drifted",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillAuditState {
    Clean,
    Warn,
    Blocked,
}

impl SkillAuditState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Warn => "warn",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillHubProvenance {
    pub source_id: String,
    pub trust_level: SkillTrustLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_state: Option<SkillUpdateState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_state: Option<SkillAuditState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_bundle_hash: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillShadow {
    pub root: SkillRoot,
    pub skill_dir: PathBuf,
}

impl SkillShadow {
    #[must_use]
    pub fn skill_path(&self) -> PathBuf {
        self.skill_dir.join("SKILL.md")
    }
}

#[derive(Clone, Debug)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub body: String,
    pub root_dir: PathBuf,
    pub tags: Vec<String>,
    pub hooks: Vec<HookRegistration>,
    pub references: Vec<PathBuf>,
    pub scripts: Vec<PathBuf>,
    pub assets: Vec<PathBuf>,
    pub metadata: BTreeMap<String, serde_yaml::Value>,
    pub extension_metadata: BTreeMap<String, serde_yaml::Value>,
    pub activation: SkillActivation,
    pub provenance: SkillProvenance,
}

impl Skill {
    #[must_use]
    pub fn system_instruction(&self) -> String {
        self.body.trim().to_string()
    }

    #[must_use]
    pub fn skill_path(&self) -> PathBuf {
        self.provenance.skill_path()
    }
}
