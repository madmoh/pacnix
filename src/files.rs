use crate::generation::{FileSpec, Generation, Trigger};
use crate::{Ctx, cmd, output_any, run_cmd};
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CString;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

#[derive(Default)]
pub struct Plan {
    write: Vec<(String, String)>,
    restore: Vec<(String, String)>,
    delete: Vec<String>,
    pacnew: Vec<String>,
    conflicts: Vec<String>,
    triggers: BTreeMap<String, Trigger>,
    stale: std::collections::BTreeSet<std::path::PathBuf>,
}

impl Plan {
    pub fn deleted(&self) -> Vec<String> {
        self.delete.iter().cloned().chain(self.restore.iter().map(|r| r.0.clone())).collect()
    }

    pub fn print(&self, l: &mut Vec<String>) {
        l.extend(self.write.iter().map(|(p, why)| format!("write file {p} ({why})")));
        l.extend(self.restore.iter().map(|(p, pkg)| format!("restore file {p} from package {pkg}")));
        l.extend(self.delete.iter().map(|p| format!("delete file {p}")));
        l.extend(self.pacnew.iter().map(|p| format!("delete {p}.pacnew (the declared content replaces it)")));
        l.extend(self.conflicts.iter().map(|p| format!("refuse to replace unmanaged file {p}")));
    }
}

pub fn sha256(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok().map(|b| Sha256::digest(&b).to_vec())
}

fn id(name: &str, user: bool) -> Option<u32> {
    let c = CString::new(name).ok()?;
    unsafe {
        if user {
            let p = libc::getpwnam(c.as_ptr());
            (!p.is_null()).then(|| (*p).pw_uid)
        } else {
            let g = libc::getgrnam(c.as_ptr());
            (!g.is_null()).then(|| (*g).gr_gid)
        }
    }
}

fn mode(spec: &FileSpec) -> u32 {
    u32::from_str_radix(&spec.mode, 8).unwrap_or(0o644)
}

/// True if the file on disk has the declared content, mode and owner.
pub fn matches(path: &str, spec: &FileSpec) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return false };
    if let Some(target) = &spec.target {
        return std::fs::read_link(path).is_ok_and(|t| t == Path::new(target));
    }
    meta.file_type().is_file()
        && meta.mode() & 0o7777 == mode(spec)
        && Some(meta.uid()) == id(&spec.user, true)
        && Some(meta.gid()) == id(&spec.group, false)
        && sha256(Path::new(path)) == sha256(Path::new(&spec.source))
}

/// Maps paths to the packages that own them.
pub fn owners(paths: &[&str]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for chunk in paths.chunks(500) {
        let out = output_any(cmd("pacman", &["-Qo"]).args(chunk).env("LC_ALL", "C")).unwrap_or_default();
        for line in out.lines() {
            if let Some((path, rest)) = line.split_once(" is owned by ") {
                map.insert(path.to_string(), rest.split_whitespace().next().unwrap_or("").to_string());
            }
        }
    }
    map
}

pub fn plan(ctx: &Ctx, g: &Generation, ledger: &[Generation]) -> Result<Plan> {
    let mut plan = Plan::default();
    plan.stale = g.m.files.keys().chain(ledger.iter().flat_map(|l| l.m.files.keys())).map(|p| temporary(p)).filter(|t| t.symlink_metadata().is_ok()).collect();
    // Triggers of the previous generation also run, so that a dropped feature
    // regenerates what its files fed (for example the initramfs).
    for l in ledger {
        plan.triggers.extend(l.m.triggers.clone());
    }
    plan.triggers.extend(g.m.triggers.clone());
    let managed: BTreeSet<&String> = ledger.iter().flat_map(|l| l.m.files.keys()).collect();
    let previous = ledger.first().map(|l| &l.m.files);
    let mut ask: Vec<&str> = g.m.files.keys().filter(|p| !managed.contains(p) && Path::new(p).exists()).map(String::as_str).collect();
    let dropped: Vec<&String> = managed.iter().copied().filter(|p| !g.m.files.contains_key(*p)).collect();
    ask.extend(dropped.iter().map(|s| s.as_str()));
    let owner = owners(&ask);
    for (path, spec) in &g.m.files {
        if Path::new(&format!("{path}.pacnew")).exists() {
            plan.pacnew.push(path.clone());
        }
        if matches(path, spec) {
            continue;
        }
        let why = if std::fs::symlink_metadata(path).is_err() {
            "create".to_string()
        } else if managed.contains(path) {
            if previous.and_then(|f| f.get(path)) == Some(spec) { "correct drift" } else { "update" }.to_string()
        } else if let Some(pkg) = owner.get(path) {
            format!("override the file of package {pkg}")
        } else if ctx.opts.overwrite_unmanaged {
            "replace an unmanaged file".to_string()
        } else {
            plan.conflicts.push(path.clone());
            continue;
        };
        plan.write.push((path.clone(), why));
    }
    for path in dropped {
        if std::fs::symlink_metadata(path).is_err() {
            continue;
        }
        match owner.get(path) {
            Some(pkg) => plan.restore.push((path.clone(), pkg.clone())),
            None => plan.delete.push(path.clone()),
        }
    }
    Ok(plan)
}

pub fn check(_ctx: &Ctx, plan: &Plan) -> Result<()> {
    if !plan.conflicts.is_empty() {
        bail!(
            "these files exist, and no package or generation owns them: {}. Give --overwrite-unmanaged to replace them",
            plan.conflicts.join(", ")
        );
    }
    Ok(())
}

fn fsync_dir(dir: &Path) -> Result<()> {
    std::fs::File::open(dir)?.sync_all()?;
    Ok(())
}

