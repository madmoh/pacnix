use crate::generation::Generation;
use crate::{cmd, output, run_cmd};
use anyhow::{Context, Result};
use std::path::Path;

#[derive(Default)]
pub struct Plan {
    activate: Vec<(String, String)>,
    deferred: Vec<String>,
}

impl Plan {
    pub fn print(&self, l: &mut Vec<String>) {
        l.extend(self.activate.iter().map(|(u, p)| format!("activate home configuration of {u}: {p}")));
        l.extend(self.deferred.iter().map(|u| format!("note: the first boot activates the home configuration of {u}")));
    }
}

/// The current Home Manager generation of a home directory.
pub fn current(home: &str) -> Option<String> {
    let p = std::fs::canonicalize(format!("{home}/.local/state/home-manager/gcroots/current-home")).ok()?;
    Some(p.display().to_string())
}

fn out_path(drv: &str) -> Result<String> {
    Ok(output(cmd("nix-store", &["--query", "--outputs", drv]))?.trim().to_string())
}

pub fn plan(g: &Generation) -> Result<Plan> {
    let mut plan = Plan::default();
    if !crate::packages::booted() {
        plan.deferred = g.m.home.keys().cloned().collect();
        return Ok(plan);
    }
    for (user, h) in &g.m.home {
        let out = out_path(&h.drv).with_context(|| format!("home configuration of {user}"))?;
        if current(&h.directory).as_deref() != Some(out.as_str()) {
            plan.activate.push((user.clone(), out));
        }
    }
    Ok(plan)
}

/// Builds each home configuration and runs its activation as the user, with
/// the user's systemd manager and session bus.
pub fn apply(g: &Generation, plan: &Plan) -> Result<()> {
    for (user, _) in &plan.activate {
        let h = &g.m.home[user];
        let out = output(cmd("nix-store", &["--realise", &h.drv]))?.trim().to_string();
        let uid = output(cmd("id", &["-u", user]))?.trim().to_string();
        run_cmd(cmd("systemctl", &["start", &format!("user@{uid}.service")]))?;
        let runtime = format!("/run/user/{uid}");
        let mut c = cmd("runuser", &["-u", user, "--", "env", "-i"]);
        c.args([
            format!("HOME={}", h.directory),
            format!("USER={user}"),
            format!("LOGNAME={user}"),
            "PATH=/usr/local/bin:/usr/bin".to_string(),
            format!("XDG_RUNTIME_DIR={runtime}"),
        ]);
        if Path::new(&runtime).join("bus").exists() {
            c.arg(format!("DBUS_SESSION_BUS_ADDRESS=unix:path={runtime}/bus"));
        }
        c.arg(format!("{out}/activate")).current_dir(&h.directory);
        run_cmd(&mut c).with_context(|| format!("activate the home configuration of {user}"))?;
    }
    Ok(())
}
