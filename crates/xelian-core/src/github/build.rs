//! Build step for imported repositories (SPEC.md §12.2, extends step 7).
//!
//! TypeScript projects — which is most of the MCP server ecosystem — declare
//! an entrypoint like `dist/index.js` that does not exist in a source
//! checkout and is only produced by `npm run build`. Without this step those
//! repositories can be imported but never launched.
//!
//! The build runs in the *package* directory with the environment's
//! `node_modules` symlinked in. Building in the environment does not work:
//! that tree is itself symlinks back to the package, and bundlers (bun, tsc,
//! esbuild) resolve each source file to its real path before looking for
//! `node_modules`, so every dependency fails to resolve. Building in place
//! also lands output exactly where launch and `push` read it.
//!
//! ## Trust
//!
//! This executes repo-authored scripts. `npm install` already runs arbitrary
//! `postinstall` hooks for any imported package, so this widens an existing
//! trust boundary rather than creating a new one. It is deliberate, and it is
//! why the build only ever runs for a package whose entrypoint is missing.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::GithubError;
use crate::run::runtime::run_command_checked;

/// Build tools that are *external* programs rather than devDependencies.
///
/// Tools like `tsc` and `esbuild` arrive in `node_modules/.bin` via the
/// package's own devDependencies, and `npm run` puts that on PATH already.
/// These do not: a package whose build script says `bun build ...` fails with
/// "bun: command not found" on a machine that has never installed bun. Since
/// Xelian's promise is that a package runs with zero setup, provision them.
const EXTERNAL_BUILD_TOOLS: &[&str] = &["bun", "pnpm", "yarn"];

/// The npm package that provides each external tool.
fn npm_package_for(tool: &str) -> &'static str {
    match tool {
        "bun" => "bun",
        "pnpm" => "pnpm",
        "yarn" => "yarn",
        _ => unreachable!("callers only pass EXTERNAL_BUILD_TOOLS entries"),
    }
}

/// The external tool a build script invokes, if any.
///
/// Scans every command in the script (`&&`, `;`, `|` separated) rather than
/// only the first, since build scripts commonly chain steps.
fn required_external_tool(build_script: &str) -> Option<&'static str> {
    for command in build_script.split(['&', ';', '|', '\n']) {
        if let Some(first) = command.split_whitespace().next() {
            let bare = first.rsplit('/').next().unwrap_or(first);
            if let Some(tool) = EXTERNAL_BUILD_TOOLS.iter().find(|t| **t == bare) {
                return Some(tool);
            }
        }
    }
    None
}

