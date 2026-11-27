use crate::generation::{Generation, Lines};
use crate::{cmd, output, run_cmd};
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
    /// The plan lines of the home files that the activation creates, changes and deletes.
    lines: [Lines; 3],
    /// The number of created or changed files of each user that an empty home configuration also has.
    support: std::collections::BTreeMap<String, usize>,
}

impl Plan {
    /// The plan lines: activations, and created, changed, deleted and restored home files.
    pub fn print(&self, l: [&mut Lines; 5]) {
        let [activate, created, changed, deleted, restored] = l;
        activate.extend(self.activate.iter().map(|(u, _)| (u.clone(), self.step(u, "Activate"))));
        created.extend(self.lines[0].iter().cloned());
        changed.extend(self.lines[1].iter().cloned());
        deleted.extend(self.lines[2].iter().cloned());
        restored.extend(self.originals.iter().map(|p| (p.clone(), format!("Restoring the original of home file {p}"))));
    }

    /// The activation step, with the number of Home Manager support files that it creates or changes.
    fn step(&self, user: &str, verb: &str) -> String {
        let step = format!("{verb} the home configuration of {user}");
        match self.support.get(user) {
            Some(n) => format!("{step}, with {}", crate::count(*n, "Home Manager support file")),
            None => step,
        }
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

/// The step of the activation of a home configuration.
fn step(user: &str) -> String {
    format!("activate the home configuration of {user}")
}

fn out_path(drv: &str) -> Result<String> {
    Ok(output(cmd("nix-store", &["--query", "--outputs", drv]))?.trim().to_string())
}

pub fn plan(g: &Generation) -> Result<Plan> {
    let mut plan = Plan::default();
    for (user, h) in &g.m.home {
        let out = out_path(&h.drv).with_context(|| format!("home configuration of {user}"))?;
        let old = current(&h.directory);
        if old.as_deref() == Some(out.as_str()) && !crate::report::retry(&step(user)) {
            continue;
        }
        let _stage = crate::report::stage(&format!("Building the home configuration of {user}"));
        run_cmd(cmd("nix-store", &["--realise", &h.drv, &h.path]))?;
        let new = home_files(&out);
        let home = Path::new(&h.directory);
        for rel in &new {
            let path = home.join(rel);
            let file = Path::new(&out).join("home-files").join(rel);
            let p = path.display().to_string();
            let linked = std::fs::read_link(&path).is_ok_and(|t| t.starts_with("/nix/store") && t.to_string_lossy().contains("-home-manager-files/"));
            let kept = backup(&path).symlink_metadata().is_ok();
            let support = h.support.iter().any(|s| Path::new(s) == rel);
            let changed = std::fs::canonicalize(&path).ok() != std::fs::canonicalize(&file).ok();
            if support && (linked || path.symlink_metadata().is_err()) {
                if changed {
                    *plan.support.entry(user.clone()).or_default() += 1;
                }
            } else if path.symlink_metadata().is_err() {
                plan.lines[0].push((p.clone(), format!("Creating home file {p}")));
            } else if !linked {
                let why = if kept { "it differs from the declaration".to_string() } else { format!("pacnix keeps the original as {}", backup(Path::new(rel)).display()) };
                plan.lines[1].push((p.clone(), format!("Changing home file {p} ({why})")));
                plan.replace.push((p, kept));
            } else if changed {
                plan.lines[1].push((p.clone(), format!("Changing home file {p}")));
            }
        }
        for rel in old.map(|o| home_files(&o)).unwrap_or_default() {
            let path = home.join(&rel);
            if new.contains(&rel) {
                continue;
            }
            if backup(&path).symlink_metadata().is_ok() {
                plan.originals.push(path.display().to_string());
            } else if path.symlink_metadata().is_ok() {
                plan.lines[2].push((path.display().to_string(), format!("Deleting home file {} (the home configuration created it)", path.display())));
            }
        }
        plan.activate.push((user.clone(), out));
    }
    Ok(plan)
}

/// Keeps the original of each home file in the way, as FILE.pacnix-hm-backup,
/// runs the activation of each home configuration as the user, with the
/// user's systemd manager and session bus, and then gives each home file
/// without a declaration its original back. Root writes to the Nix store: the
/// profile of the home configuration, the profile of its packages and the GC
/// roots. The activation reads the store only, thus no nix-daemon is necessary.
pub fn apply(g: &Generation, plan: &Plan) -> Result<()> {
    for (path, kept) in &plan.replace {
        let path = Path::new(path);
        if *kept { std::fs::remove_file(path) } else { std::fs::rename(path, backup(path)) }.with_context(|| format!("replace {}", path.display()))?;
    }
    // An activation is an isolated step: a failure does not stop the other users and the apply.
    for (user, out) in &plan.activate {
        let h = &g.m.home[user];
        crate::report::progress(&plan.step(user, "Activating"));
        for l in plan.lines.iter().flatten().filter(|l| Path::new(&l.0).starts_with(&h.directory)) {
            say!("{}", crate::report::plan_line(l));
        }
        crate::isolated(&step(user), || activate(user, &h.directory, out, &out_path(&h.path)?))?;
    }
    if !plan.activate.is_empty() {
        crate::report::last("• Log in again to use the new home configuration.".to_string());
    }
    for path in &plan.originals {
        let path = Path::new(path);
        if path.symlink_metadata().is_err() {
            std::fs::rename(backup(path), path).with_context(|| format!("restore the original of {}", path.display()))?;
        }
    }
    Ok(())
}

/// The steps of the home-manager command, with root for the writes to the Nix store.
fn activate(user: &str, home: &str, out: &str, packages: &str) -> Result<()> {
    let (profiles, roots) = (format!("{home}/.local/state/nix/profiles"), format!("{home}/.local/state/home-manager/gcroots"));
    run_cmd(cmd("runuser", &["-u", user, "--", "mkdir", "-p", &profiles, &roots]))?;
    run_cmd(cmd("nix-store", &["--realise", out, "--add-root", &format!("{roots}/new-home")]))?;
    run_cmd(cmd("nix-env", &["--profile", &format!("{profiles}/home-manager"), "--set", out]))?;
    std::fs::create_dir_all("/etc/profiles/per-user")?;
    run_cmd(cmd("nix-store", &["--realise", packages, "--add-root", &format!("/etc/profiles/per-user/{user}")]))?;
    let owner = |p: &str| run_cmd(cmd("chown", &["-h", &format!("{user}:")]).arg(p));
    for dir in [&profiles, &roots] {
        for e in std::fs::read_dir(dir)?.flatten() {
            owner(&e.path().display().to_string())?;
        }
    }
    let uid = output(cmd("id", &["-u", user]))?.trim().to_string();
    run_cmd(cmd("systemctl", &["start", &format!("user@{uid}.service")]))?;
    let runtime = format!("/run/user/{uid}");
    let mut c = cmd("runuser", &["-u", user, "--", "env", "-i"]);
    c.args([
        format!("HOME={home}"),
        format!("USER={user}"),
        format!("LOGNAME={user}"),
        "PATH=/usr/local/bin:/usr/bin".to_string(),
        format!("XDG_RUNTIME_DIR={runtime}"),
        "NIX_REMOTE=local?read-only=true".to_string(),
        "NIX_CONFIG=extra-experimental-features = read-only-local-store".to_string(),
    ]);
    if Path::new(&runtime).join("bus").exists() {
        c.arg(format!("DBUS_SESSION_BUS_ADDRESS=unix:path={runtime}/bus"));
    }
    c.args([&format!("{out}/activate"), "--driver-version", "1"]).current_dir(home);
    run_cmd(&mut c)?;
    let current = format!("{roots}/current-home");
    run_cmd(cmd("nix-store", &["--realise", out, "--add-root", &current]))?;
    owner(&current)?;
    let _ = std::fs::remove_file(format!("{roots}/new-home"));
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
