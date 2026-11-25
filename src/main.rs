/// Prints a line, see report::emit.
macro_rules! say {
    ($($t:tt)*) => { crate::report::emit(&format!($($t)*), false) };
}

/// As say!, to standard error.
macro_rules! esay {
    ($($t:tt)*) => { crate::report::emit(&format!($($t)*), true) };
}

mod audit;
mod files;
mod generation;
mod handoff;
mod home;
mod notes;
mod packages;
mod report;
mod secure;
mod trust;
mod units;
mod users;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use std::borrow::BorrowMut;
use std::io::{BufRead, IsTerminal, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// Make an Arch Linux system agree with its configuration in a Nix flake.
#[derive(Parser)]
#[command(version, after_help = "To start, see \"First configuration\" in /usr/share/doc/pacnix/README.md.")]
struct Cli {
    /// The flake [default: /etc/pacnix]
    #[arg(long, global = true, default_value = "/etc/pacnix", hide_default_value = true)]
    flake: String,
    /// The host in the flake [default: this host name]
    #[arg(long, global = true)]
    host: Option<String>,
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Subcommand)]
enum Verb {
    /// Show each change that apply makes. Change nothing
    Plan,
    /// Upgrade the system, then make it agree with the configuration
    Apply(ApplyOpts),
    /// Show what the configuration does not explain. Change nothing
    Audit,
    /// Remove pacnix and Nix. The declared files, units and packages stay
    Uninstall(Opts),
}

impl Verb {
    fn name(&self) -> &'static str {
        match self {
            Verb::Plan => "plan",
            Verb::Apply(_) => "apply",
            Verb::Audit => "audit",
            Verb::Uninstall(_) => "uninstall",
        }
    }
}

#[derive(Args, Clone, Default)]
pub struct Opts {
    /// Accept the items of the question
    #[arg(long, short)]
    pub yes: bool,
    /// Permit the removal of protected packages
    #[arg(long)]
    pub allow_protected_removal: bool,
}

#[derive(Args, Clone)]
struct ApplyOpts {
    #[command(flatten)]
    opts: Opts,
    /// Only upgrade the system, with the active configuration
    #[arg(long, conflicts_with = "config_only")]
    upgrade_only: bool,
    /// Only apply the configuration. Do not upgrade the system
    #[arg(long)]
    config_only: bool,
}

pub struct Ctx {
    pub flake: String,
    pub host: String,
    pub opts: Opts,
}

/// Each run ends with one line: the result, the changes and the exit status.
/// As with switch-to-configuration of NixOS, the exit status 4 tells that the
/// change is done but some steps failed.
fn main() {
    handoff::probe();
    let cli = Cli::parse();
    let verb = cli.verb.name();
    report::signals();
    let result = handoff::receive().and_then(|_| run(cli));
    if let Err(e) = &result {
        if !report::interrupted() {
            esay!("• Error: {e:#}");
        }
    }
    std::process::exit(report::close(verb, result.is_ok()));
}

