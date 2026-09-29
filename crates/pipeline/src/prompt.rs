//! Prompt template for pi: the skill invocation plus the output contract.

use std::collections::BTreeMap;

use momulus_core::ReviewOutput;
use momulus_skills::{Mode, Skill};

/// Where the working copy is mounted inside the container.
pub const WORK_DIR: &str = "/work";
/// Where the container stores its artifacts.
pub const OUT_DIR: &str = "/out";
/// Name of the file holding review findings.
pub const FINDINGS_FILE: &str = "findings.json";
/// Name of the file holding the patch summary.
pub const SUMMARY_FILE: &str = "summary.md";

/// Where the skill was started from — this only affects the prompt's preamble.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// A job triggered by a command in a PR.
    PullRequest { repo: String, number: u64 },
    /// A local run through `momulus run`.
    Local { path: String },
}

/// Everything needed to render a prompt.
#[derive(Debug, Clone)]
pub struct PromptContext<'a> {
    pub skill: &'a Skill,
    /// Files the skill works on (paths relative to the repository root).
    pub files: &'a [String],
    /// Command arguments, already mapped onto the contract.
    pub args: &'a BTreeMap<String, String>,
    pub origin: Origin,
}

impl PromptContext<'_> {
    /// The main prompt.
    pub fn render(&self) -> String {
        let mut out = String::new();

        // Invoke the skill explicitly: we do not rely on automatic selection.
        out.push_str(&format!("/skill:{}\n\n", self.skill.name));

        match &self.origin {
            Origin::PullRequest { repo, number } => {
                out.push_str(&format!(
                    "Context: pull request #{number} in the repository {repo}.\n"
                ));
            }
            Origin::Local { path } => {
                out.push_str(&format!("Context: a local run on a copy of {path}.\n"));
            }
        }
        out.push_str(&format!(
            "The working copy is mounted at {WORK_DIR}. Every path below is relative to {WORK_DIR}.\n\n"
        ));

        out.push_str("Files to work on:\n");
        if self.files.is_empty() {
            out.push_str("- (the list is empty)\n");
        } else {
            for file in self.files {
                out.push_str(&format!("- {file}\n"));
            }
        }

        if !self.args.is_empty() {
            out.push_str("\nCommand arguments:\n");
            for (key, value) in self.args {
                out.push_str(&format!("- {key} = {value}\n"));
            }
        }

        if !self.skill.contract.vars.is_empty() {
            out.push_str("\nSkill parameters:\n");
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

    /// The retry prompt after invalid JSON: we tell the model what exactly broke.
    pub fn render_retry(&self, error: &str) -> String {
        format!(
            "{}\n\nThe previous attempt failed validation: {}\n\
             Re-read the format requirements above and write {OUT_DIR}/{FINDINGS_FILE} again. \
             The file must contain JSON only — no markdown fences, no comments, no prose around it.\n",
            self.render(),
            error.trim()
        )
    }

    fn review_contract(&self) -> String {
        let max = self.skill.contract.max_comments();
        format!(
            "## What to return\n\n\
             Use the write tool to store the result in {OUT_DIR}/{FINDINGS_FILE}.\n\
             That file is the only artifact; nothing structured needs to go to stdout.\n\
             The working copy at {WORK_DIR} is mounted read-only — do not change anything there.\n\n\
             The file must be valid JSON matching this JSON Schema:\n\n\
             ```json\n{schema}\n```\n\n\
             Rules:\n\
             - `path` — a path relative to {WORK_DIR}, exactly as in the file list above;\n\
             - `line` — the line number in the current (new) version of the file, starting at 1;\n\
             - `severity` — one of the values from the schema;\n\
             - `body` — a short note: what is wrong and what to do about it. Write it in the\n\
             \u{a0} same language as the file you are reviewing;\n\
             - `suggestion` — an optional replacement for the whole line, with no markdown\n\
             \u{a0} fence and no line number;\n\
             - `summary` — 2–5 sentences: the overall picture plus anything that did not fit\n\
             \u{a0} into individual findings;\n\
             - at most {max} findings; if you have more, keep the important ones and mention\n\
             \u{a0} the rest in `summary`;\n\
             - if there is nothing to report, return an empty `findings` array and say so in\n\
             \u{a0} `summary`.\n\n\
             Do not write any other file and do not modify anything under {WORK_DIR}.\n",
            schema = review_schema(),
            max = max,
        )
    }

    fn patch_contract(&self) -> String {
        format!(
            "## What to do\n\n\
             Edit the files directly under {WORK_DIR}. When you are done:\n\n\
             - write a short summary of the changes to {OUT_DIR}/{SUMMARY_FILE}\n\
             \u{a0} (what you changed and why, what you deliberately left alone);\n\
             - do not run git commands: the orchestrator handles the commit, the branch\n\
             \u{a0} and the pull request;\n\
             - do not touch anything outside {WORK_DIR}, except {OUT_DIR}/{SUMMARY_FILE};\n\
             - if there is nothing to change, change nothing and say so in\n\
             \u{a0} {OUT_DIR}/{SUMMARY_FILE}.\n"
        )
    }
}

/// The JSON Schema of the expected review output — the very same schema we hand
/// to the model inside the prompt.
pub fn review_schema() -> String {
    let schema = schemars::schema_for!(ReviewOutput);
    serde_json::to_string_pretty(&schema).expect("the schema serializes")
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
                description: format!("description of {name}"),
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
        assert!(ctx.render().contains("(the list is empty)"));
    }
}
