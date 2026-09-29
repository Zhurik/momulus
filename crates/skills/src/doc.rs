//! Parsing `SKILL.md` — instructions for pi in the Agent Skills format.

use crate::error::{SkillError, SkillResult};

/// Frontmatter and body of SKILL.md.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDoc {
    pub name: String,
    pub description: String,
    /// The text after the frontmatter.
    pub body: String,
}

impl SkillDoc {
    /// Parses the file: YAML frontmatter with `name` and `description`, then the text.
    pub fn parse(skill: &str, text: &str) -> SkillResult<SkillDoc> {
        let fail = |message: &str| SkillError::Doc {
            skill: skill.to_string(),
            message: message.to_string(),
        };

        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let rest = text
            .strip_prefix("---\n")
            .or_else(|| text.strip_prefix("---\r\n"))
            .ok_or_else(|| fail("no frontmatter: the file must start with a --- line"))?;

        let (frontmatter, body) = split_frontmatter(rest)
            .ok_or_else(|| fail("frontmatter is not closed by a --- line"))?;

        let mut name = None;
        let mut description = None;
        for line in frontmatter.lines() {
            let line = line.trim_end();
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once(':') else {
                return Err(fail(&format!("unparseable frontmatter line: {line:?}")));
            };
            let value = unquote(value.trim());
            match key.trim() {
                "name" => name = Some(value),
                "description" => description = Some(value),
                _ => {} // Other frontmatter keys are none of our business.
            }
        }

        let name = name.ok_or_else(|| fail("frontmatter has no name field"))?;
        let description =
            description.ok_or_else(|| fail("frontmatter has no description field"))?;
        if name.is_empty() {
            return Err(fail("the name field is empty"));
        }
        if description.is_empty() {
            return Err(fail("the description field is empty"));
        }
        if name != skill {
            return Err(fail(&format!(
                "name = {name:?} does not match the directory name {skill:?}"
            )));
        }

        Ok(SkillDoc {
            name,
            description,
            body: body.to_string(),
        })
    }
}

/// Splits the text after the opening `---` into frontmatter and body.
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

    const DOC: &str = "---\nname: proofread\ndescription: Proofreading MDX posts\n---\n\n# Proofread\n\nInstructions.\n";

    #[test]
    fn parses_frontmatter_and_body() {
        let doc = SkillDoc::parse("proofread", DOC).unwrap();
        assert_eq!(doc.name, "proofread");
        assert_eq!(doc.description, "Proofreading MDX posts");
        assert!(doc.body.contains("# Proofread"));
    }

    #[test]
    fn accepts_quoted_values_and_extra_keys() {
        let doc = SkillDoc::parse(
            "review",
            "---\nname: \"review\"\nlicense: MIT\ndescription: 'Code review'\n---\nbody\n",
        )
        .unwrap();
        assert_eq!(doc.name, "review");
        assert_eq!(doc.description, "Code review");
    }

    #[test]
    fn requires_frontmatter() {
        let err = SkillDoc::parse("x", "# Heading\n").unwrap_err();
        assert!(err.to_string().contains("frontmatter"), "{err}");
    }

    #[test]
    fn requires_closing_delimiter() {
        let err = SkillDoc::parse("x", "---\nname: x\ndescription: y\n").unwrap_err();
        assert!(err.to_string().contains("not closed"), "{err}");
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
        assert!(err.to_string().contains("does not match"), "{err}");
    }

    #[test]
    fn handles_crlf_and_bom() {
        let doc = SkillDoc::parse(
            "x",
            "\u{feff}---\r\nname: x\r\ndescription: y\r\n---\r\nbody\r\n",
        )
        .unwrap();
        assert_eq!(doc.description, "y");
    }
}