fn run(cli: Cli) -> Result<()> {
    // Like paru, pacnix runs as the user and elevates one time, before any
    // change, so that a long build cannot outlast the sudo timestamp.
    if unsafe { libc::geteuid() } != 0 {
        let exe = std::env::current_exe()?;
        let mut sudo = Command::new("sudo");
        // sudo removes NO_COLOR from the environment.
        if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
            sudo.arg("--preserve-env=NO_COLOR");
        }
        let e = sudo.arg("--").arg(exe).args(std::env::args_os().skip(1)).exec();
        return Err(e).context("run pacnix with sudo");
    }
    // pacnix uses the Nix store directly as root, without the nix-daemon.
    unsafe { std::env::set_var("NIX_REMOTE", "local") };
    let name = std::fs::read_to_string("/etc/hostname").map(|h| h.trim().to_string());
    let host = match &cli.host {
        Some(h) => h.clone(),
        None => name.as_ref().map_err(|e| anyhow::anyhow!("read /etc/hostname, or give --host: {e}"))?.clone(),
    };
    let flake = match std::fs::canonicalize(&cli.flake) {
        Ok(p) => p.display().to_string(),
        Err(_) => cli.flake.clone(),
    };
    // pacnix uses systemd to make changes: it does not run in a chroot, for example arch-chroot.
    if !packages::booted() {
        bail!("pacnix runs only on the booted system. Boot the installed system, then run pacnix again.");
    }
    if !handoff::resumed() {
        handoff::record();
    }
    let _lock = lock()?;
    // A local flake needs flake.nix. Nix tells this without the directory and the option.
    if !flake.contains(':') && !std::path::Path::new(&flake).join("flake.nix").exists() {
        bail!("{flake} has no flake.nix. Make the configuration as \"First configuration\" of the README tells, or give --flake with the directory of the configuration.");
    }
    // The host and the flake show only when they are not the defaults.
    if name.ok().as_ref() != Some(&host) || cli.flake != "/etc/pacnix" {
        say!("• Host {host} of the flake {flake}.");
    }
    let ctx = |opts| Ctx { flake: flake.clone(), host: host.clone(), opts };
    match cli.verb {
        Verb::Plan => generation::plan(&ctx(Opts::default())),
        Verb::Apply(a) => resilient(|| generation::apply(&ctx(a.opts.clone()), !a.config_only, !a.upgrade_only)),
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

/// The number of the attempt that runs, from 0.
pub static ATTEMPT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Set for the last attempt of a verb: then `isolated` also continues after a
/// failure in the network.
pub static FINAL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The retry policy of pacnix. A failure in the network repeats the verb up to
/// three times in about two minutes, with one download at a time. The next
/// attempt continues from the work that is done: the packages in the pacman
/// cache and in the local repository. A failure of an isolated step does not
/// stop the verb, see `isolated`. Each other failure, for example an
/// evaluation error, a refused plan or a pacman transaction that fails, stops
/// pacnix one time with the reason.
fn resilient(f: impl Fn() -> Result<()>) -> Result<()> {
    if !handoff::resumed() {
        report::begin();
    }
    for (attempt, pause) in PAUSES.iter().enumerate().skip(ATTEMPT.load(std::sync::atomic::Ordering::Relaxed)) {
        ATTEMPT.store(attempt, std::sync::atomic::Ordering::Relaxed);
        report::reset();
        match f() {
            Ok(()) => return Ok(report::summary()),
            Err(e) if report::interrupted() => return Err(e),
            Err(e) => match cause(&e) {
                Cause::Network(line) => {
                    report::progress(&format!("Attempt {} of {} failed in the network: {line}. Next attempt in {pause} s, with one download at a time.", attempt + 1, PAUSES.len() + 1));
                    packages::SERIAL.store(true, std::sync::atomic::Ordering::Relaxed);
                    std::thread::sleep(Duration::from_secs(*pause));
                }
                Cause::Stale => return Err(e.context("the mirror does not have a package of the sync databases, thus the databases or the mirror are old. Run pacnix apply without --config-only, or use a different mirror")),
                Cause::Other => return Err(e),
            },
        }
    }
    ATTEMPT.store(PAUSES.len(), std::sync::atomic::Ordering::Relaxed);
    report::reset();
    FINAL.store(true, std::sync::atomic::Ordering::Relaxed);
    f().map(|_| report::summary())
}

/// Runs a step that changes one element and whose result no later step reads,
/// apart from the steps that the caller skips: a package build, a trigger, a
/// unit action or a home activation. As with nix build --keep-going and
/// switch-to-configuration of NixOS, a failure of the step does not stop the
/// verb: the summary shows it, the exit status is 4, and the next apply does
/// the step again. A failure in the network stops the attempt, so that the
/// retry policy applies, until the last attempt. Returns true when the step
/// succeeds.
pub fn isolated(step: &str, f: impl FnOnce() -> Result<()>) -> Result<bool> {
    let e = match f() {
        Ok(()) => return Ok(report::passed(step)),
        Err(e) => e,
    };
    match cause(&e) {
        _ if report::interrupted() => Err(e),
        // The next attempt can find the step done, for example a written file without its trigger.
        Cause::Stale => Err(report::remember(step, e)),
        Cause::Network(_) if !FINAL.load(std::sync::atomic::Ordering::Relaxed) => Err(report::remember(step, e)),
        _ => Ok(report::failure(step, &format!("{e:#}"))),
    }
}

/// Set when the questions of the run end, before the long work. Later steps do not ask.
static CLOSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static DELETE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The items that the user accepted in this run.
pub static APPROVED: std::sync::Mutex<std::collections::BTreeSet<String>> = std::sync::Mutex::new(std::collections::BTreeSet::new());

pub fn closed() -> bool {
    CLOSED.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn close() {
    CLOSED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The one question of a run: it shows each item that the user must accept and
/// asks one time. --yes accepts. Without a terminal and without --yes, pacnix
/// stops. The answer k continues and keeps the data of `data`. Returns true
/// when the user accepts the deletion of the data. A later call returns the
/// same answer for the accepted items. After the questions of the run end, a
/// new item stops pacnix, because only the next apply can ask for it.
pub fn ask(items: &[String], data: &[String], opts: &Opts) -> Result<bool> {
    use std::sync::atomic::Ordering::Relaxed;
    let new: Vec<&String> = items.iter().chain(data).filter(|i| !APPROVED.lock().unwrap().contains(*i)).collect();
    if new.is_empty() {
        return Ok(DELETE.load(Relaxed));
    }
    let data_new = data.iter().any(|d| new.contains(&d));
    say!("• These items need a confirmation:");
    for i in &new {
        say!("  {i}");
    }
    if closed() && !opts.yes {
        bail!("pacnix asks only at the start of the run, and these items came after it. Run pacnix apply again to confirm them.");
    }
    if !opts.yes && !std::io::stdin().is_terminal() {
        bail!("these items need a confirmation. Give --yes to continue without a terminal.");
    }
    let mut answer = String::from("y");
    if !opts.yes {
        print!("Continue? [y/N]{} ", if !data_new { " (N stops pacnix)" } else { " (N stops pacnix, k continues and keeps the data)" });
        std::io::stdout().flush()?;
        answer.clear();
        std::io::stdin().lock().read_line(&mut answer)?;
    }
    match answer.trim() {
        "y" | "Y" | "yes" => DELETE.store(DELETE.load(Relaxed) || data_new, Relaxed),
        "k" if data_new => {}
        _ => bail!("stopped by the user"),
    }
    APPROVED.lock().unwrap().extend(new.into_iter().cloned());
    Ok(DELETE.load(Relaxed))
}

const LOCK_FILE: &str = "/run/lock/pacnix.lock";

/// The lock of the run. After a handoff, the lock that the earlier pacnix took.
fn lock() -> Result<std::fs::File> {
    if let Some(f) = handoff::GIVEN.lock().unwrap().take() {
        return Ok(f);
    }
    let f = std::fs::File::create(LOCK_FILE)?;
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!("a different pacnix process is running.");
    }
    handoff::LOCK.store(f.as_raw_fd(), std::sync::atomic::Ordering::Relaxed);
    Ok(f)
}

/// The command line, without the runuser, sandbox and env prefix of the builder commands.
pub fn describe(cmd: &Command) -> String {
    let words: Vec<String> = std::iter::once(cmd.get_program()).chain(cmd.get_args()).map(|w| w.to_string_lossy().into_owned()).collect();
    let mut start = 0;
    // The sandbox has two separators: the one of unshare and the one after the writable paths.
    let separators = if words[0] == "unshare" { 2 } else { 1 };
    if words[0] == "runuser" || words[0] == "unshare" {
        start = words.iter().enumerate().filter(|(_, w)| *w == "--").nth(separators - 1).map_or(0, |(i, _)| i + 1);
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

/// An error with its message and the output of the command that failed, for the retry policy.
pub fn failed_with(message: String, lines: &[String]) -> anyhow::Error {
    Failed { message, lines: lines.to_vec() }.into()
}

/// The error of a failed command, with the last lines of its output.
pub fn failed(cmd: &Command, status: ExitStatus, lines: &[String]) -> anyhow::Error {
    let tail: Vec<&str> = lines.iter().rev().take(10).rev().map(|l| l.as_str()).collect();
    let message = match tail.is_empty() {
        true => format!("{} failed with {status}.", describe(cmd)),
        false => format!("{} failed with {status}. Its last output:\n  | {}", describe(cmd), tail.join("\n  | ")),
    };
    Failed { message, lines: lines.to_vec() }.into()
}

/// Runs a command and fails on a nonzero exit status. Its output shows only
/// in the error, with the last lines.
pub fn run_cmd(mut cmd: impl BorrowMut<Command>) -> Result<()> {
    let cmd = cmd.borrow_mut();
    let (status, lines) = forward(cmd, 20, true, |_| false)?;
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
/// line for 60 s, pacnix writes one, with the last redraw of a progress meter,
/// for example of a large download. Without `stdout`, the standard output of
/// the command is dropped. Lines that `show` refuses are not shown, but they
/// are kept for the error and the retry policy. Returns the exit status and
/// the last `keep` lines.
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
        if text.is_empty() {
            return false;
        }
        let shown = show(&text);
        if shown {
            report::emit(&format!("  | {text}"), false);
        }
        if lines.len() == keep {
            lines.pop_front();
        }
        lines.push_back(text);
        shown
    };
    let (start, mut quiet, mut buf, mut spinner) = (Instant::now(), Instant::now(), [0u8; 8192], false);
    loop {
        if !spinner && start.elapsed() >= Duration::from_secs(1) {
            spinner = report::spinner();
        }
        let mut fd = libc::pollfd { fd: pipe.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let ready = unsafe { libc::poll(&mut fd, 1, 1000) } > 0;
        // A daemon that the command started can keep the pipe open.
        if !ready && child.try_wait()?.is_some() {
            break;
        }
        if report::meter() || report::live() {
            quiet = Instant::now();
        } else if quiet.elapsed() >= Duration::from_secs(60) {
            report::heartbeat(&describe(cmd), start.elapsed().as_secs() / 60, &strip_escapes(&line));
            quiet = Instant::now();
        }
        if !ready {
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
    if spinner {
        report::end_spinner();
    }
    let status = child.wait()?;
    report::check()?;
    Ok((status, lines.into()))
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

/// Runs a command and returns its standard output. Standard error shows only in the error.
pub fn output(mut cmd: impl BorrowMut<Command>) -> Result<String> {
    let cmd = cmd.borrow_mut();
    let out = cmd.output().with_context(|| format!("run {}", describe(cmd)))?;
    report::check()?;
    if !out.status.success() {
        let lines: Vec<String> = String::from_utf8_lossy(&out.stderr).lines().map(String::from).collect();
        return Err(failed(cmd, out.status, &lines));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Returns standard output and ignores the exit status.
pub fn output_any(mut cmd: impl BorrowMut<Command>) -> Result<String> {
    let cmd = cmd.borrow_mut();
    let out = cmd.stderr(Stdio::null()).output().with_context(|| format!("run {}", describe(cmd)))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The count and the noun, for example "1 package" and "3 packages".
pub fn count(n: usize, noun: &str) -> String {
    match n {
        1 => format!("1 {noun}"),
        _ => format!("{n} {noun}s"),
    }
}

/// The items with their noun and verb, for example "file created: /etc/motd".
/// More than three items show as their number, for example "4 files created".
pub fn named(noun: &str, verb: &str, items: &[String]) -> String {
    let text = match items.len() {
        1 => format!("{noun} {verb}: {}", report::item(&items[0])),
        2 | 3 => format!("{noun}s {verb}: {}", report::items(items)),
        n => format!("{} {verb}", count(n, noun)),
    };
    text.replace("  ", " ").replace(" :", ":").trim_end().to_string()
}

/// A command without input: the questions of pacnix are the only input of the run.
pub fn cmd(program: &str, args: &[&str]) -> Command {
    let mut c = Command::new(program);
    c.args(args).stdin(Stdio::null());
    c
}

