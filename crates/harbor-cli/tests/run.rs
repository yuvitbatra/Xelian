//! Integration tests for `harbor run <local .harbor path>` (H-050/H-051/H-052),
//! exercised via the compiled `harbor` binary.
//!
//! `HarborHome::resolve()` uses `dirs::home_dir()`, which honors `$HOME` on
//! macOS/Linux — each test points the spawned `harbor` process's `HOME` at a
//! fresh tempdir so it never touches the real `~/.harbor`.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn write_file(dir: &Path, rel: &str, contents: &str) {
    let full = dir.join(rel);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&full, contents).unwrap();
}

/// Scaffold a minimal, fully valid Harbor package into `dir`.
fn scaffold_valid_package(dir: &Path, name: &str, version: &str) {
    write_file(
        dir,
        "harbor.toml",
        &format!(
            r#"
spec-version = 1
name = "{name}"
version = "{version}"
description = "A test agent."
package-type = "agent"
language = "python"
runtime = ">=3.11,<4"
entrypoint = "src/main.py"
license = "MIT"
permissions = []
features = []

[author]
name = "Jane Doe"
email = "jane@example.com"

[dependencies]
manifest = "pyproject.toml"
"#
        ),
    );
    write_file(dir, "README.md", "# test\n");
    write_file(dir, "LICENSE", "MIT License\n");
    write_file(dir, "src/main.py", "print('hi')\n");
    write_file(dir, "pyproject.toml", "[project]\nname = \"x\"\n");
}

/// Spawn `harbor run <archive>` with `HOME` pointed at `home_dir` on the
/// spawned process only (never mutating this test process's own env).
fn run_harbor_with_home(archive: &Path, home_dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_harbor"))
        .arg("run")
        .arg(archive)
        .env("HOME", home_dir)
        .output()
        .expect("run `harbor run`")
}

#[test]
fn run_local_archive_extracts_and_prepares_the_package() {
    let fixture = tempfile::tempdir().expect("fixture dir");
    scaffold_valid_package(fixture.path(), "my-agent", "1.0.0");

    let outcome = harbor_core::validate::validate_and_build(fixture.path(), None)
        .expect("fixture package should build");

    let home_dir = tempfile::tempdir().expect("home dir");

    let output = run_harbor_with_home(&outcome.archive_path, home_dir.path());
    assert!(
        output.status.success(),
        "harbor run should succeed; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("my-agent"), "stdout:\n{stdout}");
    assert!(stdout.contains("1.0.0"), "stdout:\n{stdout}");
    assert!(stdout.contains("launch not yet implemented"), "stdout:\n{stdout}");

    let extracted = home_dir.path().join(".harbor/packages/local/my-agent/1.0.0");
    assert!(extracted.join("harbor.toml").is_file());
    assert!(extracted.join("src/main.py").is_file());
    assert!(extracted.join("README.md").is_file());
    assert!(extracted.join("harbor.lock").is_file());
}

#[test]
fn corrupted_checksum_aborts_before_extraction() {
    let fixture = tempfile::tempdir().expect("fixture dir");
    scaffold_valid_package(fixture.path(), "bad-agent", "1.0.0");

    harbor_core::validate::validate_and_build(fixture.path(), None).expect("initial build");

    // Corrupt harbor.lock's package-checksum in place, then rebuild the
    // archive from the (now-inconsistent) on-disk file set, so the rebuilt
    // archive's own harbor.lock disagrees with its actual contents — the
    // same "wrong package-checksum baked into harbor.lock" scenario §9.4
    // guards against, built entirely from already-public harbor-core APIs.
    let lock_path = fixture.path().join("harbor.lock");
    let lock_str = fs::read_to_string(&lock_path).unwrap();
    let mut lock = harbor_core::lockfile::Lockfile::from_toml_str(&lock_str).unwrap();
    lock.package_checksum =
        Some("sha256:0000000000000000000000000000000000000000000000000000000000000".to_string());
    fs::write(&lock_path, lock.to_toml_string().unwrap()).unwrap();

    let files = harbor_core::package::collect_files(fixture.path()).unwrap();
    let bad_archive_path = fixture.path().join("bad-agent-1.0.0-corrupt.harbor");
    harbor_core::package::build_archive(fixture.path(), &files, &bad_archive_path)
        .expect("build corrupted archive");

    let home_dir = tempfile::tempdir().expect("home dir");
    let output = run_harbor_with_home(&bad_archive_path, home_dir.path());

    assert!(!output.status.success(), "corrupted archive must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.to_lowercase().contains("checksum"),
        "stderr should mention the checksum mismatch:\n{stderr}"
    );

    let extracted = home_dir.path().join(".harbor/packages/local/bad-agent/1.0.0");
    assert!(!extracted.exists(), "nothing should be extracted on checksum mismatch");
}

#[test]
fn second_identical_run_reuses_the_cache() {
    let fixture = tempfile::tempdir().expect("fixture dir");
    scaffold_valid_package(fixture.path(), "cache-agent", "1.0.0");

    let outcome = harbor_core::validate::validate_and_build(fixture.path(), None)
        .expect("fixture package should build");

    let home_dir = tempfile::tempdir().expect("home dir");

    let first = run_harbor_with_home(&outcome.archive_path, home_dir.path());
    assert!(
        first.status.success(),
        "first run should succeed; stderr:\n{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_stdout = String::from_utf8_lossy(&first.stdout);
    assert!(!first_stdout.contains("cached"), "first run must not be a cache hit:\n{first_stdout}");

    let second = run_harbor_with_home(&outcome.archive_path, home_dir.path());
    assert!(
        second.status.success(),
        "second run should succeed; stderr:\n{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second_stdout = String::from_utf8_lossy(&second.stdout);
    assert!(
        second_stdout.to_lowercase().contains("cached"),
        "second run should report cache reuse:\n{second_stdout}"
    );

    let extracted = home_dir.path().join(".harbor/packages/local/cache-agent/1.0.0");
    assert!(extracted.join("harbor.toml").is_file());
}

#[test]
fn nonexistent_local_path_falls_through_to_not_implemented() {
    // A bare `owner/package`-shaped target (no `.harbor` suffix, not an
    // existing file) must still hit the pre-existing "not implemented" stub
    // for registry refs, not be misdetected as a local archive.
    let home_dir = tempfile::tempdir().expect("home dir");
    let output = Command::new(env!("CARGO_BIN_EXE_harbor"))
        .arg("run")
        .arg("owner/package")
        .env("HOME", home_dir.path())
        .output()
        .expect("run harbor run owner/package");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not implemented"), "stderr:\n{stderr}");
}
