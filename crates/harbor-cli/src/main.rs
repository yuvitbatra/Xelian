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
    Init,

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

fn cmd_init() -> anyhow::Result<()> {
    not_implemented("init")
}

fn cmd_push() -> anyhow::Result<()> {
    not_implemented("push")
}

fn cmd_run(_target: &str) -> anyhow::Result<()> {
    not_implemented("run")
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
        Command::Init => cmd_init(),
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
