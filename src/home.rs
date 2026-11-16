use crate::generation::Generation;
use crate::{cmd, output, run_cmd, run_quiet};
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
        l.extend(self.deferred.iter().map(|u| format!("note: the first pacnix upgrade on the booted system activates the home configuration of {u}")));
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
        crate::report::progress(&format!("activate the home configuration of {user}"));
        let h = &g.m.home[user];
        run_quiet(cmd("nix-store", &["--realise", &h.drv]))?;
        let out = out_path(&h.drv)?;
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

/// Replaces each link of the active home configurations into the Nix store
/// with a copy of its content that the user owns and can write, so that the
/// files stay when the Nix store goes.
pub fn detach(g: &Generation) -> Result<()> {
    for (user, h) in &g.m.home {
        let Some(current) = current(&h.directory) else { continue };
        let files = format!("{current}/home-files");
        for e in walkdir::WalkDir::new(&files).min_depth(1).into_iter().flatten() {
            let path = Path::new(&h.directory).join(e.path().strip_prefix(&files)?);
            let linked = path.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink());
            if !linked || !std::fs::canonicalize(&path).is_ok_and(|t| t.starts_with("/nix/store")) {
                continue;
            }
            let copy = crate::files::temporary(&path.display().to_string());
            run_cmd(cmd("runuser", &["-u", user, "--", "cp", "-RL"]).arg(&path).arg(&copy))?;
            run_cmd(cmd("runuser", &["-u", user, "--", "chmod", "-R", "u+w"]).arg(&copy))?;
            std::fs::remove_file(&path)?;
            std::fs::rename(&copy, &path)?;
        }
    }
    Ok(())
}
