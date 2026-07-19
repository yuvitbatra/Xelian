//! Integration tests for `xelian init` (H-020/H-021).
//!
//! Runs the compiled `xelian` binary with its working directory set to a
//! fresh tempdir, so nothing under the repo itself is ever touched.

use std::process::Command;

fn xelian_in(dir: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_xelian"));
    cmd.current_dir(dir);
    cmd
}

#[test]
fn init_creates_both_files_and_exits_zero() {
    let tmp = tempfile::tempdir().expect("create tempdir");

    let output = xelian_in(tmp.path())
        .arg("init")
        .output()
        .expect("run xelian init");

    assert!(
        output.status.success(),
        "expected exit 0, stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(tmp.path().join("xelian.toml").is_file());
    assert!(tmp.path().join("xelian.lock").is_file());
}

#[test]
fn init_twice_without_force_fails_and_leaves_files_untouched() {
    let tmp = tempfile::tempdir().expect("create tempdir");

    let first = xelian_in(tmp.path())
        .arg("init")
        .output()
        .expect("first init");
    assert!(first.status.success());

    let manifest_path = tmp.path().join("xelian.toml");
    let before = std::fs::read_to_string(&manifest_path).unwrap();

    let second = xelian_in(tmp.path())
        .arg("init")
        .output()
        .expect("second init");
    assert!(
        !second.status.success(),
        "second init should fail without --force"
    );
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(stderr.contains("--force"), "stderr:\n{stderr}");

    let after = std::fs::read_to_string(&manifest_path).unwrap();
    assert_eq!(
        before, after,
        "xelian.toml must be untouched on failed re-init"
    );
}

#[test]
fn init_force_overwrites_existing_files() {
    let tmp = tempfile::tempdir().expect("create tempdir");

    let first = xelian_in(tmp.path())
        .arg("init")
        .output()
        .expect("first init");
    assert!(first.status.success());

    // Corrupt both generated files to prove --force actually rewrites them.
    std::fs::write(tmp.path().join("xelian.toml"), "not a real manifest").unwrap();
    std::fs::write(tmp.path().join("xelian.lock"), "not a real lockfile").unwrap();

    let forced = xelian_in(tmp.path())
        .args(["init", "--force"])
        .output()
        .expect("forced init");
    assert!(
        forced.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&forced.stderr)
    );

    let manifest = std::fs::read_to_string(tmp.path().join("xelian.toml")).unwrap();
    assert_ne!(manifest, "not a real manifest");
    assert!(manifest.contains("spec-version = 1"));
}
