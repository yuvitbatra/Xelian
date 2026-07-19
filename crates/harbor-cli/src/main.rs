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

        /// Only prepare the package (pipeline steps 1–9: download, extract,
        /// install deps) without provisioning a model or launching it. Used by
        /// the Python SDK for `harbor.install()` (SPEC.md §15.2).
        #[arg(long)]
        install_only: bool,

        /// Run the full pipeline up to but not including launch (steps 1–10:
        /// adds model provisioning and permission disclosure on top of
        /// --install-only). Used by the Python SDK's `run()/agent()/mcp()` so
        /// those steps happen in the binary rather than being reimplemented in
        /// Python (SPEC.md §15.1). Prints the same HARBOR_INSTALLED line.
        #[arg(long, conflicts_with = "install_only")]
        prepare: bool,
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

    // --- Resolve registry URL and check credentials before validation ---
    let home = harbor_core::cache::HarborHome::resolve()?;
    let registry_url = harbor_core::auth::resolve_registry_url(&home);

    let creds = harbor_core::auth::read_credentials(&home)
        .map_err(|e| anyhow::anyhow!("failed to read credentials: {e}"))?
        .ok_or_else(|| anyhow::anyhow!("not logged in — run `harbor login` first"))?;

    println!("Validating package (manifest, lockfile, required files, entrypoint, commands)...");

    let outcome =
        harbor_core::validate::validate_and_build(&cwd, None).map_err(|e| anyhow::anyhow!(e))?;

    for warning in &outcome.warnings {
        println!("warning: {warning}");
    }

    println!("Computed package-checksum: {}", outcome.package_checksum);
    println!("Wrote harbor.lock");
    println!("Built {}", outcome.archive_path.display());

    // --- Upload to registry ---
    let client = harbor_core::registry_client::RegistryClient::new(&registry_url);
    let owner = &creds.username;
    let manifest_path = cwd.join("harbor.toml");
    let manifest_str =
        std::fs::read_to_string(&manifest_path).context("failed to read harbor.toml")?;
    let manifest = harbor_core::manifest::Manifest::from_toml_str(&manifest_str)
        .map_err(|e| anyhow::anyhow!("failed to parse harbor.toml: {e}"))?;

    println!(
        "Publishing {}/{} v{} to {} ...",
        owner,
        manifest.name,
        manifest.version,
        registry_url,
    );

    let lock_path = cwd.join("harbor.lock");

    match client.publish(
        &creds,
        owner,
        &manifest.name,
        &outcome.archive_path,
        &lock_path,
    ) {
        Ok(response) => {
            println!(
                "Published {} v{} successfully",
                response.name, response.version
            );
            Ok(())
        }
        Err(e) => {
            use harbor_core::registry_client::RegistryError;
            match &e {
                RegistryError::Auth(msg) => {
                    anyhow::bail!("authentication failed: {msg}")
                }
                RegistryError::Api { status, message } if *status == 409 => {
                    anyhow::bail!(
                        "version {} of {}/{} was already published (immutability, SPEC.md §19.2)",
                        manifest.version,
                        owner,
                        manifest.name,
                    )
                }
                RegistryError::Api { status, message } if *status == 403 => {
                    anyhow::bail!("{message}")
                }
                _ => anyhow::bail!("upload failed: {e}"),
            }
        }
    }
}

