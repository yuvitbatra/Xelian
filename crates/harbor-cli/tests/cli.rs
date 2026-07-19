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

/// Commands that previously returned "not implemented" are now fully wired
/// (Phase 16/17). `harbor run owner/package` attempts registry resolution
/// and fails with a network/registry error (no registry running) — not a
/// "not implemented" stub. `harbor yank` fails with a credential error.
#[test]
fn registry_run_fails_with_network_error_when_no_registry() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let output = harbor()
        .current_dir(dir.path())
        .args(["run", "owner/package"])
        .env("HOME", dir.path())
        .env("HARBOR_REGISTRY_URL", "http://127.0.0.1:1")
        .output()
        .expect("run harbor run owner/package");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Should mention "failed to resolve" or "network error", not "not implemented".
    assert!(
        !stderr.contains("not implemented"),
        "registry run should no longer be a stub; got:\n{stderr}"
    );
    assert!(
        stderr.contains("failed to resolve") || stderr.contains("network error"),
        "expected registry/network error, got:\n{stderr}"
    );
}

#[test]
fn yank_fails_with_login_error_when_not_authenticated() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let output = harbor()
        .current_dir(dir.path())
        .args(["yank", "owner/package", "--version", "1.2.0"])
        .env("HOME", dir.path())
        .output()
        .expect("run harbor yank");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not logged in"),
        "expected 'not logged in' error, got:\n{stderr}"
    );
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
fn rm_all_succeeds_even_on_empty_cache() {
    // --all with no target is valid at the parser level. With an empty temp
    // home it should succeed (clear nothing, print confirmation).
    let dir = tempfile::tempdir().expect("create tempdir");
    let output = harbor()
        .current_dir(dir.path())
        .args(["rm", "--all"])
        .env("HOME", dir.path())
        .output()
        .expect("run harbor rm --all");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Cleared packages"));
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
fn logout_succeeds_even_without_credentials() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let output = harbor()
        .current_dir(dir.path())
        .args(["logout"])
        .env("HOME", dir.path())
        .output()
        .expect("run harbor logout");
    assert!(output.status.success(), "logout without credentials should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Logged out"), "stdout:\n{stdout}");
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
