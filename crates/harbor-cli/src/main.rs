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

    for warning in &prepared.warnings {
        println!("warning: {warning}");
    }

    let cached_suffix = if prepared.from_cache { " (cached)" } else { "" };
    println!(
        "prepared {}@{} at {}{}",
        prepared.name,
        prepared.version,
        prepared.package_dir.display(),
        cached_suffix
    );
    println!("launch not yet implemented (Phase 8)");

    Ok(())
}

fn cmd_add(_url: &str) -> anyhow::Result<()> {
    not_implemented("add")
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