/// Read a package's `build` script.
fn build_script(dir: &Path) -> Option<String> {
    let contents = fs::read_to_string(dir.join("package.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&contents).ok()?;
    v.get("scripts")?.get("build")?.as_str().map(str::to_string)
}

/// Install an external build tool into the environment so the build can run.
///
/// Installed into the environment rather than globally: Xelian must not
/// mutate the user's machine outside `~/.xelian`.
fn ensure_build_tool(tool: &str, env_dir: &Path, bin_dir: &Path) -> Result<PathBuf, GithubError> {
    let local = env_dir.join("node_modules").join(".bin").join(tool);
    if local.is_file() {
        return Ok(local);
    }
    if let Some(found) = crate::run::runtime::find_in_path(tool) {
        return Ok(found);
    }

    eprintln!("Build requires `{tool}`, which is not installed — provisioning it locally...");

    let mut cmd = Command::new(bin_dir.join("npm"));
    cmd.arg("install")
        .arg("--no-save")
        .arg("--prefix")
        .arg(env_dir)
        .arg(npm_package_for(tool))
        .current_dir(env_dir);
    with_runtime_path(&mut cmd, bin_dir);

    run_command_checked(&mut cmd).map_err(|e| GithubError::Build {
        source: Box::new(e),
    })?;

    Ok(local)
}

/// Prepend the provisioned runtime's bin directory (and the environment's
/// `node_modules/.bin`) to a command's PATH.
fn with_runtime_path(cmd: &mut Command, bin_dir: &Path) {
    let mut paths = vec![bin_dir.to_path_buf()];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        cmd.env("PATH", joined);
    }
}

/// Run `npm run build` for an imported Node package.
///
/// Returns `Ok(())` when the build command succeeded. It is the caller's job
/// to re-check whether the entrypoint now exists — a build can succeed and
/// still not produce the file inference predicted.
pub fn run_node_build(
    package_dir: &Path,
    env_dir: &Path,
    bin_dir: &Path,
) -> Result<(), GithubError> {
    // Build in the *package* directory, not the environment.
    //
    // The Node environment is a tree of symlinks pointing back here, which
    // works for `node --preserve-symlinks` but not for bundlers: bun, tsc and
    // esbuild resolve a source file to its real path and then look for
    // `node_modules` beside *that*, i.e. next to the package. Building in the
    // environment therefore fails with "could not resolve" for every
    // dependency. Linking `node_modules` into the package instead makes
    // resolution work for every tool, and lands build output directly where
    // launch and `push` expect it.
    let link = package_dir.join("node_modules");
    let created_link = link_node_modules(&link, &env_dir.join("node_modules"))?;

    let result = build_in_package_dir(package_dir, env_dir, bin_dir);

    // Always remove a link we created: `node_modules` must not become part of
    // the package's file set when the archive is rebuilt after the build.
    if created_link {
        let _ = fs::remove_file(&link);
    }

    result?;
    link_build_outputs_into_env(package_dir, env_dir)
}

/// Directories a JS build conventionally emits into.
const OUTPUT_DIRS: &[&str] = &["dist", "build", "lib", "out"];

/// Link freshly built output directories into the environment.
///
/// The Node environment is built from the package's top-level entries at
/// install time — before the build has run — so a `dist/` created by the
/// build has no corresponding entry there. Launch resolves the entrypoint
/// through the environment (it must, for `--preserve-symlinks` module
/// resolution to work), so without this the build succeeds and the launch
/// still reports a missing entrypoint.
fn link_build_outputs_into_env(package_dir: &Path, env_dir: &Path) -> Result<(), GithubError> {
    for name in OUTPUT_DIRS {
        let produced = package_dir.join(name);
        if !produced.is_dir() {
            continue;
        }
        let in_env = env_dir.join(name);
        match fs::symlink_metadata(&in_env) {
            // Already a symlink: it points back at the package, which is
            // exactly what we want.
            Ok(meta) if meta.file_type().is_symlink() => continue,
            // A *real* directory here is stale output from the package's own
            // lifecycle script, which npm ran inside the install staging
            // directory before this build. That copy is wrong — staging does
            // not reproduce the repository around the package, so a monorepo
            // subpackage's `tsconfig` `extends` fails there and tsc silently
            // falls back to CommonJS. Loading it under an ESM `package.json`
            // fails with "exports is not defined". The build we just ran, in
            // the real package directory, is authoritative; replace it.
            Ok(_) => {
                fs::remove_dir_all(&in_env).map_err(|e| GithubError::Io {
                    path: in_env.clone(),
                    source: e,
                })?;
            }
            Err(_) => {}
        }
        symlink_dir(&produced, &in_env).map_err(|e| GithubError::Io {
            path: in_env.clone(),
            source: e,
        })?;
    }
    Ok(())
}

/// Link the environment's `node_modules` into the package directory.
/// Returns whether a link was created (and so must be cleaned up).
fn link_node_modules(link: &Path, target: &Path) -> Result<bool, GithubError> {
    if link.exists() || fs::symlink_metadata(link).is_ok() {
        return Ok(false);
    }
    if !target.is_dir() {
        return Ok(false);
    }
    symlink_dir(target, link).map_err(|e| GithubError::Io {
        path: link.to_path_buf(),
        source: e,
    })?;
    Ok(true)
}

#[cfg(unix)]
fn symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

fn build_in_package_dir(
    package_dir: &Path,
    env_dir: &Path,
    bin_dir: &Path,
) -> Result<(), GithubError> {
    // Provision any external tool the build script needs before running it,
    // so "bun: command not found" never reaches the user.
    if let Some(script) = build_script(package_dir) {
        if let Some(tool) = required_external_tool(&script) {
            ensure_build_tool(tool, env_dir, bin_dir)?;
        }
    }

    eprintln!("Building package (npm run build)...");

    let mut cmd = Command::new(bin_dir.join("npm"));
    cmd.arg("run").arg("build").current_dir(package_dir);

    // Nested tooling shells out to node; the provisioned runtime and the
    // environment's local bin must win over anything on the ambient PATH.
    let mut paths = vec![
        package_dir.join("node_modules").join(".bin"),
        env_dir.join("node_modules").join(".bin"),
        bin_dir.to_path_buf(),
    ];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        cmd.env("PATH", joined);
    }

    run_command_checked(&mut cmd).map_err(|e| GithubError::Build {
        source: Box::new(e),
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn stale_lifecycle_build_output_in_the_env_is_replaced() {
        // npm ran the package's own `prepare` build inside the install
        // staging dir, where a monorepo subpackage's tsconfig `extends`
        // cannot resolve, so tsc emitted CommonJS. Under an ESM
        // package.json that fails at runtime with "exports is not defined".
        // Our build, run in the real package dir, must win.
        let d = tempfile::tempdir().unwrap();
        let pkg = d.path().join("pkg");
        let env = d.path().join("env");
        fs::create_dir_all(pkg.join("dist")).unwrap();
        fs::create_dir_all(env.join("dist")).unwrap();
        fs::write(pkg.join("dist/index.js"), b"import x from 'y';").unwrap();
        fs::write(env.join("dist/index.js"), b"\"use strict\"; exports.x = 1;").unwrap();

        link_build_outputs_into_env(&pkg, &env).unwrap();

        assert!(
            fs::symlink_metadata(env.join("dist"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "stale real directory must be replaced by a link to the package"
        );
        let loaded = fs::read_to_string(env.join("dist/index.js")).unwrap();
        assert!(loaded.contains("import"), "must resolve to our ESM build");
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_symlinked_output_dir_is_left_alone() {
        let d = tempfile::tempdir().unwrap();
        let pkg = d.path().join("pkg");
        let env = d.path().join("env");
        fs::create_dir_all(pkg.join("dist")).unwrap();
        fs::create_dir_all(&env).unwrap();
        fs::write(pkg.join("dist/index.js"), b"built").unwrap();
        std::os::unix::fs::symlink(pkg.join("dist"), env.join("dist")).unwrap();

        link_build_outputs_into_env(&pkg, &env).unwrap();

        assert_eq!(fs::read(env.join("dist/index.js")).unwrap(), b"built");
    }

    #[test]
    fn external_build_tools_are_detected_from_the_script() {
        // fetch-mcp shape: builds with bun, which is not a devDependency.
        assert_eq!(
            required_external_tool("bun build src/index.ts --outdir dist"),
            Some("bun")
        );
        assert_eq!(required_external_tool("pnpm run compile"), Some("pnpm"));
        assert_eq!(required_external_tool("yarn build"), Some("yarn"));
    }

    #[test]
    fn chained_build_scripts_are_scanned_past_the_first_command() {
        assert_eq!(
            required_external_tool("rm -rf dist && bun build src/index.ts"),
            Some("bun")
        );
        assert_eq!(
            required_external_tool("tsc; pnpm bundle"),
            Some("pnpm"),
            "later commands must be scanned too"
        );
    }

    #[test]
    fn devdependency_tools_are_not_treated_as_external() {
        // tsc/esbuild arrive via node_modules/.bin, which `npm run` already
        // puts on PATH — provisioning them would be wrong.
        assert_eq!(required_external_tool("tsc -p tsconfig.json"), None);
        assert_eq!(required_external_tool("esbuild src/index.ts"), None);
        assert_eq!(required_external_tool("npm run compile"), None);
    }

    #[test]
    fn tool_paths_are_matched_on_the_program_name() {
        assert_eq!(
            required_external_tool("./node_modules/.bin/bun x"),
            Some("bun")
        );
    }
}
