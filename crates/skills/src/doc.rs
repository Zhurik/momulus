//! Разбор `SKILL.md` — инструкции для pi в формате Agent Skills.

use crate::error::{SkillError, SkillResult};

/// Frontmatter и тело SKILL.md.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDoc {
    pub name: String,
    pub description: String,
    /// Текст после frontmatter.
    pub body: String,
}

impl SkillDoc {
    /// Разбирает файл: YAML-frontmatter с `name` и `description`, дальше — текст.
    pub fn parse(skill: &str, text: &str) -> SkillResult<SkillDoc> {
        let fail = |message: &str| SkillError::Doc {
            skill: skill.to_string(),
            message: message.to_string(),
        };

        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let rest = text
            .strip_prefix("---\n")
            .or_else(|| text.strip_prefix("---\r\n"))
            .ok_or_else(|| fail("нет frontmatter: файл должен начинаться со строки ---"))?;

        let (frontmatter, body) =
            split_frontmatter(rest).ok_or_else(|| fail("frontmatter не закрыт строкой ---"))?;

        let mut name = None;
        let mut description = None;
        for line in frontmatter.lines() {
            let line = line.trim_end();
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once(':') else {
                return Err(fail(&format!("непонятная строка frontmatter: {line:?}")));
            };
            let value = unquote(value.trim());
            match key.trim() {
                "name" => name = Some(value),
                "description" => description = Some(value),
                _ => {} // Остальные ключи frontmatter нас не касаются.
            }
        }

        let name = name.ok_or_else(|| fail("во frontmatter нет поля name"))?;
        let description = description.ok_or_else(|| fail("во frontmatter нет поля description"))?;
        if name.is_empty() {
            return Err(fail("поле name пустое"));
        }
        if description.is_empty() {
            return Err(fail("поле description пустое"));
        }
        if name != skill {
            return Err(fail(&format!(
                "name = {name:?} не совпадает с именем каталога {skill:?}"
            )));
        }

        Ok(SkillDoc {
            name,
            description,
            body: body.to_string(),
        })
    }
}

/// Делит текст после открывающего `---` на frontmatter и тело.
fn split_frontmatter(text: &str) -> Option<(&str, &str)> {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return Some((&text[..offset], &text[offset + line.len()..]));
        }
        offset += line.len();
    }
    None
}

fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    for quote in ['"', '\''] {
        if trimmed.len() >= 2 && trimmed.starts_with(quote) && trimmed.ends_with(quote) {
            return trimmed[1..trimmed.len() - 1].to_string();
        }
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "---\nname: proofread\ndescription: Вычитка MDX-статей\n---\n\n# Proofread\n\nИнструкции.\n";

    #[test]
    fn parses_frontmatter_and_body() {
        let doc = SkillDoc::parse("proofread", DOC).unwrap();
        assert_eq!(doc.name, "proofread");
        assert_eq!(doc.description, "Вычитка MDX-статей");
        assert!(doc.body.contains("# Proofread"));
    }

    #[test]
    fn accepts_quoted_values_and_extra_keys() {
        let doc = SkillDoc::parse(
            "review",
            "---\nname: \"review\"\nlicense: MIT\ndescription: 'Ревью кода'\n---\nтело\n",
        )
        .unwrap();
        assert_eq!(doc.name, "review");
        assert_eq!(doc.description, "Ревью кода");
    }

    #[test]
    fn requires_frontmatter() {
        let err = SkillDoc::parse("x", "# Заголовок\n").unwrap_err();
        assert!(err.to_string().contains("frontmatter"), "{err}");
    }

    #[test]
    fn requires_closing_delimiter() {
        let err = SkillDoc::parse("x", "---\nname: x\ndescription: y\n").unwrap_err();
        assert!(err.to_string().contains("не закрыт"), "{err}");
    }

    #[test]
    fn requires_name_and_description() {
        let err = SkillDoc::parse("x", "---\nname: x\n---\n").unwrap_err();
        assert!(err.to_string().contains("description"), "{err}");
        let err = SkillDoc::parse("x", "---\ndescription: y\n---\n").unwrap_err();
        assert!(err.to_string().contains("name"), "{err}");
    }

    #[test]
    fn name_must_match_directory() {
        let err =
            SkillDoc::parse("proofread", "---\nname: other\ndescription: y\n---\n").unwrap_err();
        assert!(err.to_string().contains("не совпадает"), "{err}");
    }

    #[test]
    fn handles_crlf_and_bom() {
        let doc = SkillDoc::parse(
            "x",
            "\u{feff}---\r\nname: x\r\ndescription: y\r\n---\r\nтело\r\n",
        )
        .unwrap();
        assert_eq!(doc.description, "y");
    }
}
