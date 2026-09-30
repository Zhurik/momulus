use assert_cmd::Command;
use predicates::str::contains;

/// The binary, started outside the project directory.
///
/// The service loads a `.env` from its working directory the way docker compose
/// does; without this the developer's own `.env` would leak into the tests.
fn momulus(cwd: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("momulus").unwrap();
    cmd.current_dir(cwd);
    cmd
}

#[test]
fn help_lists_subcommands() {
    Command::cargo_bin("momulus")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("serve"))
        .stdout(contains("run"))
        .stdout(contains("skills"));
}

#[test]
fn unknown_subcommand_fails() {
    Command::cargo_bin("momulus")
        .unwrap()
        .arg("nope")
        .assert()
        .failure();
}

/// A directory with one valid and one broken skill.
fn fixture_skills(dir: &std::path::Path, broken: bool) {
    std::fs::create_dir_all(dir).unwrap();
    let ok = dir.join("proofread");
    std::fs::create_dir_all(&ok).unwrap();
    std::fs::write(
        ok.join("skill.toml"),
        "mode = \"review\"\ntools = [\"read\", \"write\"]\nfiles = [\"**/*.mdx\"]\n",
    )
    .unwrap();
    std::fs::write(
        ok.join("SKILL.md"),
        "---\nname: proofread\ndescription: Proofreading posts\n---\n\nbody\n",
    )
    .unwrap();

    if broken {
        let bad = dir.join("translate");
        std::fs::create_dir_all(&bad).unwrap();
        // patch without tools — an invalid contract
        std::fs::write(bad.join("skill.toml"), "mode = \"patch\"\n").unwrap();
        std::fs::write(
            bad.join("SKILL.md"),
            "---\nname: translate\ndescription: Translation\n---\n",
        )
        .unwrap();
    }
}

#[test]
fn skills_list_prints_registry() {
    let dir = tempfile::tempdir().unwrap();
    fixture_skills(dir.path(), false);
    momulus(dir.path())
        .args([
            "--skills-dir",
            dir.path().to_str().unwrap(),
            "skills",
            "list",
        ])
        .assert()
        .success()
        .stdout(contains("proofread"))
        .stdout(contains("review"))
        .stdout(contains("Proofreading posts"));
}

#[test]
fn skills_validate_passes_on_good_contracts() {
    let dir = tempfile::tempdir().unwrap();
    fixture_skills(dir.path(), false);
    momulus(dir.path())
        .args([
            "--skills-dir",
            dir.path().to_str().unwrap(),
            "skills",
            "validate",
        ])
        .assert()
        .success()
        .stdout(contains("all contracts are valid: 1"));
}

#[test]
fn skills_validate_fails_on_broken_contract() {
    let dir = tempfile::tempdir().unwrap();
    fixture_skills(dir.path(), true);
    momulus(dir.path())
        .args([
            "--skills-dir",
            dir.path().to_str().unwrap(),
            "skills",
            "validate",
        ])
        .assert()
        .failure()
        .stderr(contains("translate"))
        .stderr(contains("tools"));
}

#[test]
fn skills_validate_reports_missing_directory() {
    let tmp = tempfile::tempdir().unwrap();
    momulus(tmp.path())
        .args(["--skills-dir", "/definitely/not/here", "skills", "validate"])
        .assert()
        .failure()
        .stderr(contains("cannot be read"));
}

#[test]
fn bundled_skills_are_valid() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills");
    let tmp = tempfile::tempdir().unwrap();
    momulus(tmp.path())
        .args(["--skills-dir", root.to_str().unwrap(), "skills", "validate"])
        .assert()
        .success()
        .stdout(contains("proofread"))
        .stdout(contains("translate"))
        .stdout(contains("review"));
}

/// A tiny repository with a post, plus a directory of prepared artifacts.
fn fixture_repo(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("posts")).unwrap();
    std::fs::write(dir.join("posts/hello.mdx"), "# Hello\n\ntext with a tpyo\n").unwrap();
    std::fs::write(dir.join("Cargo.toml"), "outside the filter\n").unwrap();
}

