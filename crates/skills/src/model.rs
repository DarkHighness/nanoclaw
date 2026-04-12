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
}

impl SkillProvenance {
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
