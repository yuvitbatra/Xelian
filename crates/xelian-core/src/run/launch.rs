//! Entrypoint launch (SPEC.md §9.10): agent REPL or MCP stdio server.
//!
//! Xelian's own status output goes to stderr. The child inherits Xelian's
//! stdin/stdout/stderr — for agents that is the interactive REPL terminal
//! (§9.10.1); for MCP servers stdin/stdout are the JSON-RPC stdio transport
//! (§9.10.2) and must carry nothing but the protocol.

use std::io;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use thiserror::Error;

use crate::manifest::{Language, Manifest, PackageType};

#[derive(Debug, Error)]
pub enum LaunchError {
    #[error("I/O error while launching: {0}")]
    Io(#[from] io::Error),

    #[error("failed to start the entrypoint process: {0}")]
    Spawn(String),

    #[error("entrypoint path {path} does not exist in the extracted package")]
    EntrypointMissing { path: PathBuf },

    #[error("failed to bind a local port for the MCP server: {0}")]
    PortBind(io::Error),
}

/// Launch an agent or MCP server (SPEC.md §9.10) and block until it exits.
///
/// Accepts the resolved `bin_dir` (from [`crate::run::prepare_environment`])
/// so this function does not need to re-provision the runtime. Returns the
/// child's [`ExitStatus`] so the CLI can propagate its exit code faithfully.
pub fn launch(
    manifest: &Manifest,
    package_dir: &Path,
    env_dir: &Path,
    bin_dir: &Path,
    env_pairs: &[(String, String)],
) -> Result<ExitStatus, LaunchError> {
    let entrypoint = package_dir.join(&manifest.entrypoint);
    if !entrypoint.is_file() {
        return Err(LaunchError::EntrypointMissing { path: entrypoint });
    }

    let (program, args, work_dir) = build_launch_command(manifest, package_dir, env_dir, bin_dir)?;

    announce_ready(manifest);

    let mut cmd = Command::new(&program);
    cmd.args(&args);
    cmd.current_dir(&work_dir);

    // Inherit the parent's stdio: interactive REPL for agents (§9.10.1),
    // transparent JSON-RPC stdio transport for MCP servers (§9.10.2).
    cmd.stdin(std::process::Stdio::inherit());
    cmd.stdout(std::process::Stdio::inherit());
    cmd.stderr(std::process::Stdio::inherit());

    // Apply resolved environment variables (§6.2.1, §9.10).
    for (key, val) in env_pairs {
        cmd.env(key, val);
    }

    // Port passthrough for MCP servers that expose HTTP (§9.10.2, decision
    // 2026-07-16): only when the manifest declares `port`. Injecting PORT
    // into stdio-only servers that never asked for one changes their
    // behavior (many frameworks switch modes on PORT's mere presence).
    if manifest.package_type == PackageType::Mcp {
        if let Some(requested) = manifest.port {
            let port = resolve_port(requested)?;
            cmd.env("PORT", port.to_string());
            if requested != 0 && port != requested {
                eprintln!("MCP server: assigned PORT={port} (requested port {requested} is busy)");
            } else {
                eprintln!("MCP server: assigned PORT={port}");
            }
        }
    }

    let mut child = cmd.spawn().map_err(|e| LaunchError::Spawn(e.to_string()))?;
    Ok(child.wait()?)
}

/// Print the "it worked, it's yours now" line immediately before handing the
/// terminal to the child.
///
/// Without this, a successful launch is indistinguishable from a hang: an
/// agent REPL that has not yet printed its own prompt, and an MCP server
/// (which is silent by design, since its stdout is the JSON-RPC transport),
/// both look like nothing happened.
///
/// Goes to stderr — stdout belongs to the child (§9.10.2).
fn announce_ready(manifest: &Manifest) {
    match manifest.package_type {
        PackageType::Agent => {
            eprintln!();
            eprintln!(
                "  {} {} is ready — type your message and press enter (Ctrl-C to exit).",
                manifest.name, manifest.version
            );
            eprintln!();
        }
        PackageType::Mcp => {
            eprintln!();
            eprintln!(
                "  {} {} is running as an MCP server over stdio.",
                manifest.name, manifest.version
            );
            eprintln!("  Connect an MCP client to this process, or press Ctrl-C to stop.");
            eprintln!();
        }
    }
}

/// Build the (program, args, working_directory) for launching the entrypoint.
fn build_launch_command(
    manifest: &Manifest,
    package_dir: &Path,
    env_dir: &Path,
    bin_dir: &Path,
) -> Result<(String, Vec<String>, PathBuf), LaunchError> {
    match manifest.language {
        Language::Python => {
            let python_bin = env_dir.join("bin").join("python");
            if !python_bin.is_file() {
                return Err(LaunchError::Spawn(format!(
                    "Python binary not found at {}",
                    python_bin.display()
                )));
            }
            // A `__main__.py` is the entrypoint of a *package*, and running it
            // by path puts its directory on sys.path instead of the package
            // root — which breaks every `from . import x` inside it. Invoke it
            // the way Python intends (`python -m pkg`) instead. This keeps the
            // manifest's entrypoint-as-path contract intact; only the launch
            // mechanics differ.
            if let Some((module, root)) = module_invocation(&manifest.entrypoint) {
                return Ok((
                    python_bin.to_string_lossy().to_string(),
                    vec!["-m".to_string(), module],
                    package_dir.join(root),
                ));
            }
            Ok((
                python_bin.to_string_lossy().to_string(),
                vec![package_dir
                    .join(&manifest.entrypoint)
                    .to_string_lossy()
                    .to_string()],
                package_dir.to_path_buf(),
            ))
        }
        Language::Node => {
            let node_bin = bin_dir.join("node");
            if !node_bin.is_file() {
                return Err(LaunchError::Spawn(format!(
                    "Node binary not found at {}",
                    node_bin.display()
                )));
            }
            // The Node env is built as symlinks from env_dir → package_dir
            // plus a real node_modules/ inside env_dir (see NodeRuntimeManager
            // in runtime.rs). The script argument MUST be the env_dir path —
            // the symlink — combined with --preserve-symlinks[-main], so Node
            // resolves modules from env_dir's ancestry and finds
            // <env_dir>/node_modules. Passing the real package_dir path would
            // make the flags no-ops and break every dependency require().
            let script = env_dir.join(&manifest.entrypoint);
            if !script.is_file() {
                return Err(LaunchError::EntrypointMissing { path: script });
            }
            Ok((
                node_bin.to_string_lossy().to_string(),
                vec![
                    "--preserve-symlinks".to_string(),
                    "--preserve-symlinks-main".to_string(),
                    script.to_string_lossy().to_string(),
                ],
                env_dir.to_path_buf(),
            ))
        }
    }
}

/// If `entrypoint` is a package's `__main__.py`, return the dotted module name
/// to run with `-m` and the working directory to run it from.
///
/// `src/mcp_atlassian/__main__.py` → (`mcp_atlassian`, `src`)
/// `agent/cli/__main__.py`         → (`agent.cli`, ``)
///
/// Returns `None` for anything else, including a bare top-level
/// `__main__.py`, which has no package to be a member of.
fn module_invocation(entrypoint: &str) -> Option<(String, String)> {
    let rest = entrypoint.strip_suffix("/__main__.py")?;
    let components: Vec<&str> = rest.split('/').filter(|c| !c.is_empty()).collect();
    if components.is_empty() {
        return None;
    }

    // A leading `src/` is a layout convention, not part of the module path.
    let (root, module_parts) = if components[0] == "src" && components.len() > 1 {
        ("src".to_string(), &components[1..])
    } else {
        (String::new(), &components[..])
    };

    if module_parts.is_empty() {
        return None;
    }
    Some((module_parts.join("."), root))
}

/// Bind to an OS-assigned free port and return it.
fn bind_free_port() -> io::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

/// Resolve the port for an MCP server that declared one (SPEC.md §9.10.2).
///
/// `0` means "any free port". A busy requested port falls back to a free one
/// (the caller informs the user). Note this is a passthrough: the child does
/// the actual bind, so the port is probed, not held — a small window exists
/// in which another process could take it.
fn resolve_port(requested: u16) -> Result<u16, LaunchError> {
    if requested == 0 {
        return bind_free_port().map_err(LaunchError::PortBind);
    }
    if TcpListener::bind(("127.0.0.1", requested)).is_ok() {
        Ok(requested)
    } else {
        bind_free_port().map_err(LaunchError::PortBind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_dunder_main_becomes_a_module_invocation() {
        assert_eq!(
            module_invocation("src/mcp_atlassian/__main__.py"),
            Some(("mcp_atlassian".to_string(), "src".to_string()))
        );
        assert_eq!(
            module_invocation("agent/__main__.py"),
            Some(("agent".to_string(), String::new()))
        );
        assert_eq!(
            module_invocation("agent/cli/__main__.py"),
            Some(("agent.cli".to_string(), String::new()))
        );
    }

    #[test]
    fn plain_scripts_are_not_module_invocations() {
        assert_eq!(module_invocation("main.py"), None);
        assert_eq!(module_invocation("src/main.py"), None);
        assert_eq!(module_invocation("index.js"), None);
    }

    #[test]
    fn a_bare_top_level_dunder_main_is_not_a_package() {
        // `__main__.py` alone has no package to be a member of, so `-m` would
        // have nothing to name — run it by path instead.
        assert_eq!(module_invocation("__main__.py"), None);
    }

    #[test]
    fn resolve_port_zero_returns_an_os_assigned_port() {
        let port = resolve_port(0).unwrap();
        assert!(port > 0);
    }

    #[test]
    fn resolve_port_returns_requested_when_available() {
        // Find a port that is currently free, release it, then request it.
        let free = bind_free_port().unwrap();
        assert_eq!(resolve_port(free).unwrap(), free);
    }

    #[test]
    fn resolve_port_falls_back_on_conflict() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let occupied = listener.local_addr().unwrap().port();
        let fallback = resolve_port(occupied).unwrap();
        assert_ne!(fallback, occupied);
        assert!(fallback > 0);
    }
}
