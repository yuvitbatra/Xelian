//! Integration tests for `xelian push` (H-150/H-151): the §8.1 validation
//! pipeline, archive build, and authenticated upload, exercised via the
//! compiled `xelian` binary.
//!
//! Tests that need to bypass the auth check create disposable credentials
//! in a `HOME`-isolated tempdir and point `XELIAN_REGISTRY_URL` at a
//! non-routable address so upload attempts fail fast without real network.

use std::fs;
use std::path::Path;
use std::process::Command;

fn xelian_push_in(dir: &Path, home_dir: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_xelian"));
    cmd.current_dir(dir)
        .arg("push")
        .env("HOME", home_dir)
        .env("XELIAN_REGISTRY_URL", "http://127.0.0.1:1");
    cmd
}

fn write_file(dir: &Path, rel: &str, contents: &str) {
    let full = dir.join(rel);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&full, contents).unwrap();
}

/// Write a minimal credentials.toml at the temp xelian home so the auth
/// check passes. The registry URL points at a non-routable address so any
/// upload attempt fails fast without real network.
fn setup_credentials(home_dir: impl AsRef<Path>) {
    let home_dir = home_dir.as_ref();
    let creds_dir = home_dir.join(".xelian");
    fs::create_dir_all(&creds_dir).unwrap();
    fs::write(
        creds_dir.join("credentials.toml"),
        r#"token = "test-token"
username = "testuser"
registry_url = "http://127.0.0.1:1"
"#,
    )
    .unwrap();
}

/// Scaffold a full, valid Xelian package (per manifest rules, §5.3 required
/// files, and a real entrypoint) into `dir`, plus a `.gitignore` excluding
/// `secret.txt` and a `secret.txt` that must never end up in the archive.
fn scaffold_valid_package(dir: &Path) {
    write_file(
        dir,
        "xelian.toml",
        r#"
spec-version = 1
name = "my-agent"
version = "1.0.0"
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
"#,
    );
    write_file(dir, "README.md", "# my-agent\n");
    write_file(dir, "LICENSE", "MIT License\n");
    write_file(dir, "src/main.py", "print('hi')\n");
    write_file(dir, "pyproject.toml", "[project]\nname = \"my-agent\"\n");
    write_file(dir, ".gitignore", "secret.txt\n");
    write_file(dir, "secret.txt", "do-not-ship-me");
}

/// List archive-relative entry paths inside a `.xelian` (tar.gz) file by
/// shelling out to `tar -tzf`, the same inspection the format's own spec
/// (§5.1) promises works with universally available tooling.
fn list_archive_entries(path: &Path) -> Vec<String> {
    let output = Command::new("tar")
        .arg("-tzf")
        .arg(path)
        .output()
        .expect("run tar -tzf");
    assert!(
        output.status.success(),
        "tar -tzf failed on {path:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.trim_end_matches('/').to_string())
        .collect()
}

#[test]
fn push_validates_and_builds_archive_then_fails_on_upload() {
    let fixture = tempfile::tempdir().expect("fixture dir");
    scaffold_valid_package(fixture.path());

    let home_dir = tempfile::tempdir().expect("home dir");
    setup_credentials(&home_dir);

    let output = xelian_push_in(fixture.path(), home_dir.path())
        .output()
        .expect("run xelian push");

    // Validation and build succeeded; the error should be from the upload
    // step trying to reach a non-routable registry.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "push should exit non-zero (upload to non-routable registry); stderr:\n{stderr}"
    );

    // The archive must exist even though the upload fails.
    let archive_path = fixture.path().join("my-agent-1.0.0.xelian");
    assert!(
        archive_path.is_file(),
        "archive must exist even though upload failed"
    );
}

