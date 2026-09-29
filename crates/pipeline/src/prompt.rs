//! Шаблон промпта для pi: вызов скилла плюс контракт вывода.

use std::collections::BTreeMap;

use momulus_core::ReviewOutput;
use momulus_skills::{Mode, Skill};

/// Куда монтируется рабочая копия внутри контейнера.
pub const WORK_DIR: &str = "/work";
/// Куда контейнер складывает артефакты.
pub const OUT_DIR: &str = "/out";
/// Имя файла с находками review.
pub const FINDINGS_FILE: &str = "findings.json";
/// Имя файла с резюме patch.
pub const SUMMARY_FILE: &str = "summary.md";

/// Откуда запущен скилл — влияет только на вводную часть промпта.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Джоба по команде в PR.
    PullRequest { repo: String, number: u64 },
    /// Локальный прогон через `momulus run`.
    Local { path: String },
}

/// Всё, что нужно для рендера промпта.
#[derive(Debug, Clone)]
pub struct PromptContext<'a> {
    pub skill: &'a Skill,
    /// Файлы, с которыми работает скилл (пути относительно корня репозитория).
    pub files: &'a [String],
    /// Аргументы команды, уже сопоставленные с контрактом.
    pub args: &'a BTreeMap<String, String>,
    pub origin: Origin,
}

impl PromptContext<'_> {
    /// Основной промпт.
    pub fn render(&self) -> String {
        let mut out = String::new();

        // Явный вызов скилла: на автоматический выбор не полагаемся.
        out.push_str(&format!("/skill:{}\n\n", self.skill.name));

        match &self.origin {
            Origin::PullRequest { repo, number } => {
                out.push_str(&format!(
                    "Контекст: pull request #{number} в репозитории {repo}.\n"
                ));
            }
            Origin::Local { path } => {
                out.push_str(&format!("Контекст: локальный прогон на копии {path}.\n"));
            }
        }
        out.push_str(&format!(
            "Рабочая копия смонтирована в {WORK_DIR}. Все пути ниже — относительно {WORK_DIR}.\n\n"
        ));

        out.push_str("Файлы, с которыми нужно работать:\n");
        if self.files.is_empty() {
            out.push_str("- (список пуст)\n");
        } else {
            for file in self.files {
                out.push_str(&format!("- {file}\n"));
            }
        }

        if !self.args.is_empty() {
            out.push_str("\nАргументы команды:\n");
            for (key, value) in self.args {
                out.push_str(&format!("- {key} = {value}\n"));
            }
        }

        if !self.skill.contract.vars.is_empty() {
            out.push_str("\nПараметры скилла:\n");
            for (key, value) in &self.skill.contract.vars {
                out.push_str(&format!("- {key} = {value}\n"));
            }
        }

        out.push('\n');
        match self.skill.contract.mode {
            Mode::Review => out.push_str(&self.review_contract()),
            Mode::Patch => out.push_str(&self.patch_contract()),
        }
        out
    }

    /// Повторный промпт после невалидного JSON: сообщаем, что именно сломалось.
    pub fn render_retry(&self, error: &str) -> String {
        format!(
            "{}\n\nПредыдущая попытка не прошла валидацию: {}\n\
             Перечитай требования к формату выше и запиши {OUT_DIR}/{FINDINGS_FILE} заново. \
             Файл должен содержать только JSON — без markdown-обёртки, комментариев и текста вокруг.\n",
            self.render(),
            error.trim()
        )
    }

    fn review_contract(&self) -> String {
        let max = self.skill.contract.max_comments();
        format!(
            "## Что вернуть\n\n\
             Запиши результат инструментом write в файл {OUT_DIR}/{FINDINGS_FILE}.\n\
             Это единственный артефакт; в stdout ничего структурированного писать не нужно.\n\
             Рабочая копия {WORK_DIR} смонтирована только для чтения — менять в ней ничего нельзя.\n\n\
             Файл обязан быть валидным JSON по этой JSON Schema:\n\n\
             ```json\n{schema}\n```\n\n\
             Правила:\n\
             - `path` — путь относительно {WORK_DIR}, ровно как в списке файлов выше;\n\
             - `line` — номер строки в текущей (новой) версии файла, начиная с 1;\n\
             - `severity` — одно из значений схемы;\n\
             - `body` — короткое замечание на русском: что не так и что сделать;\n\
             - `suggestion` — необязательная замена строки целиком, без markdown-обёртки\n\
             \u{a0} и без номера строки;\n\
             - `summary` — 2–5 предложений: общая картина и то, что не уложилось в замечания;\n\
             - не больше {max} замечаний; если нашёл больше — оставь самые важные\n\
             \u{a0} и скажи об остальных в `summary`;\n\
             - если замечаний нет, верни пустой массив `findings` и напиши это в `summary`.\n\n\
             Не пиши никаких других файлов и не меняй файлы в {WORK_DIR}.\n",
            schema = review_schema(),
            max = max,
        )
    }

    fn patch_contract(&self) -> String {
        format!(
            "## Что сделать\n\n\
             Правь файлы прямо в {WORK_DIR}. Когда закончишь:\n\n\
             - запиши в {OUT_DIR}/{SUMMARY_FILE} короткое резюме изменений на русском\n\
             \u{a0} (что и зачем поменял, чего намеренно не трогал);\n\
             - не выполняй git-команд: коммит, ветку и pull request делает оркестратор;\n\
             - не трогай файлы вне {WORK_DIR}, кроме {OUT_DIR}/{SUMMARY_FILE};\n\
             - если менять нечего, ничего не меняй и напиши это в {OUT_DIR}/{SUMMARY_FILE}.\n"
        )
    }
}

