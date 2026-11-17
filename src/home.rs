use crate::generation::Generation;
use crate::{cmd, output, run_cmd, run_quiet};
use anyhow::{Context, Result};
use std::path::Path;

/// The extension of the original of a home file that the home configuration replaces.
const BACKUP: &str = "pacnix-hm-backup";

#[derive(Default)]
pub struct Plan {
    activate: Vec<(String, String)>,
    /// Home files in the way of the home configuration: true when the original is kept already.
    replace: Vec<(String, bool)>,
    /// Home files without a declaration that get their original back.
    originals: Vec<String>,
}

impl Plan {
    pub fn print(&self, l: &mut Vec<String>) {
        l.extend(self.activate.iter().map(|(u, p)| format!("activate home configuration of {u}: {p}")));
        l.extend(self.replace.iter().map(|(p, kept)| match kept {
            false => format!("replace the unmanaged home file {p}, keep the original"),
            true => format!("replace the changed home file {p}"),
        }));
        l.extend(self.originals.iter().map(|p| format!("restore the original of home file {p}")));
    }
}

fn backup(path: &Path) -> std::path::PathBuf {
    format!("{}.{BACKUP}", path.display()).into()
}

/// The files of a Home Manager generation, relative to the home directory.
fn home_files(generation: &str) -> Vec<std::path::PathBuf> {
    let files = Path::new(generation).join("home-files");
    let entries = walkdir::WalkDir::new(&files).min_depth(1).into_iter().flatten().filter(|e| e.path_is_symlink());
    entries.filter_map(|e| e.path().strip_prefix(&files).ok().map(Path::to_path_buf)).collect()
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
    for (user, h) in &g.m.home {
        let out = out_path(&h.drv).with_context(|| format!("home configuration of {user}"))?;
        let old = current(&h.directory);
        if old.as_deref() == Some(out.as_str()) {
            continue;
        }
        run_quiet(cmd("nix-store", &["--realise", &h.drv]))?;
        let new = home_files(&out);
        let home = Path::new(&h.directory);
        for rel in &new {
            let path = home.join(rel);
            let linked = std::fs::read_link(&path).is_ok_and(|t| t.starts_with("/nix/store") && t.to_string_lossy().contains("-home-manager-files/"));
            if path.symlink_metadata().is_ok() && !linked {
                plan.replace.push((path.display().to_string(), backup(&path).symlink_metadata().is_ok()));
            }
        }
        for rel in old.map(|o| home_files(&o)).unwrap_or_default() {
            let path = home.join(&rel);
            if !new.contains(&rel) && backup(&path).symlink_metadata().is_ok() {
                plan.originals.push(path.display().to_string());
            }
        }
        plan.activate.push((user.clone(), out));
    }
    Ok(plan)
}

/// Keeps the original of each home file in the way, as FILE.pacnix-hm-backup,
/// runs the activation of each home configuration as the user, with the
/// user's systemd manager and session bus, and then gives each home file
/// without a declaration its original back.
pub fn apply(g: &Generation, plan: &Plan) -> Result<()> {
    for (path, kept) in &plan.replace {
        let path = Path::new(path);
        if *kept { std::fs::remove_file(path) } else { std::fs::rename(path, backup(path)) }.with_context(|| format!("replace {}", path.display()))?;
    }
    for (user, out) in &plan.activate {
        crate::report::progress(&format!("activate the home configuration of {user}"));
        let h = &g.m.home[user];
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
    for path in &plan.originals {
        let path = Path::new(path);
        if path.symlink_metadata().is_err() {
            std::fs::rename(backup(path), path).with_context(|| format!("restore the original of {}", path.display()))?;
        }
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