fn cmd_run(target: &str, install_only: bool, prepare: bool) -> anyhow::Result<()> {
    use std::io::Write;

    let home = harbor_core::cache::HarborHome::resolve()?;
    home.ensure_layout()?;

    // --- H-160: Target-form discrimination (SPEC.md §9.2) ---
    let run_target = harbor_core::run::parse_run_target(target)
        .map_err(|e| anyhow::anyhow!(e))?;

    match run_target {
        harbor_core::run::RunTarget::LocalArchive(path) => {
            let prepared = harbor_core::run::run_local_archive(&path, &home)
                .map_err(|e| anyhow::anyhow!(e))?;

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

            let manifest_path = prepared.package_dir.join("harbor.toml");
            let manifest_str = std::fs::read_to_string(&manifest_path)
                .map_err(|e| anyhow::anyhow!("Failed to read harbor.toml from cache: {}", e))?;
            let manifest = harbor_core::manifest::Manifest::from_toml_str(&manifest_str)
                .map_err(|e| anyhow::anyhow!("Failed to parse cached harbor.toml: {}", e))?;

            let env_dir = home.local_env_dir(&prepared.name, &prepared.version);

            // --- Prepare environment (runtime + deps) ---
            let prepared_env = harbor_core::run::prepare_environment(
                &prepared.package_dir, &manifest, &home, env_dir,
            ).map_err(|e| anyhow::anyhow!(e))?;

            let env_dir = &prepared_env.env_dir;
            let bin_dir = &prepared_env.bin_dir;

            if install_only {
                println!(
                    "HARBOR_INSTALLED|{}|{}|{}|{}|{}|{}|{}",
                    prepared.name,
                    prepared.version,
                    manifest.package_type,
                    manifest.language,
                    prepared.package_dir.display(),
                    env_dir.display(),
                    bin_dir.display(),
                );
                return Ok(());
            }

            let grants_path = home.local_grants_path(&prepared.name, &prepared.version);

            prepare_env_and_launch_inner(
                &manifest,
                &prepared.name,
                &prepared.version,
                &prepared.package_dir,
                env_dir,
                bin_dir,
                &grants_path,
                &home,
                !prepare,
            )
        }

        harbor_core::run::RunTarget::RegistryRef { owner, name } => {
            let registry_url = harbor_core::auth::resolve_registry_url(&home);
            eprintln!("resolving {owner}/{name} from registry at {registry_url} ...");

            let client = harbor_core::registry_client::RegistryClient::new(&registry_url);
            let info = client.fetch_metadata(&owner, &name)
                .map_err(|e| anyhow::anyhow!("failed to resolve {owner}/{name}: {e}"))?;

            let version = info.latest_version
                .ok_or_else(|| anyhow::anyhow!(
                    "no resolvable (non-yanked, non-pre-release) version of {owner}/{name}"
                ))?;

            eprintln!("resolved {owner}/{name} v{version}");

            // --- Check cache before downloading (§9.3) ---
            let pkg_dir = home.registry_package_dir(&owner, &name, &version);
            let from_cache = if pkg_dir.join("harbor.toml").is_file() {
                eprintln!("{}/{} v{} already cached at {}", owner, name, version, pkg_dir.display());
                Some(pkg_dir)
            } else {
                None
            };

            if let Some(cached_dir) = from_cache {
                let (manifest, warnings) = harbor_core::run::validate_extracted(&cached_dir)
                    .map_err(|e| anyhow::anyhow!(e))?;
                for warning in &warnings {
                    eprintln!("warning: {warning}");
                }
                eprintln!("prepared {}/{} v{} at {} (cached)", owner, name, version, cached_dir.display());

                let env_dir = home.registry_env_dir(&owner, &name, &version);
                let prepared_env = harbor_core::run::prepare_environment(
                    &cached_dir, &manifest, &home, env_dir,
                ).map_err(|e| anyhow::anyhow!(e))?;

                let env_dir = &prepared_env.env_dir;
                let bin_dir = &prepared_env.bin_dir;

                if install_only {
                    println!(
                        "HARBOR_INSTALLED|{}|{}|{}|{}|{}|{}|{}",
                        manifest.name,
                        manifest.version,
                        manifest.package_type,
                        manifest.language,
                        cached_dir.display(),
                        env_dir.display(),
                        bin_dir.display(),
                    );
                    return Ok(());
                }

                let grants_path = home.registry_grants_path(&owner, &name, &version);

                prepare_env_and_launch_inner(
                    &manifest,
                    &manifest.name,
                    &manifest.version,
                    &cached_dir,
                    env_dir,
                    bin_dir,
                    &grants_path,
                    &home,
                    !prepare,
                )
            } else {
                eprintln!("downloading {}/{} v{} ...", owner, name, version);
                let archive_bytes = client.download_archive(&owner, &name, &version)
                    .map_err(|e| anyhow::anyhow!("failed to download {owner}/{name} v{version}: {e}"))?;

                let tmp_dir = home.tmp();
                std::fs::create_dir_all(&tmp_dir)?;
                let archive_path = tmp_dir.join(format!("{}-{}-{}.harbor", owner, name, version));
                let mut f = std::fs::File::create(&archive_path)?;
                f.write_all(&archive_bytes)?;
                f.flush()?;

                let prepared = harbor_core::run::run_registry_archive(
                    &archive_path, &owner, &name, &home,
                ).map_err(|e| anyhow::anyhow!(e))?;

                let _ = std::fs::remove_file(&archive_path);

                for warning in &prepared.warnings {
                    eprintln!("warning: {warning}");
                }
                eprintln!(
                    "prepared {}/{} v{} at {}{}",
                    owner, name, prepared.version,
                    prepared.package_dir.display(),
                    if prepared.from_cache { " (cached)" } else { "" },
                );

                let manifest_path = prepared.package_dir.join("harbor.toml");
                let manifest_str = std::fs::read_to_string(&manifest_path)
                    .map_err(|e| anyhow::anyhow!("Failed to read harbor.toml from cache: {}", e))?;
                let manifest = harbor_core::manifest::Manifest::from_toml_str(&manifest_str)
                    .map_err(|e| anyhow::anyhow!("Failed to parse cached harbor.toml: {}", e))?;

                let env_dir = home.registry_env_dir(&owner, &name, &prepared.version);
                let prepared_env = harbor_core::run::prepare_environment(
                    &prepared.package_dir, &manifest, &home, env_dir,
                ).map_err(|e| anyhow::anyhow!(e))?;

                let env_dir = &prepared_env.env_dir;
                let bin_dir = &prepared_env.bin_dir;

                if install_only {
                    println!(
                        "HARBOR_INSTALLED|{}|{}|{}|{}|{}|{}|{}",
                        manifest.name,
                        manifest.version,
                        manifest.package_type,
                        manifest.language,
                        prepared.package_dir.display(),
                        env_dir.display(),
                        bin_dir.display(),
                    );
                    return Ok(());
                }

                let grants_path = home.registry_grants_path(&owner, &name, &prepared.version);

                prepare_env_and_launch_inner(
                    &manifest,
                    &prepared.name,
                    &prepared.version,
                    &prepared.package_dir,
                    env_dir,
                    bin_dir,
                    &grants_path,
                    &home,
                    !prepare,
                )
            }
        }

        harbor_core::run::RunTarget::GitHubUrl(url) => {
            eprintln!("importing {url} ...");
            let outcome = harbor_core::github::import_github(&url, &home)
                .map_err(|e| anyhow::anyhow!(e))?;

            let (manifest, warnings) = harbor_core::run::validate_extracted(&outcome.package_dir)
                .map_err(|e| anyhow::anyhow!(e))?;

            for warning in &warnings {
                eprintln!("warning: {warning}");
            }

            let cached_suffix = if outcome.from_cache { " (cached)" } else { "" };
            let short_sha = &outcome.sha[..outcome.sha.len().min(7)];
            eprintln!(
                "prepared {}/{}@{} at {}{}",
                outcome.repo.owner,
                outcome.repo.repo,
                short_sha,
                outcome.package_dir.display(),
                cached_suffix
            );

            let env_dir = home.github_env_dir(&outcome.repo.owner, &outcome.repo.repo, &outcome.sha);
            let prepared_env = harbor_core::run::prepare_environment(
                &outcome.package_dir, &manifest, &home, env_dir,
            ).map_err(|e| anyhow::anyhow!(e))?;

            let env_dir = &prepared_env.env_dir;
            let bin_dir = &prepared_env.bin_dir;

            if install_only {
                println!(
                    "HARBOR_INSTALLED|{}|{}|{}|{}|{}|{}|{}",
                    manifest.name,
                    manifest.version,
                    manifest.package_type,
                    manifest.language,
                    outcome.package_dir.display(),
                    env_dir.display(),
                    bin_dir.display(),
                );
                return Ok(());
            }

            let grants_path = home.github_grants_path(&outcome.repo.owner, &outcome.repo.repo, &outcome.sha);

            prepare_env_and_launch_inner(
                &manifest,
                &manifest.name,
                &manifest.version,
                &outcome.package_dir,
                env_dir,
                bin_dir,
                &grants_path,
                &home,
                !prepare,
            )
        }
    }
}

