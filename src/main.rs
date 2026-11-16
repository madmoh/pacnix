mod audit;
mod files;
mod generation;
mod home;
mod packages;
mod report;
mod trust;
mod units;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use std::borrow::BorrowMut;
use std::io::{BufRead, IsTerminal, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

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
    /// Remove pacnix, Nix and their data. The declared files, units and packages stay.
    Uninstall(Opts),
}

#[derive(Args, Clone, Default)]
pub struct Opts {
    /// Accept package removals, new maintainers or AUR dependencies of AUR packages, the Arch Linux news and the deletion of the data of removed features without a question
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
    // Like paru, pacnix runs as the user and elevates one time, before any
    // change, so that a long build cannot outlast the sudo timestamp.
    if unsafe { libc::geteuid() } != 0 {
        let exe = std::env::current_exe()?;
        let e = Command::new("sudo").arg("--").arg(exe).args(std::env::args_os().skip(1)).exec();
        return Err(e).context("run pacnix with sudo");
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
        Verb::Apply(o) => resilient(|| generation::apply(&ctx(o.clone()))),
        Verb::Upgrade(o) => resilient(|| generation::upgrade(&ctx(o.clone()))),
        Verb::Rollback(o) => resilient(|| generation::rollback(&ctx(o.clone()))),
        Verb::Audit => audit::audit(&ctx(Opts::default())),
        Verb::Uninstall(o) => resilient(|| generation::uninstall(&ctx(o.clone()))),
    }
}

/// Messages of curl, also inside the messages of pacman, git and Nix, for a
/// cause in the network: no connection, DNS, a stalled mirror, a download that
/// times out, or a server error (5xx). A missing file (404) is not one of them.
const NETWORK: &[&str] = &[
    "Could not resolve host",
    "Couldn't resolve host",
    "Resolving timed out",
    "Connection timed out",
    "Operation timed out",
    "Operation too slow",
    "Timeout was reached",
    "Failed to connect to",
    "Could not connect to server",
    "Couldn't connect to server",
    "Connection reset by peer",
    "Recv failure",
    "SSL connect error",
    "Empty reply from server",
    "transfer closed with",
    "returned error: 5",
    "HTTP error 5",
];

enum Cause {
    /// The network, with the line that shows it.
    Network(String),
    /// A package file that the mirror does not have: the sync databases are old.
    Stale,
    Other,
}

/// The cause of a failure, from the error lines of the download tools only:
/// pacman, curl, git and Nix. Other output, for example of a build that
/// prints "timed out", does not count.
fn cause(e: &anyhow::Error) -> Cause {
    let lines = e.chain().filter_map(|c| c.downcast_ref::<Failed>()).flat_map(|f| f.lines.iter());
    for line in lines {
        let l = line.trim();
        let pacman = l.strip_prefix("error: failed retrieving file '").and_then(|r| r.split_once(" : ")).map(|x| x.1);
        let message = pacman
            .or_else(|| l.starts_with("curl: (").then_some(l))
            .or_else(|| l.strip_prefix("fatal: unable to access '").and_then(|r| r.split_once("': ")).map(|x| x.1))
            .or_else(|| l.split_once("error: unable to download '").and_then(|(_, r)| r.split_once("': ")).map(|x| x.1))
            .or_else(|| l.starts_with("error: RPC failed").then_some("Recv failure"))
            .or_else(|| l.ends_with("Truncated tar archive detected while reading data").then_some("transfer closed with"));
        let Some(message) = message else { continue };
        if pacman.is_some() && message.contains("returned error: 404") {
            return Cause::Stale;
        }
        if NETWORK.iter().any(|n| message.contains(n)) {
            return Cause::Network(l.to_string());
        }
    }
    Cause::Other
}

/// The pauses before the next attempts after a failure in the network.
const PAUSES: [u64; 3] = [10, 30, 60];

/// The retry policy of pacnix. A failure in the network repeats the verb up to
/// three times in about two minutes, with one download at a time. The next
/// attempt continues from the work that is done: the packages in the pacman
/// cache and in the local repository. Each other failure, for example an
/// evaluation error, a refused plan, a build or a trigger that fails, stops
/// pacnix one time with the reason.
fn resilient(f: impl Fn() -> Result<()>) -> Result<()> {
    report::begin();
    for (attempt, pause) in PAUSES.iter().enumerate() {
        match f() {
            Ok(()) => return Ok(report::summary()),
            Err(e) => match cause(&e) {
                Cause::Network(line) => {
                    report::progress(&format!("attempt {} of {} failed in the network: {line}. Next attempt in {pause} s, with one download at a time", attempt + 1, PAUSES.len() + 1));
                    packages::SERIAL.store(true, std::sync::atomic::Ordering::Relaxed);
                    std::thread::sleep(Duration::from_secs(*pause));
                }
                Cause::Stale => return Err(e.context("the mirror does not have a package of the sync databases, thus the databases are old. Run pacnix upgrade")),
                Cause::Other => return Err(e),
            },
        }
    }
    f().map(|_| report::summary())
}

static ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static DELETE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The one question of a run: it shows each item that the user must accept and
/// asks one time. --yes accepts. Without a terminal and without --yes, pacnix
/// stops. The answer k continues and keeps the data of `data`. Returns true
/// when the user accepts the deletion of the data. Later calls in the same run
/// do not ask again and return the same answer.
pub fn ask(items: &[String], data: &[String], opts: &Opts) -> Result<bool> {
    use std::sync::atomic::Ordering::Relaxed;
    if (items.is_empty() && data.is_empty()) || ASKED.swap(true, Relaxed) {
        return Ok(DELETE.load(Relaxed));
    }
    for i in items.iter().chain(data) {
        println!("{i}");
    }
    if !opts.yes && !std::io::stdin().is_terminal() {
        bail!("these items need a confirmation. Give --yes to continue without a terminal");
    }
    let mut answer = String::from("y");
    if !opts.yes {
        print!("Continue? [y/N]{} ", if data.is_empty() { "" } else { " (k: continue and keep the data)" });
        std::io::stdout().flush()?;
        answer.clear();
        std::io::stdin().lock().read_line(&mut answer)?;
    }
    match answer.trim() {
        "y" | "Y" | "yes" => DELETE.store(!data.is_empty(), Relaxed),
        "k" if !data.is_empty() => {}
        _ => bail!("stopped by the user"),
    }
    Ok(DELETE.load(Relaxed))
}

fn lock() -> Result<std::fs::File> {
    let f = std::fs::File::create("/run/lock/pacnix.lock")?;
    if unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&f), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!("a different pacnix process is running");
    }
    Ok(f)
}

/// The command line, without the runuser and env prefix of the builder commands.
pub fn describe(cmd: &Command) -> String {
    let words: Vec<String> = std::iter::once(cmd.get_program()).chain(cmd.get_args()).map(|w| w.to_string_lossy().into_owned()).collect();
    let mut start = 0;
    if words[0] == "runuser" {
        start = words.iter().position(|w| w == "--").map_or(0, |i| i + 1);
        if words.get(start).is_some_and(|w| w == "env") {
            start += 1 + words[start + 1..].iter().take_while(|w| w.contains('=') || w.starts_with('-')).count();
        }
    }
    words[start..].join(" ")
}

/// A failed command. The message has the last lines of its output; all the
/// kept lines are for the retry policy.
#[derive(Debug)]
pub struct Failed {
    message: String,
    lines: Vec<String>,
}

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Failed {}

/// The error of a failed command, with the last lines of its output.
pub fn failed(cmd: &Command, status: ExitStatus, lines: &[String]) -> anyhow::Error {
    let tail: Vec<&str> = lines.iter().rev().take(6).rev().map(|l| l.as_str()).collect();
    let message = match tail.is_empty() {
        true => format!("{} failed with {status}", describe(cmd)),
        false => format!("{} failed with {status}. Its last output:\n  {}", describe(cmd), tail.join("\n  ")),
    };
    Failed { message, lines: lines.to_vec() }.into()
}

/// Runs a command and fails on a nonzero exit status.
pub fn run_cmd(mut cmd: impl BorrowMut<Command>) -> Result<()> {
    execute(cmd.borrow_mut(), true)
}

/// Runs a command without its standard output and fails on a nonzero exit status.
pub fn run_quiet(mut cmd: impl BorrowMut<Command>) -> Result<()> {
    execute(cmd.borrow_mut(), false)
}

/// Runs a command without output and fails on a nonzero exit status.
pub fn run_silent(mut cmd: impl BorrowMut<Command>) -> Result<()> {
    let cmd = cmd.borrow_mut();
    let status = cmd.stdout(Stdio::null()).stderr(Stdio::null()).status().with_context(|| format!("run {}", describe(cmd)))?;
    if !status.success() {
        return Err(failed(cmd, status, &[]));
    }
    Ok(())
}

