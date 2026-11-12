mod audit;
mod files;
mod generation;
mod home;
mod packages;
mod units;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use std::borrow::BorrowMut;
use std::io::{BufRead, IsTerminal, Write};
use std::process::{Command, Stdio};

/// Declarative, reconciling management of an Arch Linux host from a Nix flake.
///
/// The flake output pacnixConfigurations.<host> comes from pacnix.lib.pacnixSystem.
/// pacman, aurutils and systemd do all changes to the system.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Flake that contains the configuration
    #[arg(long, global = true, default_value = "/etc/pacnix")]
    flake: String,
    /// Host name in pacnixConfigurations (default: the system host name)
    #[arg(long, global = true)]
    host: Option<String>,
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Subcommand)]
enum Verb {
    /// Build the generation and show each change to the system. Change nothing.
    Plan,
    /// Build the generation and do the plan. Do not refresh the sync databases.
    Apply(Opts),
    /// Upgrade with pacman -Syu, rebuild changed AUR packages, then apply.
    Upgrade(Opts),
    /// Apply the previous generation.
    Rollback(Opts),
    /// Show state that no declaration explains. Change nothing.
    Audit,
}

#[derive(Args, Clone, Default)]
pub struct Opts {
    /// Do not ask for confirmation before packages are removed
    #[arg(long, short)]
    pub yes: bool,
    /// Permit removal of base, pacman, nix, pacnix and the booted kernel
    #[arg(long)]
    pub allow_protected_removal: bool,
    /// Permit replacement of files that no package and no generation owns
    #[arg(long)]
    pub overwrite_unmanaged: bool,
}

pub struct Ctx {
    pub flake: String,
    pub host: String,
    pub opts: Opts,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("pacnix: error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    if unsafe { libc::geteuid() } != 0 {
        bail!("run pacnix as root");
    }
    let host = match cli.host {
        Some(h) => h,
        None => std::fs::read_to_string("/etc/hostname")
            .context("read /etc/hostname, or give --host")?
            .trim()
            .to_string(),
    };
    let flake = match std::fs::canonicalize(&cli.flake) {
        Ok(p) => p.display().to_string(),
        Err(_) => cli.flake.clone(),
    };
    let _lock = lock()?;
    let ctx = |opts| Ctx { flake: flake.clone(), host: host.clone(), opts };
    match cli.verb {
        Verb::Plan => generation::plan(&ctx(Opts::default())),
        Verb::Apply(o) => generation::apply(&ctx(o)),
        Verb::Upgrade(o) => generation::upgrade(&ctx(o)),
        Verb::Rollback(o) => generation::rollback(&ctx(o)),
        Verb::Audit => audit::audit(&ctx(Opts::default())),
    }
}

fn lock() -> Result<std::fs::File> {
    let f = std::fs::File::create("/run/lock/pacnix.lock")?;
    if unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&f), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!("a different pacnix process is running");
    }
    Ok(f)
}

/// Runs a command with inherited output and fails on a nonzero exit status.
pub fn run_cmd(mut cmd: impl BorrowMut<Command>) -> Result<()> {
    let cmd = cmd.borrow_mut();
    let status = cmd.status().with_context(|| format!("run {cmd:?}"))?;
    if !status.success() {
        bail!("{cmd:?} failed with {status}");
    }
    Ok(())
}

/// Runs a command and returns its standard output. Standard error stays visible.
pub fn output(mut cmd: impl BorrowMut<Command>) -> Result<String> {
    let cmd = cmd.borrow_mut();
    let out = cmd.stderr(Stdio::inherit()).output().with_context(|| format!("run {cmd:?}"))?;
    if !out.status.success() {
        bail!("{cmd:?} failed with {}", out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Returns standard output and ignores the exit status.
pub fn output_any(mut cmd: impl BorrowMut<Command>) -> Result<String> {
    let cmd = cmd.borrow_mut();
    let out = cmd.stderr(Stdio::null()).output().with_context(|| format!("run {cmd:?}"))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn cmd(program: &str, args: &[&str]) -> Command {
    let mut c = Command::new(program);
    c.args(args);
    c
}

pub fn confirm(question: &str, opts: &Opts) -> Result<()> {
    if opts.yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        bail!("{question} Give --yes to continue without a terminal.");
    }
    print!("{question} [y/N] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    if !matches!(answer.trim(), "y" | "Y" | "yes") {
        bail!("stopped by the user");
    }
    Ok(())
}
