//! The handoff to the new pacnix after a system upgrade that replaced the
//! running one: the new executable continues the same apply with the same
//! arguments, the lock and the secrets of the run. The state goes
//! in a sealed anonymous file whose descriptor the new process gets; only the
//! number of the descriptor is in the environment.

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::CString;
use std::io::{Read, Seek, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering::Relaxed};

const EXE: &str = "/usr/bin/pacnix";
const VAR: &str = "PACNIX_HANDOFF";
/// The version of the state. A new pacnix reads the versions that it knows.
const VERSION: u32 = 1;
const SEALS: i32 = libc::F_SEAL_SEAL | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE;
const STEP: &str = "continuing after the system upgrade";
const INVALID: &str = "the earlier pacnix gave a state that this pacnix cannot read. The system upgrade is done. Run pacnix apply again";

/// The device and inode of /usr/bin/pacnix at the start, when it is the running executable.
static INSTALLED: OnceLock<Option<(u64, u64)>> = OnceLock::new();
static RESUMED: AtomicBool = AtomicBool::new(false);
/// The descriptor of the lock of the run.
pub static LOCK: AtomicI32 = AtomicI32::new(-1);
/// The lock that the earlier pacnix gave.
pub static GIVEN: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);
/// Set when this run read the Arch Linux news.
pub static NEWS: AtomicBool = AtomicBool::new(false);

#[derive(Serialize, Deserialize)]
struct State {
    version: u32,
    lock: RawFd,
    attempt: usize,
    last_attempt: bool,
    serial: bool,
    closed: bool,
    passwords: BTreeMap<String, String>,
    secrets: BTreeMap<String, (String, String)>,
    recovered: Vec<String>,
    started: i64,
    log_offset: u64,
    reboot: Vec<String>,
    last: Vec<String>,
    news: bool,
}

#[derive(Deserialize)]
struct Version {
    version: u32,
}

/// True in the process that continues an apply after a handoff.
pub fn resumed() -> bool {
    RESUMED.load(Relaxed)
}

/// Tells an earlier pacnix that this executable can continue its apply.
pub fn probe() {
    if std::env::var_os(VAR).is_some_and(|v| v == "probe") {
        println!("{VERSION}");
        std::process::exit(0);
    }
}

/// Records whether the running executable is /usr/bin/pacnix.
pub fn record() {
    let id = |m: std::fs::Metadata| (m.dev(), m.ino());
    let running = std::fs::metadata("/proc/self/exe").map(id).ok();
    let installed = std::fs::metadata(EXE).map(id).ok();
    let _ = INSTALLED.set(running.filter(|r| Some(*r) == installed));
}

/// After the transaction of the system upgrade: when it replaced the running
/// /usr/bin/pacnix, the new executable continues the apply. Does not return then.
pub fn check() -> Result<()> {
    let Some(Some(running)) = INSTALLED.get() else { return Ok(()) };
    if resumed() {
        return Ok(());
    }
    let m = std::fs::metadata(EXE).with_context(|| format!("{EXE} is not there after the system upgrade. Install the package pacnix again, then run pacnix apply"))?;
    if (m.dev(), m.ino()) == *running {
        return Ok(());
    }
    crate::report::check()?;
    let probe = crate::output(crate::cmd(EXE, &["--version"]).env(VAR, "probe"));
    if probe.context("the new pacnix does not run. The system upgrade is done. Run pacnix apply again")?.trim() != VERSION.to_string() {
        bail!("the system upgrade installed a pacnix that cannot continue this apply, thus the configuration is not applied. Run pacnix apply again: the next apply uses the installed pacnix");
    }
    say!("• pacnix was upgraded. The new version continues the apply.");
    exec()
}

fn cloexec(fd: RawFd, on: bool) -> std::io::Result<()> {
    match unsafe { libc::fcntl(fd, libc::F_SETFD, if on { libc::FD_CLOEXEC } else { 0 }) } {
        0 => Ok(()),
        _ => Err(std::io::Error::last_os_error()),
    }
}

/// Takes a descriptor that the earlier pacnix gave: an open descriptor above
/// standard error, closed at the next exec.
fn take(fd: RawFd) -> Result<std::fs::File> {
    if fd <= 2 || cloexec(fd, true).is_err() {
        bail!(INVALID);
    }
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

fn sigint(how: libc::c_int) {
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::pthread_sigmask(how, &set, std::ptr::null_mut());
    }
}

