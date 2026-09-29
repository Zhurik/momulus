//! Skill registry: reads the directory, validates contracts, renders help.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::contract::{Mode, SkillContract};
use crate::doc::SkillDoc;
use crate::error::{SkillError, SkillResult};

/// Name of the contract file.
pub const CONTRACT_FILE: &str = "skill.toml";
/// Name of the file holding the agent instructions.
pub const DOC_FILE: &str = "SKILL.md";

/// A single skill: contract plus instructions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    /// The skill's directory on the host.
    pub dir: PathBuf,
    pub contract: SkillContract,
    pub doc: SkillDoc,
}

impl Skill {
    pub fn mode(&self) -> Mode {
        self.contract.mode
    }

    /// Help line: `proofread (review) — description`.
    pub fn summary(&self) -> String {
        let args = if self.contract.args.is_empty() {
            String::new()
        } else {
            format!(
                " {}",
                self.contract
                    .args
                    .iter()
                    .map(|a| format!("<{a}>"))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        };
        format!(
            "`/llm {}{}` ({}) — {}",
            self.name, args, self.contract.mode, self.doc.description
        )
    }
}

/// Every skill in the directory.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    root: PathBuf,
    skills: BTreeMap<String, Skill>,
}

/// The result of reading the directory, together with per-skill problems.
#[derive(Debug)]
pub struct LoadReport {
    pub registry: Registry,
    pub errors: Vec<SkillError>,
}

impl Registry {
    /// Reads the directory; any skill problem is an error.
    pub fn load(root: &Path) -> SkillResult<Registry> {
        let report = Registry::load_report(root)?;
        match report.errors.into_iter().next() {
            Some(err) => Err(err),
            None => Ok(report.registry),
        }
    }