fn execute(cmd: &mut Command, stdout: bool) -> Result<()> {
    let (status, lines) = forward(cmd, 20, stdout, |_| true)?;
    if !status.success() {
        return Err(failed(cmd, status, &lines));
    }
    Ok(())
}

/// Runs a command with standard output and standard error in one pipe, and
/// writes each complete line to standard output, also on a terminal, so that
/// the output is the same in the journal and the retry policy can read it. A carriage return without a
/// line feed starts the line again, thus the redraws of progress meters do not
/// go to the journal. Terminal escape sequences are removed, because journalctl
/// shows a line with control characters as a blob. When the command writes no
/// line for 60 s, pacnix writes one. Without `stdout`, the standard output of
/// the command is dropped. Lines that `show` refuses are dropped too.
/// Returns the exit status and the last `keep` lines.
pub fn forward(cmd: &mut Command, keep: usize, stdout: bool, mut show: impl FnMut(&str) -> bool) -> Result<(ExitStatus, Vec<String>)> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    let child = cmd.stdout(if stdout { Stdio::from(write.try_clone()?) } else { Stdio::null() }).stderr(write).spawn();
    // The Command holds the write end until it gets other values.
    cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    let mut child = child.with_context(|| format!("run {}", describe(cmd)))?;
    let mut pipe = std::fs::File::from(read);
    let (mut lines, mut line, mut redraw) = (std::collections::VecDeque::new(), Vec::new(), false);
    let mut emit = |line: &mut Vec<u8>| {
        let text = strip_escapes(line);
        line.clear();
        if !text.is_empty() && show(&text) {
            println!("{text}");
            if lines.len() == keep {
                lines.pop_front();
            }
            lines.push_back(text);
            return true;
        }
        false
    };
    let (start, mut quiet, mut buf) = (Instant::now(), Instant::now(), [0u8; 8192]);
    loop {
        let mut fd = libc::pollfd { fd: pipe.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        if unsafe { libc::poll(&mut fd, 1, 1000) } == 0 {
            // A daemon that the command started can keep the pipe open.
            if child.try_wait()?.is_some() {
                break;
            }
            if quiet.elapsed() >= Duration::from_secs(60) {
                report::heartbeat(&describe(cmd), start.elapsed().as_secs() / 60);
                quiet = Instant::now();
            }
            continue;
        }
        let n = match pipe.read(&mut buf) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            r => r?,
        };
        if n == 0 {
            break;
        }
        for &b in &buf[..n] {
            if redraw && b != b'\n' {
                line.clear();
            }
            redraw = b == b'\r';
            match b {
                b'\n' if emit(&mut line) => quiet = Instant::now(),
                b'\n' | b'\r' => {}
                _ => line.push(b),
            }
        }
    }
    emit(&mut line);
    Ok((child.wait()?, lines.into()))
}

/// Removes ANSI escape sequences and other control characters except tabs.
fn strip_escapes(line: &[u8]) -> String {
    let (mut out, mut i) = (Vec::with_capacity(line.len()), 0);
    while i < line.len() {
        match (line[i], line.get(i + 1)) {
            (0x1b, Some(b'[')) => {
                i += 2;
                while i < line.len() && !(0x40..=0x7e).contains(&line[i]) {
                    i += 1;
                }
            }
            (0x1b, Some(b']')) => {
                while i < line.len() && line[i] != 0x07 && !(line[i] == b'\\' && line[i - 1] == 0x1b) {
                    i += 1;
                }
            }
            (0x1b, _) => i += 1,
            (b, _) if b < 0x20 && b != b'\t' || b == 0x7f => {}
            (b, _) => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).trim_end().to_string()
}

/// Runs a command and returns its standard output. Standard error stays visible.
pub fn output(mut cmd: impl BorrowMut<Command>) -> Result<String> {
    let cmd = cmd.borrow_mut();
    let out = cmd.stderr(Stdio::inherit()).output().with_context(|| format!("run {}", describe(cmd)))?;
    if !out.status.success() {
        return Err(failed(cmd, out.status, &[]));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Returns standard output and ignores the exit status.
pub fn output_any(mut cmd: impl BorrowMut<Command>) -> Result<String> {
    let cmd = cmd.borrow_mut();
    let out = cmd.stderr(Stdio::null()).output().with_context(|| format!("run {}", describe(cmd)))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn cmd(program: &str, args: &[&str]) -> Command {
    let mut c = Command::new(program);
    c.args(args);
    c
}

