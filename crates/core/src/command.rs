//! Parser for commands of the form `/llm <skill> [key=value | positional args]`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Prefix a comment body has to start with.
pub const COMMAND_PREFIX: &str = "/llm";

/// A parsed command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    pub skill: String,
    #[serde(default)]
    pub args: Args,
}

/// Command arguments: positional and named.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Args {
    #[serde(default)]
    pub positional: Vec<String>,
    #[serde(default)]
    pub named: BTreeMap<String, String>,
}

impl Args {
    pub fn is_empty(&self) -> bool {
        self.positional.is_empty() && self.named.is_empty()
    }

    /// Value of a named argument.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.named.get(key).map(String::as_str)
    }
}

/// Why a comment body did not become a command.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommandParseError {
    /// The body does not start with `/llm` — such comments are ignored silently.
    #[error("not a command")]
    NotACommand,

    #[error("command is missing a skill name")]
    MissingSkill,

    #[error("invalid skill name: {0}")]
    InvalidSkillName(String),

    #[error("unterminated quote in arguments")]
    UnterminatedQuote,

    #[error("argument with an empty name: {0}")]
    EmptyArgName(String),

    #[error("duplicate argument: {0}")]
    DuplicateArg(String),
}

impl Command {
    /// Parses a comment body.
    ///
    /// The command must be on the first non-empty line; the rest of the text is
    /// ignored so that a human can add an explanation below.
    pub fn parse(body: &str) -> Result<Command, CommandParseError> {
        let line = body
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .ok_or(CommandParseError::NotACommand)?;

        let rest = match line.strip_prefix(COMMAND_PREFIX) {
            Some(rest) if rest.is_empty() || rest.starts_with(char::is_whitespace) => rest,
            _ => return Err(CommandParseError::NotACommand),
        };

        let tokens = tokenize(rest)?;
        let mut tokens = tokens.into_iter();
        let skill = tokens.next().ok_or(CommandParseError::MissingSkill)?;
        if !is_valid_skill_name(&skill) {
            return Err(CommandParseError::InvalidSkillName(skill));
        }

        let mut args = Args::default();
        for token in tokens {
            match split_named(&token) {
                Some((key, value)) => {
                    if key.is_empty() {
                        return Err(CommandParseError::EmptyArgName(token));
                    }
                    if args
                        .named
                        .insert(key.to_string(), value.to_string())
                        .is_some()
                    {
                        return Err(CommandParseError::DuplicateArg(key.to_string()));
                    }
                }
                None => args.positional.push(token),
            }
        }

        Ok(Command { skill, args })
    }

    /// Renders the command back to text — for logs and PR bodies.
    pub fn to_command_line(&self) -> String {
        let mut out = format!("{COMMAND_PREFIX} {}", self.skill);
        for value in &self.args.positional {
            out.push(' ');
            out.push_str(&quote_if_needed(value));
        }
        for (key, value) in &self.args.named {
            out.push(' ');
            out.push_str(key);
            out.push('=');
            out.push_str(&quote_if_needed(value));
        }
        out
    }
}

/// Splits a token into `key=value` when the key looks like an identifier.
fn split_named(token: &str) -> Option<(&str, &str)> {
    let (key, value) = token.split_once('=')?;
    if key.is_empty() {
        return Some((key, value));
    }
    let looks_like_key = key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    looks_like_key.then_some((key, value))
}

