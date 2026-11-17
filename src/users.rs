use crate::generation::{Generation, STATE};
use crate::{Opts, cmd, report, run_cmd};
use anyhow::{Result, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{IsTerminal, Write};
use std::process::Stdio;
use std::sync::Mutex;

/// The accounts that pacnix created, one "user NAME" or "group NAME" a line.
/// Only these go when their declaration goes.
const CREATED: &str = "/var/lib/pacnix/accounts";

/// The passwords of the new accounts, from the question of the apply. They stay in memory.
static PASSWORDS: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

#[derive(Default)]
pub struct Plan {
    groupadd: Vec<(String, Vec<String>)>,
    useradd: Vec<(String, Vec<String>)>,
    usermod: Vec<(String, Vec<String>, Vec<String>)>,
    userdel: Vec<String>,
    groupdel: Vec<String>,
}

impl Plan {
    pub fn print(&self, l: &mut Vec<String>) {
        l.extend(self.groupadd.iter().map(|(g, _)| format!("create group {g}")));
        l.extend(self.useradd.iter().map(|(u, _)| format!("create user {u}")));
        l.extend(self.usermod.iter().map(|(u, _, what)| format!("change user {u}: {}", what.join(", "))));
        l.extend(self.userdel.iter().map(|u| format!("delete user {u} (the home directory stays)")));
        l.extend(self.groupdel.iter().map(|g| format!("delete group {g}")));
    }
}

/// The fields of a line of /etc/passwd or /etc/group, from getent.
fn entry(db: &str, name: &str) -> Option<Vec<String>> {
    let out = cmd("getent", &[db, name]).output().ok().filter(|o| o.status.success())?;
    Some(String::from_utf8_lossy(&out.stdout).trim().split(':').map(String::from).collect())
}

/// The supplementary groups of a user, from /etc/group.
fn supplementary(user: &str) -> BTreeSet<String> {
    let text = std::fs::read_to_string("/etc/group").unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            (f.len() == 4 && f[3].split(',').any(|m| m == user)).then(|| f[0].to_string())
        })
        .collect()
}

fn created() -> BTreeSet<String> {
    std::fs::read_to_string(CREATED).unwrap_or_default().lines().map(String::from).collect()
}

fn record(add: &[String], remove: &[String]) -> Result<()> {
    let mut set = created();
    set.extend(add.iter().cloned());
    set.retain(|l| !remove.contains(l));
    std::fs::create_dir_all(STATE)?;
    std::fs::write(CREATED, set.into_iter().map(|l| l + "\n").collect::<String>())?;
    Ok(())
}