/// JSON Schema ожидаемого вывода review — её же отдаём модели в промпте.
pub fn review_schema() -> String {
    let schema = schemars::schema_for!(ReviewOutput);
    serde_json::to_string_pretty(&schema).expect("схема сериализуется")
}

#[cfg(test)]
mod tests {
    use super::*;
    use momulus_skills::{SkillContract, SkillDoc};
    use std::path::PathBuf;

    fn skill(name: &str, contract: &str) -> Skill {
        Skill {
            name: name.to_string(),
            dir: PathBuf::from(format!("/skills/{name}")),
            contract: SkillContract::parse(name, contract).unwrap(),
            doc: SkillDoc {
                name: name.to_string(),
                description: format!("описание {name}"),
                body: String::new(),
            },
        }
    }

    #[test]
    fn review_prompt_snapshot() {
        let skill = skill(
            "proofread",
            "mode = \"review\"\ntools = [\"read\", \"grep\", \"write\"]\nfiles = [\"**/*.mdx\"]\nmax_comments = 5",
        );
        let files = vec!["posts/hello.mdx".to_string(), "posts/next.mdx".to_string()];
        let ctx = PromptContext {
            skill: &skill,
            files: &files,
            args: &BTreeMap::new(),
            origin: Origin::PullRequest {
                repo: "acme/blog".to_string(),
                number: 42,
            },
        };
        insta::assert_snapshot!("review_prompt", ctx.render());
    }

    #[test]
    fn patch_prompt_snapshot() {
        let skill = skill(
            "translate",
            "mode = \"patch\"\ntools = [\"read\", \"write\", \"edit\"]\nargs = [\"lang\"]\n[vars]\nnaming = \"{stem}.{lang}{ext}\"",
        );
        let files = vec!["posts/hello.mdx".to_string()];
        let args = BTreeMap::from([("lang".to_string(), "en".to_string())]);
        let ctx = PromptContext {
            skill: &skill,
            files: &files,
            args: &args,
            origin: Origin::PullRequest {
                repo: "acme/blog".to_string(),
                number: 7,
            },
        };
        insta::assert_snapshot!("patch_prompt", ctx.render());
    }

    #[test]
    fn local_origin_prompt_snapshot() {
        let skill = skill("review", "mode = \"review\"\ntools = [\"read\", \"write\"]");
        let files = vec!["src/main.rs".to_string()];
        let ctx = PromptContext {
            skill: &skill,
            files: &files,
            args: &BTreeMap::new(),
            origin: Origin::Local {
                path: "/home/me/blog".to_string(),
            },
        };
        insta::assert_snapshot!("local_prompt", ctx.render());
    }

    #[test]
    fn retry_prompt_mentions_the_error() {
        let skill = skill(
            "proofread",
            "mode = \"review\"\ntools = [\"read\", \"write\"]",
        );
        let files = vec!["a.mdx".to_string()];
        let ctx = PromptContext {
            skill: &skill,
            files: &files,
            args: &BTreeMap::new(),
            origin: Origin::Local {
                path: "/tmp/repo".to_string(),
            },
        };
        let text = ctx.render_retry("expected value at line 1 column 1");
        assert!(text.contains("expected value at line 1 column 1"), "{text}");
        assert!(text.starts_with("/skill:proofread"), "{text}");
    }

    #[test]
    fn review_schema_snapshot() {
        insta::assert_snapshot!("review_schema", review_schema());
    }

    #[test]
    fn empty_file_list_is_explicit() {
        let skill = skill(
            "proofread",
            "mode = \"review\"\ntools = [\"read\", \"write\"]",
        );
        let ctx = PromptContext {
            skill: &skill,
            files: &[],
            args: &BTreeMap::new(),
            origin: Origin::Local {
                path: "/tmp/repo".to_string(),
            },
        };
        assert!(ctx.render().contains("(список пуст)"));
    }
}