    /// Reads the directory collecting every problem — used by `skills validate`.
    pub fn load_report(root: &Path) -> SkillResult<LoadReport> {
        let entries = std::fs::read_dir(root).map_err(|e| SkillError::Registry {
            path: root.to_path_buf(),
            message: format!("cannot be read: {e}"),
        })?;

        let mut dirs: Vec<PathBuf> = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| SkillError::Registry {
                path: root.to_path_buf(),
                message: e.to_string(),
            })?;
            let path = entry.path();
            if path.is_dir() && !name_of(&path).starts_with('.') {
                dirs.push(path);
            }
        }
        dirs.sort();

        let mut skills = BTreeMap::new();
        let mut errors = Vec::new();
        for dir in dirs {
            match load_skill(&dir) {
                Ok(skill) => {
                    skills.insert(skill.name.clone(), skill);
                }
                Err(err) => errors.push(err),
            }
        }

        Ok(LoadReport {
            registry: Registry {
                root: root.to_path_buf(),
                skills,
            },
            errors,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn get(&self, name: &str) -> SkillResult<&Skill> {
        self.skills
            .get(name)
            .ok_or_else(|| SkillError::Unknown(name.to_string()))
    }

    pub fn contains(&self, name: &str) -> bool {
        self.skills.contains_key(name)
    }

    pub fn names(&self) -> Vec<&str> {
        self.skills.keys().map(String::as_str).collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Skill> {
        self.skills.values()
    }

    pub fn len(&self) -> usize {
        self.skills.len()
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// The help text the bot posts on an unparseable command.
    pub fn help_text(&self) -> String {
        let mut out = String::from("Available commands:\n\n");
        if self.skills.is_empty() {
            out.push_str("_no skills are installed yet_\n");
            return out;
        }
        for skill in self.skills.values() {
            out.push_str("- ");
            out.push_str(&skill.summary());
            out.push('\n');
        }
        out.push_str("\nFormat: `/llm <skill> [arguments]`, for example `/llm proofread`.\n");
        out
    }
}

fn load_skill(dir: &Path) -> SkillResult<Skill> {
    let name = name_of(dir);
    let contract_path = dir.join(CONTRACT_FILE);
    let doc_path = dir.join(DOC_FILE);

    let contract_text =
        std::fs::read_to_string(&contract_path).map_err(|e| SkillError::Contract {
            skill: name.clone(),
            message: format!("{} cannot be read: {e}", contract_path.display()),
        })?;
    let doc_text = std::fs::read_to_string(&doc_path).map_err(|e| SkillError::Doc {
        skill: name.clone(),
        message: format!("{} cannot be read: {e}", doc_path.display()),
    })?;

    let contract = SkillContract::parse(&name, &contract_text)?;
    let doc = SkillDoc::parse(&name, &doc_text)?;

    Ok(Skill {
        name,
        dir: dir.to_path_buf(),
        contract,
        doc,
    })
}

fn name_of(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates a skill directory inside a temporary directory.
    fn write_skill(root: &Path, name: &str, contract: &str, doc: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(CONTRACT_FILE), contract).unwrap();
        std::fs::write(dir.join(DOC_FILE), doc).unwrap();
    }

    fn doc_for(name: &str) -> String {
        format!("---\nname: {name}\ndescription: Description of {name}\n---\n\nbody\n")
    }

    #[test]
    fn loads_all_skills_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(
            tmp.path(),
            "review",
            "mode = \"review\"\ntools = [\"read\", \"write\"]",
            &doc_for("review"),
        );
        write_skill(
            tmp.path(),
            "proofread",
            "mode = \"review\"\ntools = [\"read\", \"write\"]\nfiles = [\"**/*.mdx\"]",
            &doc_for("proofread"),
        );
        let registry = Registry::load(tmp.path()).unwrap();
        assert_eq!(registry.names(), vec!["proofread", "review"]);
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn ignores_hidden_directories_and_files() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(
            tmp.path(),
            "review",
            "mode = \"review\"\ntools = [\"read\", \"write\"]",
            &doc_for("review"),
        );
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        std::fs::write(tmp.path().join("README.md"), "not a skill").unwrap();
        let registry = Registry::load(tmp.path()).unwrap();
        assert_eq!(registry.names(), vec!["review"]);
    }

    #[test]
    fn reports_missing_contract() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("broken")).unwrap();
        std::fs::write(tmp.path().join("broken").join(DOC_FILE), doc_for("broken")).unwrap();
        let err = Registry::load(tmp.path()).unwrap_err();
        assert!(err.to_string().contains("skill.toml"), "{err}");
    }

    #[test]
    fn collects_all_errors_in_report() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(tmp.path(), "a", "mode = \"patch\"", &doc_for("a"));
        write_skill(
            tmp.path(),
            "b",
            "mode = \"review\"\ntools = [\"read\", \"write\"]",
            "no frontmatter here",
        );
        write_skill(
            tmp.path(),
            "ok",
            "mode = \"review\"\ntools = [\"read\", \"write\"]",
            &doc_for("ok"),
        );
        let report = Registry::load_report(tmp.path()).unwrap();
        assert_eq!(report.errors.len(), 2, "{:?}", report.errors);
        assert_eq!(report.registry.names(), vec!["ok"]);
    }

    #[test]
    fn unknown_skill_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = Registry::load(tmp.path()).unwrap();
        let err = registry.get("nope").unwrap_err();
        assert!(err.to_string().contains("nope"), "{err}");
    }

    #[test]
    fn missing_directory_is_reported() {
        let err = Registry::load(Path::new("/definitely/not/here")).unwrap_err();
        assert!(err.to_string().contains("cannot be read"), "{err}");
    }

    #[test]
    fn help_text_lists_skills_with_args() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(
            tmp.path(),
            "translate",
            "mode = \"patch\"\ntools = [\"write\", \"edit\"]\nargs = [\"lang\"]",
            &doc_for("translate"),
        );
        let registry = Registry::load(tmp.path()).unwrap();
        let help = registry.help_text();
        assert!(help.contains("`/llm translate <lang>` (patch)"), "{help}");
        assert!(help.contains("Description of translate"), "{help}");
    }

    #[test]
    fn help_text_for_empty_registry() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = Registry::load(tmp.path()).unwrap();
        assert!(registry.help_text().contains("no skills are installed yet"));
    }
}