/// Writes a file with atomic replace: a temporary file in the same directory,
/// fsync, rename, fsync of the directory.
fn temporary(path: &str) -> std::path::PathBuf {
    let target = Path::new(path);
    target.with_file_name(format!(".{}.pacnix-new", target.file_name().unwrap().to_string_lossy()))
}

fn write_atomic(path: &str, spec: &FileSpec) -> Result<()> {
    let target = Path::new(path);
    let dir = target.parent().context("file has no directory")?;
    std::fs::create_dir_all(dir)?;
    let tmp = temporary(path);
    if let Some(link) = &spec.target {
        std::os::unix::fs::symlink(link, &tmp)?;
        std::fs::rename(&tmp, target)?;
        return fsync_dir(dir);
    }
    let data = std::fs::read(&spec.source)?;
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&data)?;
        let uid = id(&spec.user, true).with_context(|| format!("user {} does not exist", spec.user))?;
        let gid = id(&spec.group, false).with_context(|| format!("group {} does not exist", spec.group))?;
        std::os::unix::fs::fchown(&f, Some(uid), Some(gid))?;
        f.set_permissions(std::fs::Permissions::from_mode(mode(spec)))?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, target)?;
    fsync_dir(dir)
}

/// Replaces a file with the content from the package archive in the pacman cache.
fn restore(path: &str, pkg: &str) -> Result<()> {
    let version = output_any(cmd("pacman", &["-Q", pkg]))?.split_whitespace().nth(1).unwrap_or("").to_string();
    let find = || -> Option<String> {
        std::fs::read_dir("/var/cache/pacman/pkg").ok()?.flatten().map(|e| e.path().display().to_string()).find(|p| {
            let name = Path::new(p).file_name().unwrap().to_string_lossy().into_owned();
            name.starts_with(&format!("{pkg}-{version}-")) && name.rsplit_once("-").is_some_and(|(head, _)| head == format!("{pkg}-{version}")) && !name.ends_with(".sig")
        })
    };
    let archive = match find() {
        Some(a) => a,
        None => {
            run_cmd(cmd("pacman", &["-Sw", "--noconfirm", "--config", "/var/lib/pacnix/pacman.conf", pkg]))?;
            find().with_context(|| format!("no archive of {pkg} {version} in the pacman cache"))?
        }
    };
    let target = Path::new(path);
    let dir = target.parent().unwrap();
    let tmp = dir.join(".pacnix-restore");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let member = path.trim_start_matches('/');
    run_cmd(cmd("bsdtar", &["-xpf", &archive, "-C"]).arg(&tmp).arg(member))?;
    std::fs::rename(tmp.join(member), target)?;
    std::fs::remove_dir_all(&tmp)?;
    fsync_dir(dir)
}

/// Removes empty directories above a deleted file that no package owns.
fn prune_dirs(path: &str) {
    let mut dir = Path::new(path).parent();
    while let Some(d) = dir {
        let s = d.display().to_string();
        if s.matches('/').count() < 2 || !owners(&[&s]).is_empty() || std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

/// Returns the paths that changed.
pub fn apply(_ctx: &Ctx, g: &Generation, plan: &Plan) -> Result<Vec<String>> {
    let mut changed = Vec::new();
    for t in &plan.stale {
        std::fs::remove_file(t)?;
    }
    for (path, _) in &plan.write {
        match write_atomic(path, &g.m.files[path]) {
            // arch-chroot bind-mounts /etc/resolv.conf from the live system.
            Err(e) if !crate::packages::booted() && e.root_cause().downcast_ref::<std::io::Error>().and_then(|e| e.raw_os_error()) == Some(libc::EBUSY) => {
                let _ = std::fs::remove_file(temporary(path));
                println!("pacnix: note: {path} is a mount point in this chroot; the first boot writes it");
            }
            r => {
                r.with_context(|| format!("write {path}"))?;
                changed.push(path.clone());
            }
        }
    }
    for path in &plan.pacnew {
        std::fs::remove_file(format!("{path}.pacnew"))?;
    }
    for (path, pkg) in &plan.restore {
        restore(path, pkg).with_context(|| format!("restore {path}"))?;
        changed.push(path.clone());
    }
    for path in &plan.delete {
        std::fs::remove_file(path).with_context(|| format!("delete {path}"))?;
        prune_dirs(path);
        changed.push(path.clone());
    }
    Ok(changed)
}

fn triggered<'a>(plan: &'a Plan, changed: &[String]) -> Vec<(&'a String, &'a String)> {
    plan.triggers
        .iter()
        .filter(|(_, t)| {
            changed.iter().any(|c| t.paths.iter().any(|p| c == p || (p.ends_with('/') && c.starts_with(p.as_str()))))
        })
        .map(|(n, t)| (n, &t.command))
        .collect()
}

pub fn print_triggers(plan: &Plan, l: &mut Vec<String>) {
    let changed: Vec<String> = plan
        .write
        .iter()
        .map(|w| w.0.clone())
        .chain(plan.restore.iter().map(|r| r.0.clone()))
        .chain(plan.delete.iter().cloned())
        .collect();
    l.extend(triggered(plan, &changed).into_iter().map(|(n, c)| format!("run trigger {n}: {c}")));
    l.extend(crate::units::changed_units(&changed).into_iter().map(|u| format!("restart unit {u} if it runs")));
}

pub fn run_triggers(plan: &Plan, changed: &[String]) -> Result<()> {
    for (name, command) in triggered(plan, changed) {
        println!("pacnix: run trigger {name}: {command}");
        run_cmd(cmd("sh", &["-ec", command])).with_context(|| format!("trigger {name}"))?;
    }
    Ok(())
}
