//! Integration tests for `harbor push` (H-040/H-041/H-042): the §8.1
//! validation pipeline and archive build, exercised via the compiled
//! `harbor` binary (upload itself is out of scope / not yet implemented).

use std::fs;
use std::path::Path;
use std::process::Command;

fn harbor_in(dir: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_harbor"));
    cmd.current_dir(dir);
    cmd
}

fn write_file(dir: &Path, rel: &str, contents: &str) {
    let full = dir.join(rel);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&full, contents).unwrap();
}

/// Scaffold a full, valid Harbor package (per manifest rules, §5.3 required
/// files, and a real entrypoint) into `dir`, plus a `.gitignore` excluding
/// `secret.txt` and a `secret.txt` that must never end up in the archive.
fn scaffold_valid_package(dir: &Path) {
    write_file(
        dir,
        "harbor.toml",
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

/// List archive-relative entry paths inside a `.harbor` (tar.gz) file by
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
fn push_builds_archive_but_fails_at_the_unimplemented_upload_step() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    scaffold_valid_package(tmp.path());

    let output = harbor_in(tmp.path()).arg("push").output().expect("run harbor push");

    assert!(
        !output.status.success(),
        "push should exit non-zero (upload is unimplemented)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not yet implemented"),
        "stderr should mention the upload is unimplemented, got:\n{stderr}"
    );

    let archive_path = tmp.path().join("my-agent-1.0.0.harbor");
    assert!(
        archive_path.is_file(),
        "archive must exist even though upload failed"
    );
}

#[test]
fn built_archive_contains_required_files_and_excludes_gitignored_and_git_metadata() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    scaffold_valid_package(tmp.path());
    // A .git/ directory should always be excluded regardless of .gitignore.
    write_file(tmp.path(), ".git/HEAD", "ref: refs/heads/main\n");

    harbor_in(tmp.path()).arg("push").output().expect("run harbor push");

    let archive_path = tmp.path().join("my-agent-1.0.0.harbor");
    assert!(archive_path.is_file());

    let entries = list_archive_entries(&archive_path);

    for expected in ["harbor.toml", "harbor.lock", "README.md", "LICENSE", "src/main.py"] {
        assert!(entries.contains(&expected.to_string()), "missing {expected} in {entries:?}");
    }
    assert!(!entries.contains(&"secret.txt".to_string()), "got: {entries:?}");
    assert!(
        !entries.iter().any(|e| e == ".git" || e.starts_with(".git/")),
        "got: {entries:?}"
    );
}

#[test]
fn harbor_lock_in_cwd_has_all_keys_populated() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    scaffold_valid_package(tmp.path());

    harbor_in(tmp.path()).arg("push").output().expect("run harbor push");

    let lock_path = tmp.path().join("harbor.lock");
    assert!(lock_path.is_file());
    let lock_str = fs::read_to_string(&lock_path).unwrap();
    let lock = harbor_core::lockfile::Lockfile::from_toml_str(&lock_str)
        .expect("harbor.lock written by push must parse");

    assert_eq!(lock.spec_version, 1);
    assert!(!lock.harbor_version.is_empty());
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
    let tmp = tempfile::tempdir().expect("create tempdir");
    scaffold_valid_package(tmp.path());
    fs::remove_file(tmp.path().join("LICENSE")).unwrap();

    let output = harbor_in(tmp.path()).arg("push").output().expect("run harbor push");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("LICENSE"), "stderr:\n{stderr}");
    assert!(
        !tmp.path().join("my-agent-1.0.0.harbor").exists(),
        "no archive should be produced on validation failure"
    );
}

#[test]
fn gitignored_entrypoint_fails_validation_with_a_specific_error() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    scaffold_valid_package(tmp.path());
    write_file(tmp.path(), ".gitignore", "secret.txt\nsrc/main.py\n");

    let output = harbor_in(tmp.path()).arg("push").output().expect("run harbor push");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("entrypoint"), "stderr:\n{stderr}");
    assert!(
        !tmp.path().join("my-agent-1.0.0.harbor").exists(),
        "no archive should be produced on validation failure"
    );
}
