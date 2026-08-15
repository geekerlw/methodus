use std::fs;
use std::process::Command;

use assert_cmd::cargo::cargo_bin;
use tempfile::tempdir;

fn methodus() -> Command {
    Command::new(cargo_bin!("methodus"))
}

fn methodus_with_home(home: &std::path::Path) -> Command {
    let mut cmd = methodus();
    cmd.env("METHODUS_HOME", home);
    cmd
}

#[test]
fn init_creates_home_and_is_idempotent() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("mh");

    let out = methodus_with_home(&home).arg("init").output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(home.join("state.db").exists());
    assert!(home.join("faces/general/face.yaml").exists());
    assert!(home.join("methods/general-software.yaml").exists());
    assert!(home.join("skills/workspace-hygiene/SKILL.md").exists());
    assert!(home.join("config.yaml").exists());

    let out = methodus_with_home(&home).arg("init").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("already initialized"));
}

#[test]
fn task_create_and_list() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("mh");
    assert!(methodus_with_home(&home)
        .arg("init")
        .status()
        .unwrap()
        .success());

    let out = methodus_with_home(&home)
        .args(["task", "create", "fix the latch"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(id.starts_with("task_"));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("resolved face=general"));
    assert!(stderr.contains("method=general-software"));
    assert!(stderr.contains("low-confidence"));

    let out = methodus_with_home(&home)
        .args(["task", "list"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(&id));
    assert!(stdout.contains("queued"));

    let yaml = fs::read_to_string(home.join("faces/general/face.yaml")).unwrap();
    assert!(yaml.contains("id: general"));

    let show = methodus_with_home(&home)
        .args(["task", "show", &id])
        .output()
        .unwrap();
    let shown = String::from_utf8_lossy(&show.stdout);
    assert!(shown.contains("general-software"));
    assert!(shown.contains("workspace-hygiene"));
}

#[test]
fn run_without_init_fails() {
    let dir = tempdir().unwrap();
    let out = methodus()
        .env("METHODUS_HOME", dir.path())
        .args(["run", "task_doesnotexist"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[test]
fn events_tail_after_init_is_empty() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("mh");
    assert!(methodus_with_home(&home)
        .arg("init")
        .status()
        .unwrap()
        .success());
    let out = methodus_with_home(&home)
        .args(["events", "tail"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
