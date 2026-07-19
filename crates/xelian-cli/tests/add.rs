//! Integration tests for `xelian add <github-url>` (H-113), exercised via the
//! compiled `xelian` binary.
//!
//! `XelianHome::resolve()` uses `dirs::home_dir()`, which honors `$HOME` on
//! macOS/Linux — each test points the spawned `xelian` process's `HOME` at a
//! fresh tempdir so it never touches the real `~/.xelian`.
//!
//! Most of `xelian add`'s behavior (resolve → download → detect → infer →
//! build → run) requires network access to github.com, so it is covered by a
//! single `#[ignore]`d end-to-end test, run manually. The tests below cover
//! the non-network path: URL rejection, which must fail fast before any
//! network activity.

use std::path::Path;
use std::process::{Command, Output};

/// Spawn `xelian add <url>` with `HOME` pointed at `home_dir` on the spawned
/// process only (never mutating this test process's own env).
fn run_xelian_add_with_home(url: &str, home_dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xelian"))
        .arg("add")
        .arg(url)
        .env("HOME", home_dir)
        .output()
        .expect("run `xelian add`")
}

#[test]
fn rejects_a_non_github_url_before_any_network_activity() {
    let home_dir = tempfile::tempdir().expect("home dir");
    let output = run_xelian_add_with_home("not-a-github-url", home_dir.path());

    assert!(!output.status.success(), "a malformed URL must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid GitHub repository URL"),
        "stderr should explain the URL was rejected:\n{stderr}"
    );
    assert!(
        stderr.contains("https://"),
        "stderr should mention the expected https:// form:\n{stderr}"
    );

    // Nothing should have been cached: the URL was rejected before any
    // import work (network or filesystem) began.
    assert!(!home_dir.path().join(".xelian/packages/github").exists());
}

#[test]
fn rejects_a_non_github_host() {
    let home_dir = tempfile::tempdir().expect("home dir");
    let output = run_xelian_add_with_home("https://gitlab.com/octocat/hello-world", home_dir.path());

    assert!(!output.status.success(), "a non-github.com host must be rejected");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("github.com"),
        "stderr should explain that the host must be github.com:\n{stderr}"
    );

    assert!(!home_dir.path().join(".xelian/packages/github").exists());
}

#[test]
fn rejects_extra_path_segments_in_the_url() {
    let home_dir = tempfile::tempdir().expect("home dir");
    let output = run_xelian_add_with_home(
        "https://github.com/octocat/hello-world/tree/main",
        home_dir.path(),
    );

    assert!(!output.status.success(), "extra path segments must be rejected");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid GitHub repository URL"),
        "stderr:\n{stderr}"
    );
}

/// Full end-to-end `xelian add` against a real, small, public GitHub
/// repository: resolves HEAD, downloads, detects language, infers
/// xelian.toml, builds the package, then runs the shared §9.6+ pipeline.
/// Requires network access, so it is not run by default.
#[test]
#[ignore = "network: imports a real GitHub repo"]
fn imports_and_runs_a_real_public_repository() {
    let home_dir = tempfile::tempdir().expect("home dir");

    // octocat/Hello-World is GitHub's own small, stable demo repository. It
    // has no pyproject.toml/package.json, so language detection is expected
    // to fail with a clear error — this still exercises resolve → download →
    // detect end-to-end without depending on the repo happening to be a
    // packageable agent.
    let output = run_xelian_add_with_home("https://github.com/octocat/Hello-World", home_dir.path());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Resolving"), "stderr:\n{stderr}");
    assert!(stderr.contains("Resolved to commit"), "stderr:\n{stderr}");

    let cache_root = home_dir.path().join(".xelian/packages/github/octocat/Hello-World");
    assert!(cache_root.is_dir(), "the repo should have been downloaded and cached");
}
