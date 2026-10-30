//! Binary-level tests for config errors. These all exit with code 4 before any
//! GitHub auth or network access.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const EXIT_CONFIG: i32 = 4;

fn unique_dir(test_name: &str) -> PathBuf {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("pr-bro-test-cli-{}-{}", test_name, timestamp));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Run pr-bro with HOME and caches pointed into `dir`, no token, no terminal.
fn run(dir: &PathBuf, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pr-bro"))
        .args(args)
        .arg("--no-version-check")
        .current_dir(dir)
        .env("HOME", dir)
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env_remove("PR_BRO_GH_TOKEN")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn missing_file_among_several_configs_exits_4_naming_it() {
    let dir = unique_dir("missing");
    std::fs::write(dir.join("team.yaml"), "queries: [{query: q}]").unwrap();
    let out = run(&dir, &["--config", "team.yaml", "--config", "local.yaml"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(out.status.code(), Some(EXIT_CONFIG), "{}", stderr);
    assert!(stderr.contains("local.yaml"), "{}", stderr);
}

#[test]
fn init_with_several_configs_exits_4() {
    let dir = unique_dir("init");
    let out = run(&dir, &["init", "--config", "a.yaml", "--config", "b.yaml"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(out.status.code(), Some(EXIT_CONFIG), "{}", stderr);
    assert!(stderr.contains("at most one --config"), "{}", stderr);
}

#[test]
fn unknown_key_in_later_config_exits_4_naming_it() {
    let dir = unique_dir("unknown");
    std::fs::write(dir.join("team.yaml"), "queries: [{query: q}]").unwrap();
    std::fs::write(dir.join("local.yaml"), "thme: dark").unwrap();
    let out = run(&dir, &["--config", "team.yaml", "--config", "local.yaml"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(out.status.code(), Some(EXIT_CONFIG), "{}", stderr);
    assert!(stderr.contains("local.yaml"), "{}", stderr);
    assert!(stderr.contains("unknown field `thme`"), "{}", stderr);
}

#[test]
fn configs_before_and_after_subcommand_are_all_loaded() {
    let dir = unique_dir("split");
    std::fs::write(dir.join("local.yaml"), "queries: [{query: q}]").unwrap();
    let out = run(
        &dir,
        &["--config", "team.yaml", "list", "--config", "local.yaml"],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(out.status.code(), Some(EXIT_CONFIG), "{}", stderr);
    assert!(stderr.contains("team.yaml"), "{}", stderr);
}