pub fn plan(g: &Generation, old: &Generation) -> Result<Plan> {
    let mut plan = Plan::default();
    let (users, groups) = (&g.m.users.users, &g.m.users.groups);
    for (name, grp) in groups {
        if entry("group", name).is_none() {
            let mut args = Vec::new();
            if let Some(gid) = grp.gid {
                args.extend(["-g".to_string(), gid.to_string()]);
            }
            plan.groupadd.push((name.clone(), args));
        }
    }
    for (name, u) in users {
        let Some(f) = entry("passwd", name) else {
            let mut args = vec!["-m".to_string(), "-d".to_string(), u.home.clone()];
            let opt = |flag: &str, v: &Option<String>| v.iter().flat_map(|v| [flag.to_string(), v.clone()]).collect::<Vec<_>>();
            args.extend(opt("-u", &u.uid.map(|i| i.to_string())));
            args.extend(opt("-g", &u.group));
            args.extend(opt("-s", &u.shell));
            args.extend(opt("-c", &u.description));
            if !u.extra_groups.is_empty() {
                args.extend(["-G".to_string(), u.extra_groups.join(",")]);
            }
            plan.useradd.push((name.clone(), args));
            continue;
        };
        if let Some(uid) = u.uid.filter(|uid| f.get(2) != Some(&uid.to_string())) {
            bail!("users.users.{name}.uid is {uid}, and the existing account has the ID {}. pacnix does not change the ID of an account", f[2]);
        }
        let (mut args, mut what) = (Vec::new(), Vec::new());
        if let Some(shell) = u.shell.as_ref().filter(|s| f.get(6) != Some(s)) {
            args.extend(["-s".to_string(), shell.clone()]);
            what.push(format!("shell {shell}"));
        }
        if let Some(d) = u.description.as_ref().filter(|d| f.get(4) != Some(d)) {
            args.extend(["-c".to_string(), d.clone()]);
            what.push(format!("description {d}"));
        }
        if let Some(group) = u.group.as_ref().filter(|grp| entry("group", grp).and_then(|e| e.get(2).cloned()) != f.get(3).cloned()) {
            args.extend(["-g".to_string(), group.clone()]);
            what.push(format!("primary group {group}"));
        }
        let want: BTreeSet<String> = u.extra_groups.iter().cloned().collect();
        if supplementary(name) != want {
            args.extend(["-G".to_string(), u.extra_groups.join(",")]);
            what.push(format!("groups {}", if want.is_empty() { "none".to_string() } else { u.extra_groups.join(",") }));
        }
        if !args.is_empty() {
            plan.usermod.push((name.clone(), args, what));
        }
    }
    let mine = created();
    let gone = |kind: &str, name: &String, declared: bool| !declared && mine.contains(&format!("{kind} {name}"));
    plan.userdel = old.m.users.users.keys().filter(|n| gone("user", n, users.contains_key(*n)) && entry("passwd", n).is_some()).cloned().collect();
    plan.groupdel = old.m.users.groups.keys().filter(|n| gone("group", n, groups.contains_key(*n)) && entry("group", n).is_some()).cloned().collect();
    Ok(plan)
}

/// Asks for the password of each account that the apply creates, at the
/// question of the apply, before any change. Without a terminal, or with
/// --yes, the account has no password, and the summary says so.
pub fn ask_passwords(plan: &Plan, opts: &Opts) -> Result<()> {
    if opts.yes || !std::io::stdin().is_terminal() {
        return Ok(());
    }
    let mut passwords = PASSWORDS.lock().unwrap();
    for (name, _) in &plan.useradd {
        while !passwords.contains_key(name) {
            let first = rpassword::prompt_password(format!("Password of the new user {name}: "))?;
            let again = rpassword::prompt_password(format!("Password of the new user {name} again: "))?;
            if first == again && !first.is_empty() {
                passwords.insert(name.clone(), first);
            } else {
                println!("The passwords are empty or not equal. Type them again.");
            }
        }
    }
    Ok(())
}

pub fn apply(plan: &Plan) -> Result<()> {
    for (name, args) in &plan.groupadd {
        run_cmd(cmd("groupadd", &[]).args(args).arg(name))?;
        record(&[format!("group {name}")], &[])?;
    }
    for (name, args) in &plan.useradd {
        run_cmd(cmd("useradd", &[]).args(args).arg(name))?;
        record(&[format!("user {name}")], &[])?;
        match PASSWORDS.lock().unwrap().remove(name) {
            Some(password) => {
                let mut c = cmd("chpasswd", &[]).stdin(Stdio::piped()).spawn()?;
                writeln!(c.stdin.take().unwrap(), "{name}:{password}")?;
                if !c.wait()?.success() {
                    bail!("chpasswd failed for the user {name}");
                }
            }
            None => report::last(format!("pacnix: the new user {name} has no password. Set it with: passwd {name}")),
        }
    }
    for (name, args, _) in &plan.usermod {
        run_cmd(cmd("usermod", &[]).args(args).arg(name))?;
    }
    for name in &plan.userdel {
        run_cmd(cmd("userdel", &[name]))?;
        record(&[], &[format!("user {name}")])?;
    }
    for name in &plan.groupdel {
        run_cmd(cmd("groupdel", &[name]))?;
        record(&[], &[format!("group {name}")])?;
    }
    Ok(())
}
