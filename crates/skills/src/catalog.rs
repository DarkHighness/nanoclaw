use crate::{Skill, SkillRoot, SkillRootKind};
use std::collections::BTreeSet;
use std::sync::{Arc, RwLock};

#[derive(Clone, Debug, Default)]
struct SkillCatalogState {
    roots: Vec<SkillRoot>,
    skills: Vec<Skill>,
}

#[derive(Clone, Debug, Default)]
pub struct SkillCatalog {
    state: Arc<RwLock<SkillCatalogState>>,
}

impl SkillCatalog {
    #[must_use]
    pub fn new(skills: Vec<Skill>) -> Self {
        Self::from_parts(Vec::new(), skills)
    }

    #[must_use]
    pub fn from_parts(roots: Vec<SkillRoot>, mut skills: Vec<Skill>) -> Self {
        skills.sort_by(|left, right| left.name.cmp(&right.name));
        Self {
            state: Arc::new(RwLock::new(SkillCatalogState { roots, skills })),
        }
    }

    pub fn replace(&self, roots: Vec<SkillRoot>, mut skills: Vec<Skill>) {
        skills.sort_by(|left, right| left.name.cmp(&right.name));
        *self.state.write().expect("skill catalog write lock") =
            SkillCatalogState { roots, skills };
    }

    #[must_use]
    pub fn roots(&self) -> Vec<SkillRoot> {
        self.state
            .read()
            .expect("skill catalog read lock")
            .roots
            .clone()
    }

    #[must_use]
    pub fn managed_root(&self) -> Option<SkillRoot> {
        self.roots()
            .into_iter()
            .find(|root| matches!(root.kind, SkillRootKind::Managed))
    }

    #[must_use]
    pub fn all(&self) -> Vec<Skill> {
        self.state
            .read()
            .expect("skill catalog read lock")
            .skills
            .clone()
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<Skill> {
        self.state
            .read()
            .expect("skill catalog read lock")
            .skills
            .iter()
            .find(|skill| skill.name == name)
            .cloned()
    }

    #[must_use]
    pub fn resolve(&self, query: &str) -> Option<Skill> {
        let normalized = query.trim();
        if normalized.is_empty() {
            return None;
        }
        self.state
            .read()
            .expect("skill catalog read lock")
            .skills
            .iter()
            .find(|skill| {
                skill.name == normalized
                    || skill
                        .aliases
                        .iter()
                        .any(|alias| alias.as_str() == normalized)
            })
            .cloned()
    }

    #[must_use]
    pub fn tool_names(&self) -> Vec<String> {
        let mut names = BTreeSet::new();
        for skill in self.all() {
            names.insert(skill.name.clone());
        }
        names.into_iter().collect()
    }
}
