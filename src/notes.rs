use crate::generation::NoteCheck;
use std::ffi::{CStr, CString};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, Stdio};

/// The time in seconds that a check can run.
const LIMIT: u32 = 5;
/// Runs the command of its positional parameters and writes only the exit
/// status of the command, so that a failure of setpriv or timeout cannot look
/// like a result of the check. The configuration is not shell text.
const WRAPPER: &str = r#""$@" </dev/null >/dev/null 2>&1; echo "$?""#;

/// The name of a group, or its number when it has no name.
fn group(gid: libc::gid_t) -> String {
    let g = unsafe { libc::getgrgid(gid) };
    match g.is_null() {
        true => gid.to_string(),
        false => unsafe { CStr::from_ptr((*g).gr_name) }.to_string_lossy().into_owned(),
    }
}

/// Runs the check of an install note as its user, with setpriv: no input, no
/// output, a clean environment and a time limit. Returns true when the step is
/// not done (exit status 1), false when it is done (0), and else the reason why
/// the check gives no result.
pub fn check(c: &NoteCheck) -> Result<bool, String> {
    let missing = || format!("the user {} does not exist", c.user);
    let name = CString::new(c.user.as_str()).map_err(|_| missing())?;
    let pw = unsafe { libc::getpwnam(name.as_ptr()) };
    if pw.is_null() {
        return Err(missing());
    }
    let (uid, gid, home) = unsafe { ((*pw).pw_uid, (*pw).pw_gid, CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned()) };
    if uid == 0 {
        return Err(format!("the user {} has the UID 0, and a check does not run as root", c.user));
    }
    if !Path::new(&c.command[0]).metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0) {
        return Err(format!("{} is not an executable file", c.command[0]));
    }
    // The groups of this account, also when a different account has the same UID.
    // setpriv reads a name before a number, thus it gets the names.
    let (mut n, mut gids) = (64, vec![0; 64]);
    while unsafe { libc::getgrouplist(name.as_ptr(), gid, gids.as_mut_ptr(), &mut n) } < 0 {
        gids.resize(n as usize, 0);
    }
    gids.truncate(n as usize);
    let groups: Vec<String> = gids.into_iter().map(group).collect();
    let mut cmd = Command::new("timeout");
    cmd.args(["--kill-after=1".to_string(), LIMIT.to_string(), "setpriv".to_string(), format!("--reuid={}", c.user), format!("--regid={}", group(gid))]);
    cmd.args([format!("--groups={}", groups.join(",")), "--no-new-privs".into(), "--".into(), "/usr/bin/sh".into(), "-c".into(), WRAPPER.into(), "sh".into()]);
    cmd.args(&c.command);
    cmd.env_clear().envs([("HOME", home.as_str()), ("USER", &c.user), ("LOGNAME", &c.user), ("PATH", "/usr/local/bin:/usr/bin")]);
    // The services of a session of the user, when the user has one.
    let runtime = format!("/run/user/{uid}");
    if Path::new(&runtime).is_dir() {
        cmd.env("XDG_RUNTIME_DIR", &runtime);
        if Path::new(&runtime).join("bus").exists() {
            cmd.env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={runtime}/bus"));
        }
    }
    cmd.current_dir(if Path::new(&home).is_dir() { home.as_str() } else { "/" });
    let out = cmd.stdin(Stdio::null()).stderr(Stdio::null()).output().map_err(|e| format!("timeout does not start: {e}"))?;
    match (String::from_utf8_lossy(&out.stdout).trim().parse::<i32>().ok(), out.status.code()) {
        (Some(0), _) => Ok(false),
        (Some(1), _) => Ok(true),
        (Some(n @ (126 | 127)), _) => Err(format!("the command did not start (exit status {n})")),
        (Some(n), _) => Err(format!("the check ended with exit status {n}")),
        (None, Some(124 | 137)) => Err(format!("the check did not end in {LIMIT} s")),
        (None, Some(n)) => Err(format!("the check did not start as {} (exit status {n})", c.user)),
        (None, None) => Err(format!("the check stopped with signal {}", out.status.signal().unwrap_or_default())),
    }
}
