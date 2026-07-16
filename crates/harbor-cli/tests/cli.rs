//! Integration tests for the `harbor` CLI command surface (H-002).
//!
//! These tests exercise the compiled binary directly rather than calling
//! into library code, since the goal of this task is to verify the clap
//! wiring (subcommands, flags, exit codes) rather than command behavior.

use std::process::Command;

fn harbor() -> Command {
    Command::new(env!("CARGO_BIN_EXE_harbor"))
}

#[test]
fn help_lists_all_nine_commands() {
    let output = harbor().arg("--help").output().expect("run harbor --help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    for cmd in [
        "init", "push", "run", "add", "list", "rm", "login", "logout", "yank",
    ] {
        assert!(
            stdout.contains(cmd),
            "--help output missing command `{cmd}`:\n{stdout}"
        );
    }
}

#[test]
fn version_flag_prints_binary_version() {
    let output = harbor().arg("-V").output().expect("run harbor -V");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")));

    // Long form should behave the same way.
    let output_long = harbor().arg("--version").output().expect("run harbor --version");
    assert!(output_long.status.success());
    let stdout_long = String::from_utf8_lossy(&output_long.stdout);
    assert!(stdout_long.contains(env!("CARGO_PKG_VERSION")));
}

/// Each not-yet-implemented subcommand, given minimal valid arguments, should
/// currently exit non-zero and mention "not implemented". (`init` is
/// implemented and covered in tests/init.rs.)
#[test]
fn each_subcommand_reports_not_implemented() {
    let cases: &[&[&str]] = &[
        &["push"],
        &["run", "owner/package"],
        &["add", "https://github.com/owner/repo"],
        &["list"],
        &["rm", "owner/package"],
        &["login"],
        &["logout"],
        &["yank", "owner/package", "--version", "1.2.0"],
    ];

    // Run in a tempdir so no command can leave stray files in the repo.
    let dir = tempfile::tempdir().expect("create tempdir");
    for args in cases {
        let output = harbor()
            .current_dir(dir.path())
            .args(*args)
            .output()
            .expect("run harbor subcommand");
        assert!(
            !output.status.success(),
            "expected non-zero exit for {args:?}, got success"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("not implemented"),
            "expected 'not implemented' in stderr for {args:?}, got:\n{stderr}"
        );
    }
}

#[test]
fn rm_all_conflicts_with_target() {
    let output = harbor()
        .args(["rm", "owner/package", "--all"])
        .output()
        .expect("run harbor rm owner/package --all");
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2), "clap usage errors exit with code 2");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot be used with"), "stderr:\n{stderr}");
}

#[test]
fn rm_env_requires_target() {
    let output = harbor()
        .args(["rm", "--env"])
        .output()
        .expect("run harbor rm --env");
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn rm_all_alone_is_accepted_by_clap() {
    // --all with no target is valid at the parser level; the command itself
    // isn't implemented yet, so it should still fail, but via the
    // "not implemented" path (exit 1), not a clap usage error (exit 2).
    let output = harbor().args(["rm", "--all"]).output().expect("run harbor rm --all");
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not implemented"));
}

#[test]
fn yank_without_version_fails_with_clap_usage_error() {
    let output = harbor()
        .args(["yank", "owner/package"])
        .output()
        .expect("run harbor yank owner/package");
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2), "clap usage errors exit with code 2");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--version"), "stderr:\n{stderr}");
}

#[test]
fn yank_undo_flag_is_accepted() {
    let output = harbor()
        .args(["yank", "owner/package", "--version", "1.2.0", "--undo"])
        .output()
        .expect("run harbor yank ... --undo");
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn no_subcommand_is_a_usage_error() {
    let output = harbor().output().expect("run harbor with no args");
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
}
