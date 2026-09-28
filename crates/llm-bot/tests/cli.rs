use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn help_lists_subcommands() {
    Command::cargo_bin("llm-bot")
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
    Command::cargo_bin("llm-bot")
        .unwrap()
        .arg("nope")
        .assert()
        .failure();
}

/// Каталог с одним валидным и одним битым скиллом.
fn fixture_skills(dir: &std::path::Path, broken: bool) {
    let ok = dir.join("proofread");
    std::fs::create_dir_all(&ok).unwrap();
    std::fs::write(
        ok.join("skill.toml"),
        "mode = \"review\"\ntools = [\"read\"]\nfiles = [\"**/*.mdx\"]\n",
    )
    .unwrap();
    std::fs::write(
        ok.join("SKILL.md"),
        "---\nname: proofread\ndescription: Вычитка статей\n---\n\nтело\n",
    )
    .unwrap();

    if broken {
        let bad = dir.join("translate");
        std::fs::create_dir_all(&bad).unwrap();
        // patch без tools — контракт невалиден
        std::fs::write(bad.join("skill.toml"), "mode = \"patch\"\n").unwrap();
        std::fs::write(
            bad.join("SKILL.md"),
            "---\nname: translate\ndescription: Перевод\n---\n",
        )
        .unwrap();
    }
}

#[test]
fn skills_list_prints_registry() {
    let dir = tempfile::tempdir().unwrap();
    fixture_skills(dir.path(), false);
    Command::cargo_bin("llm-bot")
        .unwrap()
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
        .stdout(contains("Вычитка статей"));
}

#[test]
fn skills_validate_passes_on_good_contracts() {
    let dir = tempfile::tempdir().unwrap();
    fixture_skills(dir.path(), false);
    Command::cargo_bin("llm-bot")
        .unwrap()
        .args([
            "--skills-dir",
            dir.path().to_str().unwrap(),
            "skills",
            "validate",
        ])
        .assert()
        .success()
        .stdout(contains("все контракты валидны: 1"));
}

#[test]
fn skills_validate_fails_on_broken_contract() {
    let dir = tempfile::tempdir().unwrap();
    fixture_skills(dir.path(), true);
    Command::cargo_bin("llm-bot")
        .unwrap()
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
    Command::cargo_bin("llm-bot")
        .unwrap()
        .args(["--skills-dir", "/definitely/not/here", "skills", "validate"])
        .assert()
        .failure()
        .stderr(contains("не читается"));
}

#[test]
fn bundled_skills_are_valid() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills");
    Command::cargo_bin("llm-bot")
        .unwrap()
        .args(["--skills-dir", root.to_str().unwrap(), "skills", "validate"])
        .assert()
        .success()
        .stdout(contains("proofread"))
        .stdout(contains("translate"))
        .stdout(contains("review"));
}
