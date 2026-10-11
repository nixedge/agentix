mod profile;

use anyhow::{Context, Result};
use clap::Parser;
use profile::{Profile, ProfileOptions};
use std::{collections::HashMap, env, path::PathBuf, process::Command};

#[derive(Parser)]
#[command(
    name = "ax-jail",
    about = "Run ax in an isolated project with Nix daemon access"
)]
struct Args {
    #[arg(long = "ro", value_name = "PATH")]
    ro_paths: Vec<PathBuf>,
    #[arg(long = "rw", value_name = "PATH")]
    rw_paths: Vec<PathBuf>,
    #[arg(long = "all-worktrees")]
    all_worktrees: bool,
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    ax_args: Vec<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let bin_dir = PathBuf::from(
        env::var("AX_JAIL_BIN_DIR").context("run ax-jail through its Nix package wrapper")?,
    );
    let profile = Profile::build(ProfileOptions {
        home: PathBuf::from(env::var("HOME").context("HOME not set")?),
        cwd: env::current_dir()?,
        bin_dir: bin_dir.clone(),
        nix_socket: env::var_os("AGENTIC_NIX_DAEMON")
            .map(PathBuf::from)
            .unwrap_or_else(|| "/nix/var/nix/daemon-socket/socket".into()),
        ro_paths: args.ro_paths,
        rw_paths: args.rw_paths,
        all_worktrees: args.all_worktrees,
        environment: env::vars().collect::<HashMap<_, _>>(),
    })?;
    eprintln!("[ax-jail] project: {}", profile.root.display());
    let status = Command::new(env::var("AX_JAIL_BWRAP").unwrap_or_else(|_| "bwrap".into()))
        .args(&profile.args)
        .arg("--")
        .arg("ax")
        .args(args.ax_args)
        .status()
        .context("launching bubblewrap")?;
    drop(profile);
    std::process::exit(status.code().unwrap_or(130));
}
