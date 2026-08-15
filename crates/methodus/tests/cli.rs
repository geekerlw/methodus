use std::process::Command;

use assert_cmd::cargo::cargo_bin;

fn methodus() -> Command {
    Command::new(cargo_bin!("methodus"))
}

#[test]
fn help_describes_tui_not_subcommands() {
    let out = methodus().arg("--help").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout).to_lowercase();
    assert!(stdout.contains("tui") || stdout.contains("setup"));
    assert!(!stdout.contains("methodus init"));
    assert!(!stdout.contains("task create"));
}

#[test]
fn extra_args_are_rejected() {
    let out = methodus().arg("init").output().unwrap();
    assert!(!out.status.success());
    let err = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
    .to_lowercase();
    assert!(
        err.contains("unexpected") || err.contains("unrecognized") || err.contains("error"),
        "{err}"
    );
}