#[test]
fn built_archive_contains_required_files_and_excludes_gitignored_and_git_metadata() {
    let fixture = tempfile::tempdir().expect("fixture dir");
    scaffold_valid_package(fixture.path());
    // A .git/ directory should always be excluded regardless of .gitignore.
    write_file(fixture.path(), ".git/HEAD", "ref: refs/heads/main\n");

    let home_dir = tempfile::tempdir().expect("home dir");
    setup_credentials(&home_dir);

    xelian_push_in(fixture.path(), home_dir.path())
        .output()
        .expect("run xelian push");

    let archive_path = fixture.path().join("my-agent-1.0.0.xelian");
    assert!(archive_path.is_file());

    let entries = list_archive_entries(&archive_path);

    for expected in [
        "xelian.toml",
        "xelian.lock",
        "README.md",
        "LICENSE",
        "src/main.py",
    ] {
        assert!(
            entries.contains(&expected.to_string()),
            "missing {expected} in {entries:?}"
        );
    }
    assert!(
        !entries.contains(&"secret.txt".to_string()),
        "got: {entries:?}"
    );
    assert!(
        !entries
            .iter()
            .any(|e| e == ".git" || e.starts_with(".git/")),
        "got: {entries:?}"
    );
}

#[test]
fn xelian_lock_in_cwd_has_all_keys_populated() {
    let fixture = tempfile::tempdir().expect("fixture dir");
    scaffold_valid_package(fixture.path());

    let home_dir = tempfile::tempdir().expect("home dir");
    setup_credentials(&home_dir);

    xelian_push_in(fixture.path(), home_dir.path())
        .output()
        .expect("run xelian push");

    let lock_path = fixture.path().join("xelian.lock");
    assert!(lock_path.is_file());
    let lock_str = fs::read_to_string(&lock_path).unwrap();
    let lock = xelian_core::lockfile::Lockfile::from_toml_str(&lock_str)
        .expect("xelian.lock written by push must parse");

    assert_eq!(lock.spec_version, 1);
    assert!(!lock.xelian_version.is_empty());
    assert_eq!(lock.package_version, "1.0.0");
    assert!(!lock.generated_at.is_empty());
    assert_eq!(lock.native_manifest, "pyproject.toml");
    // No native lockfile declared in this scaffold, so native-lock-checksum
    // must be absent while package-checksum must be present.
    assert!(lock.native_lockfile.is_none());
    assert!(lock.native_lock_checksum.is_none());
    assert!(lock.package_checksum.is_some());
    assert!(lock.package_checksum.unwrap().starts_with("sha256:"));
}

#[test]
fn missing_required_file_fails_validation_before_building_anything() {
    let fixture = tempfile::tempdir().expect("fixture dir");
    scaffold_valid_package(fixture.path());
    fs::remove_file(fixture.path().join("LICENSE")).unwrap();

    let home_dir = tempfile::tempdir().expect("home dir");
    setup_credentials(&home_dir);

    let output = xelian_push_in(fixture.path(), home_dir.path())
        .output()
        .expect("run xelian push");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("LICENSE"), "stderr:\n{stderr}");
    assert!(
        !fixture.path().join("my-agent-1.0.0.xelian").exists(),
        "no archive should be produced on validation failure"
    );
}

#[test]
fn gitignored_entrypoint_fails_validation_with_a_specific_error() {
    let fixture = tempfile::tempdir().expect("fixture dir");
    scaffold_valid_package(fixture.path());
    write_file(fixture.path(), ".gitignore", "secret.txt\nsrc/main.py\n");

    let home_dir = tempfile::tempdir().expect("home dir");
    setup_credentials(&home_dir);

    let output = xelian_push_in(fixture.path(), home_dir.path())
        .output()
        .expect("run xelian push");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("entrypoint"), "stderr:\n{stderr}");
    assert!(
        !fixture.path().join("my-agent-1.0.0.xelian").exists(),
        "no archive should be produced on validation failure"
    );
}

#[test]
fn push_without_login_fails_with_helpful_message() {
    let fixture = tempfile::tempdir().expect("fixture dir");
    scaffold_valid_package(fixture.path());

    // No credentials set up — push should fail with a "not logged in" error.
    let home_dir = tempfile::tempdir().expect("home dir");

    let output = Command::new(env!("CARGO_BIN_EXE_xelian"))
        .current_dir(fixture.path())
        .arg("push")
        .env("HOME", home_dir.path())
        .output()
        .expect("run xelian push");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not logged in"),
        "expected not-logged-in error, got:\n{stderr}"
    );
}