/// Shared pipeline tail for both `harbor run` and `harbor add` — after
/// environment preparation is done, this handles permissions, model
/// management, env var resolution, and launch.
///
/// Never returns on a non-zero child exit — it mirrors the exit code via
/// `std::process::exit`, exactly as callers expect, so callers can
/// distinguish outcomes as if they had run the entrypoint directly.
// Wide by design: this is the single shared pipeline tail threading already-
// resolved, distinct values (manifest, identity, the three cache dirs, grants,
// home, launch mode) straight into launch. Bundling them into a struct would
// only move the argument list, not remove it.
#[allow(clippy::too_many_arguments)]
fn prepare_env_and_launch_inner(
    manifest: &harbor_core::manifest::Manifest,
    name: &str,
    version: &str,
    package_dir: &std::path::Path,
    env_dir: &std::path::Path,
    bin_dir: &std::path::Path,
    grants_path: &std::path::Path,
    home: &harbor_core::cache::HarborHome,
    launch: bool,
) -> anyhow::Result<()> {
    eprintln!("environment ready at {}", env_dir.display());

    // --- Phase 9 / H-090: First-run permission prompt (disclosure-only). ---
    harbor_core::permissions::check_and_prompt(name, version, &manifest.permissions, grants_path)
        .map_err(|e| anyhow::anyhow!("permission error: {e}"))?;

    // --- Phase 10 / H-100, H-101: Model management (pipeline step 10, §9.1). ---
    harbor_core::run::model::ensure_model(manifest.primary_model.as_deref(), home)
        .map_err(|e| anyhow::anyhow!("model error: {e}"))?;

    // `--prepare` (SPEC.md §15.2 `run()` minus launch): the SDK owns the final
    // process spawn to hand back a chat/expose handle, but permissions (above)
    // and model provisioning MUST run in the binary — not be reimplemented in
    // Python. Emit the install descriptor and stop before launch (step 11).
    if !launch {
        println!(
            "HARBOR_INSTALLED|{}|{}|{}|{}|{}|{}|{}",
            name,
            version,
            manifest.package_type,
            manifest.language,
            package_dir.display(),
            env_dir.display(),
            bin_dir.display(),
        );
        return Ok(());
    }

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
    let prepared_env = harbor_core::run::prepare_environment(
        &outcome.package_dir, &manifest, &home, env_dir,
    ).map_err(|e| anyhow::anyhow!(e))?;

    let env_dir = &prepared_env.env_dir;
    let bin_dir = &prepared_env.bin_dir;

    let grants_path = home.github_grants_path(&outcome.repo.owner, &outcome.repo.repo, &outcome.sha);

    prepare_env_and_launch_inner(
        &manifest,
        &manifest.name,
        &manifest.version,
        &outcome.package_dir,
        env_dir,
        bin_dir,
        &grants_path,
        &home,
        true,
    )
}