fn quote_if_needed(value: &str) -> String {
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

fn is_valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Splits a string into tokens, honouring single and double quotes.
fn tokenize(input: &str) -> Result<Vec<String>, CommandParseError> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    let mut quote: Option<char> = None;
    let mut escaped = false;

    for ch in input.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        match quote {
            Some(q) => {
                if ch == '\\' && q == '"' {
                    escaped = true;
                } else if ch == q {
                    quote = None;
                } else {
                    current.push(ch);
                }
            }
            None => {
                if ch == '"' || ch == '\'' {
                    quote = Some(ch);
                    has_token = true;
                } else if ch.is_whitespace() {
                    if has_token {
                        tokens.push(std::mem::take(&mut current));
                        has_token = false;
                    }
                } else {
                    current.push(ch);
                    has_token = true;
                }
            }
        }
    }

    if quote.is_some() || escaped {
        return Err(CommandParseError::UnterminatedQuote);
    }
    if has_token {
        tokens.push(current);
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_skill() {
        let cmd = Command::parse("/llm proofread").unwrap();
        assert_eq!(cmd.skill, "proofread");
        assert!(cmd.args.is_empty());
    }

    #[test]
    fn parses_positional_args() {
        let cmd = Command::parse("/llm translate en").unwrap();
        assert_eq!(cmd.skill, "translate");
        assert_eq!(cmd.args.positional, vec!["en".to_string()]);
    }

    #[test]
    fn parses_named_args() {
        let cmd = Command::parse("/llm translate lang=en style=formal").unwrap();
        assert_eq!(cmd.args.get("lang"), Some("en"));
        assert_eq!(cmd.args.get("style"), Some("formal"));
        assert!(cmd.args.positional.is_empty());
    }

    #[test]
    fn mixes_positional_and_named() {
        let cmd = Command::parse("/llm translate en tone=casual").unwrap();
        assert_eq!(cmd.args.positional, vec!["en".to_string()]);
        assert_eq!(cmd.args.get("tone"), Some("casual"));
    }

    #[test]
    fn handles_quoted_values() {
        let cmd = Command::parse(r#"/llm review focus="error handling" 'two words'"#).unwrap();
        assert_eq!(cmd.args.get("focus"), Some("error handling"));
        assert_eq!(cmd.args.positional, vec!["two words".to_string()]);
    }

    #[test]
    fn handles_escaped_quotes() {
        let cmd = Command::parse(r#"/llm review note="he said \"hi\"""#).unwrap();
        assert_eq!(cmd.args.get("note"), Some(r#"he said "hi""#));
    }

    #[test]
    fn ignores_text_after_the_command_line() {
        let cmd = Command::parse("/llm proofread\n\nplease, the intro only").unwrap();
        assert_eq!(cmd.skill, "proofread");
        assert!(cmd.args.is_empty());
    }

    #[test]
    fn skips_leading_blank_lines_and_spaces() {
        let cmd = Command::parse("\n   \n  /llm proofread  \r\n").unwrap();
        assert_eq!(cmd.skill, "proofread");
    }

    #[test]
    fn rejects_non_commands() {
        for body in [
            "just a comment",
            "",
            "   ",
            "llm proofread",
            "/llmproofread",
            "some text\n/llm proofread",
        ] {
            assert_eq!(
                Command::parse(body),
                Err(CommandParseError::NotACommand),
                "body = {body:?}"
            );
        }
    }

    #[test]
    fn rejects_missing_skill() {
        assert_eq!(Command::parse("/llm"), Err(CommandParseError::MissingSkill));
        assert_eq!(
            Command::parse("/llm   "),
            Err(CommandParseError::MissingSkill)
        );
    }

    #[test]
    fn rejects_bad_skill_name() {
        assert_eq!(
            Command::parse("/llm ../etc/passwd"),
            Err(CommandParseError::InvalidSkillName("../etc/passwd".into()))
        );
        assert_eq!(
            Command::parse("/llm -weird"),
            Err(CommandParseError::InvalidSkillName("-weird".into()))
        );
    }

    #[test]
    fn rejects_duplicate_named_args() {
        assert_eq!(
            Command::parse("/llm translate lang=en lang=de"),
            Err(CommandParseError::DuplicateArg("lang".into()))
        );
    }

    #[test]
    fn rejects_empty_arg_name() {
        assert_eq!(
            Command::parse("/llm translate =en"),
            Err(CommandParseError::EmptyArgName("=en".into()))
        );
    }

    #[test]
    fn rejects_unterminated_quote() {
        assert_eq!(
            Command::parse(r#"/llm review focus="oops"#),
            Err(CommandParseError::UnterminatedQuote)
        );
    }

    #[test]
    fn value_with_url_stays_positional_like() {
        // A value may contain '=' — we split on the first one.
        let cmd = Command::parse("/llm review url=https://x/y?a=b").unwrap();
        assert_eq!(cmd.args.get("url"), Some("https://x/y?a=b"));
    }

    #[test]
    fn token_that_is_not_a_key_stays_positional() {
        let cmd = Command::parse("/llm review a+b=c").unwrap();
        assert_eq!(cmd.args.positional, vec!["a+b=c".to_string()]);
    }

    #[test]
    fn command_line_roundtrip() {
        let cmd = Command::parse(r#"/llm translate en lang="ru RU""#).unwrap();
        assert_eq!(cmd.to_command_line(), r#"/llm translate en lang="ru RU""#);
        assert_eq!(Command::parse(&cmd.to_command_line()).unwrap(), cmd);
    }

    #[test]
    fn command_survives_json_roundtrip() {
        let cmd = Command::parse("/llm translate en tone=casual").unwrap();
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), cmd);
    }
}