#[test]
fn run_with_fake_runner_prints_findings() {
    let tmp = tempfile::tempdir().unwrap();
    let skills = tmp.path().join("skills");
    fixture_skills(&skills, false);
    let repo = tmp.path().join("repo");
    fixture_repo(&repo);

    let prepared = tmp.path().join("prepared");
    std::fs::create_dir_all(&prepared).unwrap();
    std::fs::write(
        prepared.join("findings.json"),
        r#"{"summary":"one typo","findings":[{"path":"posts/hello.mdx","line":3,"severity":"typo","body":"tpyo -> typo"}]}"#,
    )
    .unwrap();

    let out = tmp.path().join("out");
    momulus(tmp.path())
        .args([
            "--skills-dir",
            skills.to_str().unwrap(),
            "run",
            "--repo-path",
            repo.to_str().unwrap(),
            "--skill",
            "proofread",
            "--out",
            out.to_str().unwrap(),
            "--fake-runner",
            prepared.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(contains("skill proofread (review)"))
        .stdout(contains("files: 1"))
        .stdout(contains("tpyo -> typo"));

    assert!(out.join("findings.json").exists());
    assert!(out.join("pi.log").exists(), "the pi log was stored");
}

#[test]
fn run_reports_when_nothing_matches_the_filter() {
    let tmp = tempfile::tempdir().unwrap();
    let skills = tmp.path().join("skills");
    fixture_skills(&skills, false);
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("main.rs"), "fn main() {}\n").unwrap();

    momulus(tmp.path())
        .args([
            "--skills-dir",
            skills.to_str().unwrap(),
            "run",
            "--repo-path",
            repo.to_str().unwrap(),
            "--skill",
            "proofread",
            "--fake-runner",
            tmp.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(contains("nothing to do"));
}

#[test]
fn run_rejects_unknown_skill() {
    let tmp = tempfile::tempdir().unwrap();
    let skills = tmp.path().join("skills");
    fixture_skills(&skills, false);
    let repo = tmp.path().join("repo");
    fixture_repo(&repo);

    momulus(tmp.path())
        .args([
            "--skills-dir",
            skills.to_str().unwrap(),
            "run",
            "--repo-path",
            repo.to_str().unwrap(),
            "--skill",
            "no-such-skill",
            "--fake-runner",
            tmp.path().to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(contains("unknown skill"));
}

#[test]
fn run_fails_when_skill_produced_no_findings_file() {
    let tmp = tempfile::tempdir().unwrap();
    let skills = tmp.path().join("skills");
    fixture_skills(&skills, false);
    let repo = tmp.path().join("repo");
    fixture_repo(&repo);
    let empty = tmp.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();

    momulus(tmp.path())
        .args([
            "--skills-dir",
            skills.to_str().unwrap(),
            "run",
            "--repo-path",
            repo.to_str().unwrap(),
            "--skill",
            "proofread",
            "--out",
            tmp.path().join("out").to_str().unwrap(),
            "--fake-runner",
            empty.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(contains("findings.json"));
}

#[test]
fn serve_reports_missing_config() {
    let tmp = tempfile::tempdir().unwrap();
    momulus(tmp.path())
        .args(["--config", "/definitely/not/here.toml", "serve"])
        .assert()
        .failure()
        .stderr(contains("config"));
}

#[test]
fn serve_reports_missing_github_secrets() {
    let tmp = tempfile::tempdir().unwrap();
    let skills = tmp.path().join("skills");
    fixture_skills(&skills, false);
    let config = tmp.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            "allowed_users = [\"zhurik\"]\ndata_dir = {:?}\n",
            tmp.path().join("data").to_string_lossy()
        ),
    )
    .unwrap();

    momulus(tmp.path())
        .args([
            "--config",
            config.to_str().unwrap(),
            "--skills-dir",
            skills.to_str().unwrap(),
            "serve",
        ])
        // The secret variables are deliberately empty.
        .env_remove("GITHUB_TOKEN")
        .env_remove("GITHUB_APP_ID")
        .env_remove("GITHUB_APP_PRIVATE_KEY_PATH")
        .env("LLM_API_KEY", "test-key")
        .env("LLM_BASE_URL", "https://openrouter.ai/api/v1")
        .assert()
        .failure()
        .stderr(contains("GITHUB_APP_ID"));
}

#[test]
fn serve_requires_base_url_for_custom_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let skills = tmp.path().join("skills");
    fixture_skills(&skills, false);
    let config = tmp.path().join("config.toml");
    // A provider pi does not know needs an endpoint to talk to.
    std::fs::write(
        &config,
        format!(
            "allowed_users = [\"zhurik\"]\ndata_dir = {:?}\n\n             [llm]\ndefault_provider = \"my-gateway\"\napi = \"openai-completions\"\n",
            tmp.path().join("data").to_string_lossy()
        ),
    )
    .unwrap();

    momulus(tmp.path())
        .args([
            "--config",
            config.to_str().unwrap(),
            "--skills-dir",
            skills.to_str().unwrap(),
            "serve",
        ])
        .env_remove("LLM_BASE_URL")
        .assert()
        .failure()
        .stderr(contains("LLM_BASE_URL"));
}

#[test]
fn serve_accepts_a_builtin_provider_without_base_url() {
    let tmp = tempfile::tempdir().unwrap();
    let skills = tmp.path().join("skills");
    fixture_skills(&skills, false);
    let config = tmp.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            "allowed_users = [\"zhurik\"]\ndata_dir = {:?}\n",
            tmp.path().join("data").to_string_lossy()
        ),
    )
    .unwrap();

    // The default provider is built into pi, so the run gets as far as the
    // missing GitHub credentials rather than complaining about the endpoint.
    momulus(tmp.path())
        .args([
            "--config",
            config.to_str().unwrap(),
            "--skills-dir",
            skills.to_str().unwrap(),
            "serve",
        ])
        .env_remove("LLM_BASE_URL")
        .env_remove("GITHUB_TOKEN")
        .env_remove("GITHUB_APP_ID")
        .env("LLM_API_KEY", "test-key")
        .assert()
        .failure()
        .stderr(contains("GITHUB_APP_ID"));
}

/// A config with the given extra lines and a temporary data directory.
fn config_with(tmp: &std::path::Path, extra: &str) -> std::path::PathBuf {
    let path = tmp.join("config.toml");
    std::fs::write(
        &path,
        format!(
            "allowed_users = [\"zhurik\"]\ndata_dir = {:?}\n{extra}",
            tmp.join("data").to_string_lossy()
        ),
    )
    .unwrap();
    path
}

#[test]
fn serve_without_any_github_credentials_lists_both_ways() {
    let tmp = tempfile::tempdir().unwrap();
    let skills = tmp.path().join("skills");
    fixture_skills(&skills, false);
    let config = config_with(tmp.path(), "");

    momulus(tmp.path())
        .args([
            "--config",
            config.to_str().unwrap(),
            "--skills-dir",
            skills.to_str().unwrap(),
            "serve",
        ])
        .env_remove("GITHUB_TOKEN")
        .env_remove("GITHUB_APP_ID")
        .env_remove("GITHUB_APP_PRIVATE_KEY_PATH")
        .env("LLM_API_KEY", "test-key")
        .assert()
        .failure()
        .stderr(contains("GITHUB_TOKEN"))
        .stderr(contains("GITHUB_APP_ID"));
}

#[test]
fn serve_with_a_token_requires_the_repo_allowlist() {
    let tmp = tempfile::tempdir().unwrap();
    let skills = tmp.path().join("skills");
    fixture_skills(&skills, false);
    // No `repos` in the config: a token cannot discover repositories itself.
    let config = config_with(tmp.path(), "");

    momulus(tmp.path())
        .args([
            "--config",
            config.to_str().unwrap(),
            "--skills-dir",
            skills.to_str().unwrap(),
            "serve",
        ])
        .env("GITHUB_TOKEN", "ghp_test_token")
        .env("LLM_API_KEY", "test-key")
        .assert()
        .failure()
        .stderr(contains("repos"));
}

#[test]
fn serve_with_half_an_app_says_which_half_is_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let skills = tmp.path().join("skills");
    fixture_skills(&skills, false);
    let config = config_with(tmp.path(), "");

    momulus(tmp.path())
        .args([
            "--config",
            config.to_str().unwrap(),
            "--skills-dir",
            skills.to_str().unwrap(),
            "serve",
        ])
        .env_remove("GITHUB_TOKEN")
        .env_remove("GITHUB_APP_PRIVATE_KEY_PATH")
        .env("GITHUB_APP_ID", "123456")
        .env("LLM_API_KEY", "test-key")
        .assert()
        .failure()
        .stderr(contains("GITHUB_APP_PRIVATE_KEY_PATH"));
}

#[test]
fn env_overrides_the_data_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let skills = tmp.path().join("skills");
    fixture_skills(&skills, false);
    // The config points at a path that does not exist; the override wins, so the
    // run gets as far as the missing credentials instead of a filesystem error.
    let config = config_with(tmp.path(), "");
    std::fs::write(
        &config,
        "allowed_users = [\"zhurik\"]\ndata_dir = \"/definitely/not/writable\"\n",
    )
    .unwrap();

    momulus(tmp.path())
        .args([
            "--config",
            config.to_str().unwrap(),
            "--skills-dir",
            skills.to_str().unwrap(),
            "serve",
        ])
        .env(
            "MOMULUS_DATA_DIR",
            tmp.path().join("data").to_str().unwrap(),
        )
        .env_remove("GITHUB_TOKEN")
        .env_remove("GITHUB_APP_ID")
        .env_remove("GITHUB_APP_PRIVATE_KEY_PATH")
        .env("LLM_API_KEY", "test-key")
        .assert()
        .failure()
        .stderr(contains("GITHUB_TOKEN"));
}
