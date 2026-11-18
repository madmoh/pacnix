use crate::generation::{FileSpec, Generation, Trigger};
use crate::{Ctx, cmd, output_any, report, run_cmd, run_hidden};
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
    adopt: Vec<String>,
    originals: Vec<String>,
    triggers: BTreeMap<String, Trigger>,
    stale: std::collections::BTreeSet<std::path::PathBuf>,
}

impl Plan {
    pub fn deleted(&self) -> Vec<String> {
        self.delete.iter().chain(&self.originals).cloned().chain(self.restore.iter().map(|r| r.0.clone())).collect()
    }

    /// The plan lines: created, changed, deleted and restored files.
    pub fn print(&self, l: [&mut crate::generation::Lines; 4]) {
        let [created, changed, deleted, restored] = l;
        for (p, why) in &self.write {
            match why.strip_prefix("create") {
                Some(value) => created.push((p.clone(), format!("create file {p}{value}"))),
                None => changed.push((p.clone(), format!("change file {p}{why}"))),
            }
        }
        deleted.extend(self.delete.iter().map(|p| (p.clone(), format!("delete file {p} (pacnix created it, no original to restore)"))));
        deleted.extend(self.pacnew.iter().map(|p| (format!("{p}.pacnew"), format!("delete file {p}.pacnew (the declared content replaces it)"))));
        restored.extend(self.originals.iter().map(|p| (p.clone(), format!("restore file {p} (the original from before pacnix)"))));
        restored.extend(self.restore.iter().map(|(p, pkg)| (p.clone(), format!("restore file {p} (the file of package {pkg})"))));
    }
}

/// The new value of a file for its plan line: the target of a link, or the
/// lines of a text file that leave and that come, six at most, compared with
/// the file at the path. Without `full`, only the number of changed lines.
pub fn value(path: &Path, new: &Path, link: bool, full: bool) -> String {
    if link {
        return format!("\n  link to {}", new.display());
    }
    let Ok(text) = std::fs::read_to_string(new) else { return String::new() };
    let old = std::fs::read_to_string(path).unwrap_or_default();
    let (before, after): (BTreeSet<&str>, BTreeSet<&str>) = (old.lines().collect(), text.lines().collect());
    let mut lines: Vec<String> = old.lines().filter(|l| !after.contains(l)).map(|l| format!("- {l}")).collect();
    lines.extend(text.lines().filter(|l| !before.contains(l)).map(|l| format!("+ {l}")));
    if !full {
        lines = if lines.is_empty() { vec![] } else { vec![format!("{} differ", crate::count(lines.len(), "line"))] };
    }
    let more = lines.len().saturating_sub(6);
    lines.truncate(6);
    if more > 0 {
        lines.push(format!("and {more} more changed lines"));
    }
    let end = if !text.is_empty() && !text.ends_with('\n') { " (no newline at the end; end the text with \\n)" } else { "" };
    format!("{end}{}", lines.iter().map(|l| format!("\n  {l}")).collect::<String>())
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

/// The declared mode. None: the file system gives the mode and the owner, for
/// example vfat through its mount options.
fn mode(spec: &FileSpec) -> Option<u32> {
    spec.mode.as_ref().map(|m| u32::from_str_radix(m, 8).unwrap_or(0o644))
}

/// The copy of a file that a declared file replaced. Without the declaration,
/// the copy goes back.
const ORIGINALS: &str = "/var/lib/pacnix/originals";

fn original(path: &str) -> std::path::PathBuf {
    Path::new(ORIGINALS).join(path.trim_start_matches('/'))
}

/// True if the file on disk has the declared content, mode and owner.
pub fn matches(path: &str, spec: &FileSpec) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return false };
    if let Some(target) = &spec.target {
        return std::fs::read_link(path).is_ok_and(|t| t == Path::new(target));
    }
    let owned = mode(spec).is_none_or(|m| meta.mode() & 0o7777 == m && Some(meta.uid()) == id(&spec.user, true) && Some(meta.gid()) == id(&spec.group, false));
    meta.file_type().is_file() && owned && sha256(Path::new(path)) == sha256(Path::new(&spec.source))
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

