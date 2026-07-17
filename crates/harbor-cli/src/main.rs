use clap::{Parser, Subcommand};

/// Harbor: a local-first registry and runtime for AI agents and MCP servers.
#[derive(Parser, Debug)]
#[command(name = "harbor", version = env!("CARGO_PKG_VERSION"), about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Create a new package skeleton (harbor.toml, harbor.lock) in the current directory.
    ///
    /// Performs no network activity. Fails if harbor.toml already exists in
    /// the current directory, unless --force is given, in which case both
    /// harbor.toml and harbor.lock are overwritten.
    Init {
        /// Overwrite an existing harbor.toml and harbor.lock in the current directory.
        #[arg(long)]
        force: bool,
    },

    /// Validate and publish the current package to the registry.
    Push,

    /// Download (if necessary) and run a package.
    Run {
        /// Registry ref (owner/package), GitHub URL, or local .harbor path.
        target: String,
    },

    /// Import a GitHub repository as a local Harbor package and run it.
    Add {
        /// GitHub repository URL.
        url: String,
    },

    /// List locally cached packages.
    List,

    /// Remove cached package state from ~/.harbor/.
    Rm {
        /// Package to remove (owner/package). Omit only when using --all.
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        target: Option<String>,

        /// Also remove the package's cached environment.
        #[arg(long, requires = "target", conflicts_with = "all")]
        env: bool,

        /// Remove everything under packages/, envs/, runtimes/, and models/.
        #[arg(long)]
        all: bool,
    },

    /// Authenticate the CLI against the registry.
    Login,

    /// Remove the stored registry credential.
    Logout,

    /// Mark a published version as yanked, or reverse that.
    Yank {
        /// Package to yank (owner/package).
        target: String,

        /// Version to yank (required).
        #[arg(long, required = true)]
        version: String,

        /// Reverse a previous yank instead of applying one.
        #[arg(long)]
        undo: bool,
    },
}

fn not_implemented(cmd: &str) -> anyhow::Result<()> {
    anyhow::bail!("harbor {cmd}: not implemented")
}

fn cmd_init(force: bool) -> anyhow::Result<()> {
    use anyhow::Context;

    let cwd = std::env::current_dir().context("failed to determine the current directory")?;

    match harbor_core::init::init_package(&cwd, force) {
        Ok(outcome) => {
            println!("Created {}", outcome.manifest_path.display());
            println!("Created {}", outcome.lockfile_path.display());
            if outcome.name_is_placeholder {
                println!(
                    "Note: the current directory name isn't a valid package name, \
                     so harbor.toml uses the placeholder name {:?} — edit `name` \
                     before running `harbor push`.",
                    outcome.name
                );
            }
            Ok(())
        }
        Err(err) => Err(anyhow::anyhow!(err)),
    }
}

fn cmd_push() -> anyhow::Result<()> {
    use anyhow::Context;

    let cwd = std::env::current_dir().context("failed to determine the current directory")?;

    println!("Validating package (manifest, lockfile, required files, entrypoint, commands)...");

    let outcome =
        harbor_core::validate::validate_and_build(&cwd, None).map_err(|e| anyhow::anyhow!(e))?;

    for warning in &outcome.warnings {
        println!("warning: {warning}");
    }

    println!("Computed package-checksum: {}", outcome.package_checksum);
    println!("Wrote harbor.lock");
    println!("Built {}", outcome.archive_path.display());

    anyhow::bail!("error: registry upload not yet implemented (Phase 15)")
}