fn exec() -> Result<()> {
    let (log_offset, reboot) = crate::report::RUN.lock().unwrap().as_ref().map(|r| (r.2, r.3.clone())).unwrap_or_default();
    let state = State {
        version: VERSION,
        lock: LOCK.load(Relaxed),
        attempt: crate::ATTEMPT.load(Relaxed),
        last_attempt: crate::FINAL.load(Relaxed),
        serial: crate::packages::SERIAL.load(Relaxed),
        closed: crate::closed(),
        passwords: crate::users::PASSWORDS.lock().unwrap().clone(),
        secrets: crate::secure::SECRETS.lock().unwrap().clone(),
        recovered: crate::secure::RECOVERED.lock().unwrap().clone(),
        started: crate::report::STARTED.load(Relaxed),
        log_offset,
        reboot,
        last: crate::report::LAST.lock().unwrap().clone(),
        news: NEWS.load(Relaxed),
    };
    let fd = unsafe { libc::memfd_create(c"pacnix-handoff".as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error()).context("make the state of the handoff");
    }
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    file.write_all(&serde_json::to_vec(&state).map_err(|_| anyhow!("write the state of the handoff"))?)?;
    if unsafe { libc::fcntl(fd, libc::F_ADD_SEALS, SEALS) } != 0 {
        return Err(std::io::Error::last_os_error()).context("seal the state of the handoff");
    }
    let argv: Vec<CString> = std::env::args_os().map(|a| CString::new(a.as_bytes())).collect::<Result<_, _>>()?;
    let mut ptrs: Vec<*const libc::c_char> = argv.iter().map(|a| a.as_ptr()).collect();
    ptrs.push(std::ptr::null());
    // The new process gets an interrupt of this window after it installs its handler.
    sigint(libc::SIG_BLOCK);
    let result = crate::report::check().and_then(|_| {
        cloexec(fd, false)?;
        cloexec(state.lock, false)?;
        unsafe { std::env::set_var(VAR, fd.to_string()) };
        unsafe { libc::execv(c"/usr/bin/pacnix".as_ptr(), ptrs.as_ptr()) };
        Err(std::io::Error::last_os_error()).context(format!("run the new {EXE}. The system upgrade is done. Run pacnix apply again"))
    });
    let _ = cloexec(fd, true).and(cloexec(state.lock, true));
    unsafe { std::env::remove_var(VAR) };
    sigint(libc::SIG_UNBLOCK);
    result
}

/// In the process that continues an apply: takes the state of the earlier
/// pacnix, before any other step.
pub fn receive() -> Result<()> {
    let Some(value) = std::env::var_os(VAR) else { return Ok(()) };
    unsafe { std::env::remove_var(VAR) };
    // Only root continues an apply: the earlier pacnix runs as root.
    if unsafe { libc::geteuid() } != 0 {
        bail!(INVALID);
    }
    RESUMED.store(true, Relaxed);
    // A summary for a stop before the state is read: the step, without the earlier changes.
    *crate::report::RUN.lock().unwrap() = Some((STEP.to_string(), std::time::Instant::now(), u64::MAX, Vec::new()));
    crate::report::changing();
    let result = restore(value.to_str().and_then(|v| v.parse().ok()));
    sigint(libc::SIG_UNBLOCK);
    result
}

fn restore(fd: Option<RawFd>) -> Result<()> {
    let fd = fd.context(INVALID)?;
    let mut file = take(fd)?;
    // Only a sealed anonymous file has these seals.
    let size = file.metadata().map_err(|_| anyhow!(INVALID))?.len();
    if unsafe { libc::fcntl(fd, libc::F_GET_SEALS) } != SEALS || size > 1 << 20 {
        bail!(INVALID);
    }
    let mut bytes = Vec::new();
    file.rewind().and_then(|_| file.read_to_end(&mut bytes)).map_err(|_| anyhow!(INVALID))?;
    drop(file);
    let version = serde_json::from_slice::<Version>(&bytes).map_err(|_| anyhow!(INVALID))?.version;
    if version != VERSION {
        bail!("the earlier pacnix gave a state of version {version}, and this pacnix reads version {VERSION}. The system upgrade is done. Run pacnix apply again");
    }
    let s: State = serde_json::from_slice(&bytes).map_err(|_| anyhow!(INVALID))?;
    // The lock of the earlier process: the same file, and this process holds it.
    if s.lock == fd {
        bail!(INVALID);
    }
    let lock = take(s.lock)?;
    let same = match (lock.metadata(), std::fs::metadata(crate::LOCK_FILE)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    };
    if !same || unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!(INVALID);
    }
    *GIVEN.lock().unwrap() = Some(lock);
    crate::ATTEMPT.store(s.attempt, Relaxed);
    crate::FINAL.store(s.last_attempt, Relaxed);
    crate::packages::SERIAL.store(s.serial, Relaxed);
    if s.closed {
        crate::close();
    }
    *crate::users::PASSWORDS.lock().unwrap() = s.passwords;
    *crate::secure::SECRETS.lock().unwrap() = s.secrets;
    *crate::secure::RECOVERED.lock().unwrap() = s.recovered;
    crate::report::STARTED.store(s.started, Relaxed);
    *crate::report::RUN.lock().unwrap() = Some((STEP.to_string(), std::time::Instant::now(), s.log_offset, s.reboot));
    *crate::report::LAST.lock().unwrap() = s.last;
    NEWS.store(s.news, Relaxed);
    say!("• pacnix {} continues the apply.", env!("CARGO_PKG_VERSION"));
    Ok(())
}