/// Refuses files below a declared mount point that does not have the declared
/// file system, for example an EFI system partition that is not mounted yet.
fn check_mounts(g: &Generation) -> Result<()> {
    for (point, fs) in &g.m.mounts {
        if fs == "auto" || !g.m.files.keys().any(|p| Path::new(p).starts_with(point)) {
            continue;
        }
        // Reading the directory mounts an automount.
        let _ = std::fs::read_dir(point);
        let mounts = std::fs::read_to_string("/proc/self/mounts")?;
        let found = mounts.lines().map(|l| l.split(' ').collect::<Vec<_>>()).filter(|f| f.len() > 2 && f[1] == point).map(|f| f[2].to_string()).last();
        match found {
            Some(t) if &t == fs => {}
            Some(t) => bail!("{point} has the file system {t}, and the configuration declares {fs}. Mount the {fs} file system at {point}, then run pacnix again"),
            None => bail!("{point} is not a mount point, and pacnix writes files below it only on the declared {fs} file system. Mount it, then run pacnix again. A change of /etc/fstab mounts it only at the next boot"),
        }
    }
    Ok(())
}

pub fn plan(_ctx: &Ctx, g: &Generation, ledger: &[Generation]) -> Result<Plan> {
    check_mounts(g)?;
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
    // The files of the active generation, the kept originals and the files that a
    // pending generation wrote are pacnix's. A pending generation can stop before
    // it writes its files, for example at a build failure.
    let ours = |p: &String| {
        previous.is_some_and(|f| f.contains_key(p))
            || original(p).symlink_metadata().is_ok()
            || ledger.iter().skip(1).any(|l| l.m.files.get(p).is_some_and(|spec| matches(p, spec)))
    };
    for (path, spec) in &g.m.files {
        if Path::new(&format!("{path}.pacnew")).exists() {
            plan.pacnew.push(path.clone());
        }
        // A file that pacnix did not write and that agrees is no change: the
        // apply keeps its original, for the removal of the declaration.
        if matches(path, spec) {
            if !ours(path) {
                plan.adopt.push(path.clone());
            }
            continue;
        }
        let value = value(Path::new(path), spec.target.as_deref().map(Path::new).unwrap_or(Path::new(&spec.source)), spec.target.is_some(), true);
        // The mode and the owner show when they change.
        let meta = std::fs::symlink_metadata(path).ok();
        let same = |m: u32| meta.as_ref().is_some_and(|x| x.mode() & 0o7777 == m && Some(x.uid()) == id(&spec.user, true) && Some(x.gid()) == id(&spec.group, false));
        let perm = mode(spec).filter(|m| spec.target.is_none() && !same(*m)).map(|m| format!(" ({m:04o} {}:{})", spec.user, spec.group)).unwrap_or_default();
        let value = format!("{perm}{value}");
        let why = if std::fs::symlink_metadata(path).is_err() {
            format!("create{value}")
        } else if ours(path) {
            let drift = if previous.and_then(|f| f.get(path)) == Some(spec) { " (it differs from the declaration)" } else { "" };
            format!("{drift}{value}")
        } else {
            plan.adopt.push(path.clone());
            let of = owner.get(path).map(|pkg| format!(" of package {pkg}")).unwrap_or_default();
            format!("\n  keep the original{of} as\n    {}{value}", original(path).display())
        };
        plan.write.push((path.clone(), why));
    }
    for path in dropped {
        if original(path).symlink_metadata().is_ok() {
            plan.originals.push(path.clone());
            continue;
        }
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

pub fn check(_ctx: &Ctx, g: &Generation, plan: &Plan) -> Result<()> {
    let mut probed = BTreeSet::new();
    for (path, _) in &plan.write {
        let spec = &g.m.files[path];
        if probed.insert((Path::new(path).parent(), &spec.mode, &spec.user, &spec.group)) {
            probe(path, spec)?;
        }
    }
    Ok(())
}

/// Writes a test file in the directory of a file to write, with the declared
/// mode and owner, and fails if the file system does not keep them. Else each
/// apply corrects the same drift again, for example on a vfat file system,
/// where the mount options set the mode and owner of all files. An owner that
/// does not exist yet is not tested.
fn probe(path: &str, spec: &FileSpec) -> Result<()> {
    let Some(dir) = Path::new(path).ancestors().skip(1).find(|d| d.is_dir()) else { return Ok(()) };
    let Some(want) = mode(spec).filter(|_| spec.target.is_none()) else { return Ok(()) };
    let test = dir.join(".pacnix-probe");
    let f = std::fs::File::create(&test).with_context(|| format!("write {}", test.display()))?;
    let (uid, gid) = (id(&spec.user, true), id(&spec.group, false));
    let set = std::os::unix::fs::fchown(&f, uid, gid).and_then(|_| f.set_permissions(std::fs::Permissions::from_mode(want)));
    let meta = f.metadata();
    std::fs::remove_file(&test)?;
    let meta = meta?;
    if set.is_ok() && meta.mode() & 0o7777 == want && uid.is_none_or(|u| u == meta.uid()) && gid.is_none_or(|g| g == meta.gid()) {
        return Ok(());
    }
    let (fs, target, options) = mount(dir);
    bail!(
        "the {fs} file system on {target} cannot keep the mode {want:04o} and the owner {}:{} of {path}: a new file gets the mode {:04o} and the owner {}:{} (mount options {options}). Change the mount options in /etc/fstab, declare the mode and owner that the file system gives, or declare the mode null",
        spec.user,
        spec.group,
        meta.mode() & 0o7777,
        meta.uid(),
        meta.gid()
    )
}

/// The file system type, mount point and options of the mount that holds a path.
fn mount(path: &Path) -> (String, String, String) {
    let info = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    let mut best = (String::new(), String::new(), String::new());
    for line in info.lines() {
        let f: Vec<&str> = line.split(' ').collect();
        let Some(sep) = f.iter().position(|x| *x == "-") else { continue };
        if f.len() > sep + 3 && path.starts_with(f[4]) && f[4].len() >= best.1.len() {
            best = (f[sep + 1].to_string(), f[4].to_string(), format!("{},{}", f[5], f[sep + 3]));
        }
    }
    best
}

fn fsync_dir(dir: &Path) -> Result<()> {
    std::fs::File::open(dir)?.sync_all()?;
    Ok(())
}

/// The temporary file of a declared file, in the same directory. apply writes
/// all of them, syncs the disk one time and then renames each into place: a
/// power loss leaves each file with its old or its new content.
pub fn temporary(path: &str) -> std::path::PathBuf {
    let target = Path::new(path);
    target.with_file_name(format!(".{}.pacnix-new", target.file_name().unwrap().to_string_lossy()))
}

fn write_temporary(path: &str, spec: &FileSpec) -> Result<()> {
    let tmp = temporary(path);
    std::fs::create_dir_all(tmp.parent().context("file has no directory")?)?;
    if let Some(link) = &spec.target {
        std::os::unix::fs::symlink(link, &tmp)?;
        return Ok(());
    }
    let data = std::fs::read(&spec.source)?;
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(&data)?;
    if let Some(m) = mode(spec) {
        let uid = id(&spec.user, true).with_context(|| format!("user {} does not exist", spec.user))?;
        let gid = id(&spec.group, false).with_context(|| format!("group {} does not exist", spec.group))?;
        std::os::unix::fs::fchown(&f, Some(uid), Some(gid))?;
        f.set_permissions(std::fs::Permissions::from_mode(m))?;
    }
    Ok(())
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
            run_hidden(cmd("pacman", &["-Sw", "--noconfirm", "--config", crate::packages::CONF, pkg]))?;
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
pub fn prune_dirs(path: &str) {
    let mut dir = Path::new(path).parent();
    while let Some(d) = dir {
        let s = d.display().to_string();
        let empty = std::fs::read_dir(d).is_ok_and(|mut e| e.next().is_none());
        if s.matches('/').count() < 2 || !empty || !owners(&[&s]).is_empty() || std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

/// Returns the paths that changed.
pub fn apply(_ctx: &Ctx, g: &Generation, plan: &Plan) -> Result<Vec<String>> {
    let mut changed = Vec::new();
    let changing = changes(plan);
    // The step shows the lines of the plan.
    let mut lines: [crate::generation::Lines; 4] = Default::default();
    let [a, b, c, d] = &mut lines;
    plan.print([a, b, c, d]);
    let lines = lines.concat();
    if !lines.is_empty() {
        report::progress(&format!("change {}", crate::count(lines.len(), "file")));
    }
    lines.iter().for_each(|l| say!("  {}", l.1.replace('\n', "\n  ")));
    // The triggers and unit restarts that follow the changes are recorded
    // first: after an interrupt, no change of a file shows them again.
    report::pending(triggered(plan, &changing).into_iter().map(|(n, _)| format!("trigger {n}")));
    report::pending(crate::units::changed_units(&changing).into_iter().map(|u| format!("systemctl try-restart {u}")));
    for t in &plan.stale {
        std::fs::remove_file(t)?;
    }
    // The original of an unmanaged file is copied before the declared file replaces it.
    for path in &plan.adopt {
        let keep = original(path);
        std::fs::create_dir_all(keep.parent().unwrap())?;
        run_cmd(cmd("cp", &["-a", "-T", path]).arg(&keep)).with_context(|| format!("keep the original of {path}"))?;
    }
    for (path, _) in &plan.write {
        write_temporary(path, &g.m.files[path]).with_context(|| format!("write {path}"))?;
    }
    if !plan.write.is_empty() {
        unsafe { libc::sync() };
    }
    for (path, _) in &plan.write {
        std::fs::rename(temporary(path), path).with_context(|| format!("write {path}"))?;
        changed.push(path.clone());
    }
    for path in &plan.pacnew {
        std::fs::remove_file(format!("{path}.pacnew"))?;
    }
    for (path, pkg) in &plan.restore {
        restore(path, pkg).with_context(|| format!("restore {path}"))?;
        changed.push(path.clone());
    }
    for path in &plan.originals {
        let keep = original(path);
        run_cmd(cmd("cp", &["-a", "-T"]).arg(&keep).arg(temporary(path))).with_context(|| format!("restore the original of {path}"))?;
        std::fs::rename(temporary(path), path)?;
        std::fs::remove_file(&keep)?;
        prune_dirs(&keep.display().to_string());
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
        .filter(|(n, t)| {
            changed.iter().any(|c| t.paths.iter().any(|p| c == p || (p.ends_with('/') && c.starts_with(p.as_str()))))
                || crate::report::retry(&format!("trigger {n}"))
        })
        .map(|(n, t)| (n, &t.command))
        .collect()
}

/// The paths that the apply changes.
fn changes(plan: &Plan) -> Vec<String> {
    plan.write.iter().map(|w| w.0.clone()).chain(plan.restore.iter().map(|r| r.0.clone())).chain(plan.originals.iter().cloned()).chain(plan.delete.iter().cloned()).collect()
}

pub fn print_triggers(plan: &Plan, triggers: &mut crate::generation::Lines, units: &mut crate::generation::Lines) {
    let changed = changes(plan);
    triggers.extend(triggered(plan, &changed).into_iter().map(|(n, c)| (n.clone(), format!("run trigger {n}: {c}"))));
    units.extend(crate::units::changed_units(&changed).into_iter().map(|u| (u.clone(), format!("restart unit {u} if it runs"))));
    let retries = crate::report::retries().into_iter().filter_map(|s| s.strip_prefix("systemctl try-restart ").map(String::from));
    units.extend(retries.map(|u| (u.clone(), format!("restart unit {u} if it runs (the last apply did not complete it)"))));
}

/// Runs the triggers of the changed files and the triggers that failed at an
/// earlier apply, in the order of their names. A trigger can need the result
/// of an earlier one, for example the boot loader needs the boot images: after
/// a failed trigger, the later triggers wait for the next apply.
pub fn run_triggers(plan: &Plan, changed: &[String]) -> Result<()> {
    let mut failed: Option<&String> = None;
    for (name, command) in triggered(plan, changed) {
        let step = format!("trigger {name}");
        if let Some(f) = failed {
            crate::report::failure(&step, &format!("it runs after trigger {f}, which failed"));
            continue;
        }
        crate::report::progress(&format!("run trigger {name}: {command}"));
        if !crate::isolated(&step, || run_hidden(cmd("sh", &["-ec", command])))? {
            failed = Some(name);
        }
    }
    Ok(())
}