fn cmd_list() -> anyhow::Result<()> {
    let home = harbor_core::cache::HarborHome::resolve()?;
    let packages = harbor_core::cache::list_cached_packages(&home)
        .map_err(|e| anyhow::anyhow!("failed to list cached packages: {e}"))?;

    if packages.is_empty() {
        println!("No cached packages.");
        return Ok(());
    }

    for pkg in &packages {
        let source = match &pkg.source {
            harbor_core::cache::PackageSource::Local => "local ",
            harbor_core::cache::PackageSource::Github { .. } => "github",
            harbor_core::cache::PackageSource::Registry { .. } => "regsty",
        };
        println!("{source}  {:<30}  {}", pkg.name, pkg.version);
    }
    Ok(())
}

fn cmd_rm(target: Option<&str>, remove_env: bool, all: bool) -> anyhow::Result<()> {
    let home = harbor_core::cache::HarborHome::resolve()?;

    if all {
        harbor_core::cache::remove_all(&home)
            .map_err(|e| anyhow::anyhow!("failed to clear cache: {e}"))?;
        println!(
            "Cleared packages/, envs/, runtimes/, models/ (credentials.toml left intact)."
        );
        return Ok(());
    }

    let target = target.unwrap(); // guaranteed by clap's required_unless_present
    // `owner/name` addresses registry/github packages; a bare `name` addresses
    // a local package (built/run from a local `.harbor` path), which has no
    // owner namespace and is stored under `packages/local/<name>/`.
    let outcome = match target.split_once('/') {
        Some((owner, name)) => harbor_core::cache::remove_packages(&home, owner, name, remove_env),
        None => harbor_core::cache::remove_local_packages(&home, target, remove_env),
    }
    .map_err(|e| anyhow::anyhow!("failed to remove {target}: {e}"))?;

    for p in &outcome.removed_packages {
        println!("Removed {}", p.display());
    }
    for e in &outcome.removed_envs {
        println!("Removed environment {}", e.display());
    }

    if outcome.removed_packages.is_empty() {
        eprintln!("No cached packages matched {target}.");
    }
    Ok(())
}

