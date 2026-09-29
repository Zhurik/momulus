//! Реестр скиллов: читает каталог, валидирует контракты, отдаёт help.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::contract::{Mode, SkillContract};
use crate::doc::SkillDoc;
use crate::error::{SkillError, SkillResult};

/// Имя файла с контрактом.
pub const CONTRACT_FILE: &str = "skill.toml";
/// Имя файла с инструкциями для агента.
pub const DOC_FILE: &str = "SKILL.md";

/// Один скилл: контракт плюс инструкции.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    /// Каталог скилла на хосте.
    pub dir: PathBuf,
    pub contract: SkillContract,
    pub doc: SkillDoc,
}

impl Skill {
    pub fn mode(&self) -> Mode {
        self.contract.mode
    }

    /// Строка для help: `proofread (review) — описание`.
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

/// Все скиллы каталога.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    root: PathBuf,
    skills: BTreeMap<String, Skill>,
}

/// Результат чтения каталога вместе с проблемами отдельных скиллов.
#[derive(Debug)]
pub struct LoadReport {
    pub registry: Registry,
    pub errors: Vec<SkillError>,
}

impl Registry {
    /// Читает каталог; любая проблема скилла — ошибка.
    pub fn load(root: &Path) -> SkillResult<Registry> {
        let report = Registry::load_report(root)?;
        match report.errors.into_iter().next() {
            Some(err) => Err(err),
            None => Ok(report.registry),
        }
    }

    /// Читает каталог, собирая все проблемы — для `skills validate`.
    pub fn load_report(root: &Path) -> SkillResult<LoadReport> {
        let entries = std::fs::read_dir(root).map_err(|e| SkillError::Registry {
            path: root.to_path_buf(),
            message: format!("не читается: {e}"),
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

    /// Текст подсказки, который бот пишет в PR на непонятную команду.
    pub fn help_text(&self) -> String {
        let mut out = String::from("Доступные команды:\n\n");
        if self.skills.is_empty() {
            out.push_str("_скиллов пока нет_\n");
            return out;
        }
        for skill in self.skills.values() {
            out.push_str("- ");
            out.push_str(&skill.summary());
            out.push('\n');
        }
        out.push_str("\nФормат: `/llm <skill> [аргументы]`, например `/llm proofread`.\n");
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
            message: format!("{} не читается: {e}", contract_path.display()),
        })?;
    let doc_text = std::fs::read_to_string(&doc_path).map_err(|e| SkillError::Doc {
        skill: name.clone(),
        message: format!("{} не читается: {e}", doc_path.display()),
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

    /// Создаёт каталог скилла во временной директории.
    fn write_skill(root: &Path, name: &str, contract: &str, doc: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(CONTRACT_FILE), contract).unwrap();
        std::fs::write(dir.join(DOC_FILE), doc).unwrap();
    }

    fn doc_for(name: &str) -> String {
        format!("---\nname: {name}\ndescription: Описание {name}\n---\n\nтело\n")
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
        std::fs::write(tmp.path().join("README.md"), "не скилл").unwrap();
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
            "без frontmatter",
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
        assert!(err.to_string().contains("не читается"), "{err}");
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
        assert!(help.contains("Описание translate"), "{help}");
    }

    #[test]
    fn help_text_for_empty_registry() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = Registry::load(tmp.path()).unwrap();
        assert!(registry.help_text().contains("скиллов пока нет"));
    }
}
