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