fn cmd_login() -> anyhow::Result<()> {
    use anyhow::Context;

    let home = harbor_core::cache::HarborHome::resolve()?;

    let registry_url = harbor_core::auth::resolve_registry_url(&home);

    eprint!("Registry username: ");
    std::io::Write::flush(&mut std::io::stderr()).ok();
    let mut username = String::new();
    std::io::stdin()
        .read_line(&mut username)
        .context("failed to read username")?;
    let username = username.trim().to_string();

    let password = rpassword::prompt_password("Registry password: ")
        .context("failed to read password")?;

    let client = harbor_core::registry_client::RegistryClient::new(&registry_url);

    match client.login(&username, &password) {
        Ok(response) => {
            let creds = harbor_core::auth::StoredCredentials {
                token: response.token,
                username: response.username.clone(),
                registry_url,
            };
            harbor_core::auth::write_credentials(&home, &creds)
                .map_err(|e| anyhow::anyhow!("failed to store credentials: {e}"))?;
            println!("Logged in as {}", response.username);
            Ok(())
        }
        Err(e) => {
            use harbor_core::registry_client::RegistryError;
            match &e {
                RegistryError::Auth(msg) => {
                    anyhow::bail!("login failed: {msg}")
                }
                RegistryError::Network(msg) => {
                    anyhow::bail!("cannot reach registry at {}: {msg}", registry_url)
                }
                _ => anyhow::bail!("login failed: {e}"),
            }
        }
    }
}

fn cmd_logout() -> anyhow::Result<()> {
    let home = harbor_core::cache::HarborHome::resolve()?;
    harbor_core::auth::delete_credentials(&home)
        .map_err(|e| anyhow::anyhow!("failed to remove credentials: {e}"))?;
    println!("Logged out.");
    Ok(())
}

fn cmd_yank(target: &str, version: &str, undo: bool) -> anyhow::Result<()> {
    let target = target.trim();
    let (owner, name) = target
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!(
            "yank target must be in owner/name format, got {target:?}"
        ))?;

    let home = harbor_core::cache::HarborHome::resolve()?;
    let registry_url = harbor_core::auth::resolve_registry_url(&home);

    let creds = harbor_core::auth::read_credentials(&home)
        .map_err(|e| anyhow::anyhow!("failed to read credentials: {e}"))?
        .ok_or_else(|| anyhow::anyhow!("not logged in — run `harbor login` first"))?;

    let client = harbor_core::registry_client::RegistryClient::new(&registry_url);
    let yanked = !undo;

    let action = if yanked { "yank" } else { "unyank" };
    eprintln!("{action}ing {target} v{version} ...");

    client
        .yank(&creds, owner, name, version, yanked)
        .map_err(|e| {
            use harbor_core::registry_client::RegistryError;
            match &e {
                RegistryError::Auth(msg) => {
                    anyhow::anyhow!("authentication failed: {msg}")
                }
                RegistryError::Api { status: 403, message } => {
                    anyhow::anyhow!("{message}")
                }
                RegistryError::Api { status: 404, message } => {
                    anyhow::anyhow!("{message}")
                }
                _ => anyhow::anyhow!("failed to {action} {target} v{version}: {e}"),
            }
        })?;

    let past = if yanked { "yanked" } else { "unyanked" };
    println!("{target} v{version} {past}");
    Ok(())
}

fn main() {
    let cli = Cli::parse();

    let result = match &cli.command {
        Command::Init { force } => cmd_init(*force),
        Command::Push => cmd_push(),
        Command::Run { target, install_only, prepare } => cmd_run(target, *install_only, *prepare),
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
