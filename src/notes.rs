use crate::generation::Generation;
use crate::report;
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CStr, CString};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, Stdio};

const CHECK_LIMIT: u32 = 5;
const ACTION_LIMIT: u32 = 120;
/// Runs the command of its positional parameters and writes only the exit
/// status of the command, so that a failure of setpriv or timeout cannot look
/// like a result of the check. The configuration is not shell text.
const WRAPPER: &str = r#""$@" </dev/null >/dev/null 2>&1; echo "$?""#;

/// Runs each eligible setup action once, then checks its result. An incomplete
/// or failed check keeps the dependent steps pending until a later apply.
pub fn apply(target: &Generation) -> Result<Vec<String>> {
    let mut done = BTreeSet::new();
    let mut unchecked = BTreeMap::new();
    let mut show = Vec::new();
    let mut ran = Vec::new();
    for note in &target.m.install_steps {
        report::check()?;
        let name = &note.name;
        let ready = note.after.iter().all(|p| done.contains(p));
        let Some(spec) = &note.check else {
            if let Some(message) = &note.message {
                unchecked.insert(name.clone(), (message.clone(), ready));
            }
            continue;
        };
        if !ready {
            continue;
        }
        let check = || {
            report::step(&format!("checking setup step {name}"));
            let status = run(&spec.user, &spec.command, "check", CHECK_LIMIT);
            report::check()?;
            Ok::<_, anyhow::Error>(status)
        };
        let status = check()?;
        match (status, &note.action) {
            (Ok(true), _) => {
                done.insert(name);
            }
            (Ok(false), Some(action)) => {
                let label = format!("Setup action {name}");
                report::progress(&label);
                let result = run(&spec.user, action, "action", ACTION_LIMIT);
                report::check()?;
                let status = match result {
                    Ok(_) => {
                        ran.push(name.clone());
                        check()?
                    }
                    Err(why) => Err(why),
                };
                match status {
                    Ok(true) => {
                        done.insert(name);
                    }
                    Ok(false) => {}
                    Err(why) => {
                        report::failure(&label, &format!("{why}."));
                    }
                }
            }
            (Ok(false), None) => {
                if let Some(message) = &note.message {
                    show.push((name.clone(), message.clone()));
                }
            }
            (Err(why), Some(_)) => {
                report::failure(&format!("Setup check {name}"), &format!("{why}."));
            }
            (Err(why), None) => show.push((name.clone(), format!("the check gave no result: {why}"))),
        }
    }
    report::check()?;
    report::install_notes(unchecked, show);
    Ok(ran)
}

/// The name of a group, or its number when it has no name.
fn group(gid: libc::gid_t) -> String {
    let g = unsafe { libc::getgrgid(gid) };
    match g.is_null() {
        true => gid.to_string(),
        false => unsafe { CStr::from_ptr((*g).gr_name) }.to_string_lossy().into_owned(),
    }
}

/// Runs a setup command as its user, with a clean environment and a time limit.
fn run(user: &str, command: &[String], kind: &str, limit: u32) -> Result<bool, String> {
    let missing = || format!("the user {user} does not exist");
    let name = CString::new(user).map_err(|_| missing())?;
    let pw = unsafe { libc::getpwnam(name.as_ptr()) };
    if pw.is_null() {
        return Err(missing());
    }
    let (uid, gid, home) = unsafe { ((*pw).pw_uid, (*pw).pw_gid, CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned()) };
    if uid == 0 {
        return Err(format!("the user {user} has the UID 0, and a {kind} does not run as root"));
    }
    let program = command.first().filter(|p| Path::new(p).is_absolute()).ok_or("give a program with an absolute path")?;
    if !Path::new(program).metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0) {
        return Err(format!("{program} is not an executable file"));
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
    cmd.args(["--kill-after=1".to_string(), limit.to_string(), "setpriv".to_string(), format!("--reuid={user}"), format!("--regid={}", group(gid))]);
    cmd.args([format!("--groups={}", groups.join(",")), "--no-new-privs".into(), "--".into(), "/usr/bin/sh".into(), "-c".into(), WRAPPER.into(), "sh".into()]);
    cmd.args(command);
    cmd.env_clear().envs([("HOME", home.as_str()), ("USER", user), ("LOGNAME", user), ("PATH", "/usr/local/bin:/usr/bin")]);
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
        (Some(0), Some(0)) => Ok(true),
        (Some(1), Some(0)) if kind == "check" => Ok(false),
        (Some(n @ (126 | 127)), Some(0)) => Err(format!("the command did not start (exit status {n})")),
        (Some(n), Some(0)) => Err(format!("the {kind} ended with exit status {n}")),
        (_, Some(124 | 137)) => Err(format!("the {kind} did not end in {limit} s")),
        (_, Some(n)) => Err(format!("the {kind} did not start as {user} (exit status {n})")),
        (_, None) => Err(format!("the {kind} stopped with signal {}", out.status.signal().unwrap_or_default())),
    }
}