/// Shared pipeline tail for both `harbor run` and `harbor add` (SPEC.md
/// §9.7 onward): environment preparation, first-run permission prompt, model
/// management, env var resolution, launch, and exit-code mirroring.
///
/// `env_dir` and `grants_path` are caller-supplied so this function makes no
/// assumption about a package's source: `cmd_run` derives them from
/// `home.local_*`, `cmd_add` from `home.github_*` (SPEC.md §12.2 step 7).
///
/// Never returns on a non-zero child exit — it mirrors the exit code via
/// `std::process::exit`, exactly as `cmd_run` always has, so callers of
/// `harbor run`/`harbor add` can distinguish outcomes as if they had run the
/// entrypoint directly.
fn prepare_env_and_launch(
    manifest: &harbor_core::manifest::Manifest,
    name: &str,
    version: &str,
    package_dir: &std::path::Path,
    env_dir: std::path::PathBuf,
    grants_path: &std::path::Path,
    home: &harbor_core::cache::HarborHome,
) -> anyhow::Result<()> {
    let prepared_env = harbor_core::run::prepare_environment(package_dir, manifest, home, env_dir)
        .map_err(|e| anyhow::anyhow!(e))?;

    let env_dir = &prepared_env.env_dir;
    let bin_dir = &prepared_env.bin_dir;

    eprintln!("environment ready at {}", env_dir.display());

    // --- Phase 9 / H-090: First-run permission prompt (disclosure-only). ---
    harbor_core::permissions::check_and_prompt(name, version, &manifest.permissions, grants_path)
        .map_err(|e| anyhow::anyhow!("permission error: {e}"))?;

    // --- Phase 10 / H-100, H-101: Model management (pipeline step 10, §9.1). ---
    harbor_core::run::model::ensure_model(manifest.primary_model.as_deref(), home)
        .map_err(|e| anyhow::anyhow!("model error: {e}"))?;

    // --- Phase 8 / H-080: Resolve required/default environment variables,
    // immediately before launch per §9.10. ---
    let env_pairs = harbor_core::run::env_vars::resolve_env_vars(&manifest.environment)
        .map_err(|e| anyhow::anyhow!(e))?;

    // --- Phase 8 / H-081, H-082: Launch (agent REPL or MCP server). ---
    let status =
        harbor_core::run::launch::launch(manifest, package_dir, env_dir, bin_dir, &env_pairs)
            .map_err(|e| anyhow::anyhow!("launch error: {e}"))?;

    // Mirror the entrypoint's exit code so callers of `harbor run`/`harbor
    // add` can distinguish outcomes exactly as if they had run the
    // entrypoint directly.
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

fn cmd_run(target: &str) -> anyhow::Result<()> {
    // Target discrimination (decision 2026-07-16): only the local `.harbor`
    // path form is implemented so far. Registry refs and GitHub URLs still
    // fall through to the "not implemented" stub; full discrimination among
    // all three target forms is a later task (H-160).
    let path = std::path::Path::new(target);
    let is_local_archive = target.ends_with(".harbor") || path.is_file();

    if !is_local_archive {
        return not_implemented("run");
    }

    let home = harbor_core::cache::HarborHome::resolve()?;
    home.ensure_layout()?;

    let prepared =
        harbor_core::run::run_local_archive(path, &home).map_err(|e| anyhow::anyhow!(e))?;

    // All of Harbor's own status output goes to stderr: for MCP packages the
    // child inherits Harbor's stdout as the JSON-RPC stdio transport
    // (SPEC.md §9.10.2), so stdout must carry nothing but the protocol.
    for warning in &prepared.warnings {
        eprintln!("warning: {warning}");
    }

    let cached_suffix = if prepared.from_cache { " (cached)" } else { "" };
    eprintln!(
        "prepared {}@{} at {}{}",
        prepared.name,
        prepared.version,
        prepared.package_dir.display(),
        cached_suffix
    );

    // Load manifest from extracted package directory to pass to prepare_environment
    let manifest_path = prepared.package_dir.join("harbor.toml");
    let manifest_str = std::fs::read_to_string(&manifest_path)
        .map_err(|e| anyhow::anyhow!("Failed to read harbor.toml from cache: {}", e))?;
    let manifest = harbor_core::manifest::Manifest::from_toml_str(&manifest_str)
        .map_err(|e| anyhow::anyhow!("Failed to parse cached harbor.toml: {}", e))?;

    let env_dir = home.local_env_dir(&prepared.name, &prepared.version);
    let grants_path = home.local_grants_path(&prepared.name, &prepared.version);

    prepare_env_and_launch(
        &manifest,
        &prepared.name,
        &prepared.version,
        &prepared.package_dir,
        env_dir,
        &grants_path,
        &home,
    )
}

/// `harbor add <github-url>` (SPEC.md §12): import a GitHub repository as a
/// local Harbor package, then run it through the same execution pipeline as
/// `harbor run`, starting from manifest validation (§9.6) onward (§12.2 step
/// 7). Performs no publishing (§12.3).
fn cmd_add(url: &str) -> anyhow::Result<()> {
    let home = harbor_core::cache::HarborHome::resolve()?;
    home.ensure_layout()?;

    let outcome = harbor_core::github::import_github(url, &home).map_err(|e| anyhow::anyhow!(e))?;

    let (manifest, warnings) =
        harbor_core::run::validate_extracted(&outcome.package_dir).map_err(|e| anyhow::anyhow!(e))?;

    // All of Harbor's own status output goes to stderr: for MCP packages the
    // child inherits Harbor's stdout as the JSON-RPC stdio transport
    // (SPEC.md §9.10.2), so stdout must carry nothing but the protocol.
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }

    let cached_suffix = if outcome.from_cache { " (cached)" } else { "" };
    let short_sha = &outcome.sha[..outcome.sha.len().min(7)];
    eprintln!(
        "imported {}/{}@{} at {}{}",
        outcome.repo.owner,
        outcome.repo.repo,
        short_sha,
        outcome.package_dir.display(),
        cached_suffix
    );

    let env_dir = home.github_env_dir(&outcome.repo.owner, &outcome.repo.repo, &outcome.sha);
    let grants_path = home.github_grants_path(&outcome.repo.owner, &outcome.repo.repo, &outcome.sha);

    prepare_env_and_launch(
        &manifest,
        &manifest.name,
        &manifest.version,
        &outcome.package_dir,
        env_dir,
        &grants_path,
        &home,
    )
}

fn cmd_list() -> anyhow::Result<()> {
    not_implemented("list")
}

fn cmd_rm(_target: Option<&str>, _env: bool, _all: bool) -> anyhow::Result<()> {
    not_implemented("rm")
}

fn cmd_login() -> anyhow::Result<()> {
    not_implemented("login")
}

fn cmd_logout() -> anyhow::Result<()> {
    not_implemented("logout")
}

fn cmd_yank(_target: &str, _version: &str, _undo: bool) -> anyhow::Result<()> {
    not_implemented("yank")
}

fn main() {
    let cli = Cli::parse();

    let result = match &cli.command {
        Command::Init { force } => cmd_init(*force),
        Command::Push => cmd_push(),
        Command::Run { target } => cmd_run(target),
        Command::Add { url } => cmd_add(url),
        Command::List => cmd_list(),
        Command::Rm { target, env, all } => cmd_rm(target.as_deref(), *env, *all),
        Command::Login => cmd_login(),
        Command::Logout => cmd_logout(),
        Command::Yank {
            target,
            version,
            undo,
        } => cmd_yank(target, version, *undo),
    };

    if let Err(err) = result {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
