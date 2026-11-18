use crate::generation::{Generation, build, history};
use crate::{Ctx, cmd, files, output, output_any};
use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::collections::BTreeSet;
use std::path::Path;

const SKIP: &[&str] = &[
    "/proc", "/sys", "/dev", "/run", "/tmp", "/var/tmp", "/home", "/root", "/mnt", "/media", "/lost+found",
    "/var/lib/pacman", "/var/cache/pacman/pkg",
];

fn globs(patterns: impl IntoIterator<Item = String>) -> GlobSet {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        if let Ok(g) = Glob::new(p.trim_end_matches('/')) {
            b.add(g);
        }
    }
    b.build().unwrap_or_else(|_| GlobSet::empty())
}

/// Finds the features of earlier generations whose claims match a path.
struct Origins(Vec<(u64, String, GlobSet, GlobSet)>);

impl Origins {
    fn new(current: &Generation) -> Result<Self> {
        let mut list = Vec::new();
        for (n, g) in history()?.into_iter().rev() {
            for (feature, c) in g.m.claims {
                if !current.m.claims.contains_key(&feature) {
                    list.push((n, feature, globs(c.system), globs(c.home)));
                }
            }
        }
        Ok(Origins(list))
    }

    fn of(&self, path: &str, home: bool) -> String {
        self.0
            .iter()
            .find(|(_, _, s, h)| if home { h.is_match(path) } else { s.is_match(path) })
            .map(|(n, f, _, _)| format!(" (claimed by feature {f} in generation {n}; the feature is no longer enabled)"))
            .unwrap_or_default()
    }
}

pub fn audit(ctx: &Ctx) -> Result<()> {
    let g = build(ctx)?;
    let _stage = crate::report::stage("check the system");
    let origins = Origins::new(&g)?;
    let mut report: Vec<String> = Vec::new();

    let mut owned: BTreeSet<String> = BTreeSet::new();
    for l in output(cmd("pacman", &["-Qlq"]))?.lines() {
        owned.insert(l.trim_end_matches('/').to_string());
    }
    let parents: BTreeSet<&str> = g.m.files.keys().flat_map(|p| Path::new(p).ancestors().skip(1).filter_map(|a| a.to_str())).collect();
    let claimed = globs(g.m.claims.values().flat_map(|c| c.system.clone()).chain(g.m.audit.ignore.clone()));
    let mut walk = walkdir::WalkDir::new("/").into_iter();
    while let Some(Ok(e)) = walk.next() {
        let path = e.path().display().to_string();
        if SKIP.contains(&path.as_str()) || claimed.is_match(&path) || g.m.files.contains_key(&path) {
            if e.file_type().is_dir() {
                walk.skip_current_dir();
            }
            continue;
        }
        if owned.contains(&path) || parents.contains(path.as_str()) || path == "/" {
            continue;
        }
        if path.ends_with(".pacnew") || path.ends_with(".pacsave") {
            report.push(format!("{} file: {path}", &path[path.rfind('.').unwrap() + 1..]));
            continue;
        }
        let kind = if e.file_type().is_dir() { "directory" } else { "file" };
        report.push(format!("unowned {kind}: {path}{}", origins.of(&path, false)));
        if e.file_type().is_dir() {
            walk.skip_current_dir();
        }
    }

    for l in output_any(cmd("pacman", &["-Qkk"]).env("LC_ALL", "C"))?.lines() {
        let Some(rest) = l.strip_prefix("warning: ") else { continue };
        let Some((pkg, rest)) = rest.split_once(": ") else { continue };
        let Some((path, why)) = rest.rsplit_once(" (") else { continue };
        let content = why.contains("checksum") || why.contains("Size");
        if content && !g.m.files.contains_key(path) && !claimed.is_match(path) {
            report.push(format!("modified package file: {path} ({pkg})"));
        }
    }

    for (path, spec) in &g.m.files {
        if !files::matches(path, spec) {
            report.push(format!("managed file differs from the generation: {path}"));
        }
    }

    let (declared, _) = crate::packages::declared(&g, &crate::packages::sync_names()?)?;
    for p in output(cmd("pacman", &["-Qqe"]))?.lines() {
        if !declared.contains(p) {
            report.push(format!("explicit package not declared: {p}"));
        }
    }

    let units = crate::units::plan(&g, &g, true)?;
    report.extend(units.disable.iter().map(|u| format!("enabled unit not declared and not enabled by a vendor preset: {u}")));
    report.extend(units.enable.iter().filter(|u| g.m.units.enable.contains(*u)).map(|u| format!("declared unit not enabled: {u}")));

    home(&g, &origins, &mut report)?;
    if let Some(state) = crate::secure::audit(&g, &mut report)? {
        say!("pacnix: {state}");
    }
    drop(_stage);
    print(&report);
    let mut notes = Vec::new();
    crate::report::audit(&mut notes);
    notes.iter().for_each(|n| say!("pacnix: {n}"));
    Ok(())
}

/// Shows the number of items first, then the items under one heading for
/// each kind, and at the end one line that tells what to do.
fn print(report: &[String]) {
    if report.is_empty() {
        say!("pacnix: the system agrees with the configuration.");
        return;
    }
    let mut kinds: Vec<(&str, Vec<&str>)> = Vec::new();
    for line in report {
        let (kind, item) = line.split_once(": ").unwrap_or((line, ""));
        match kinds.iter_mut().find(|k| k.0 == kind) {
            Some(k) => k.1.push(item),
            None => kinds.push((kind, vec![item])),
        }
    }
    match report.len() {
        1 => say!("pacnix: 1 item that is not declared or that differs from the configuration"),
        n => say!("pacnix: {n} items that are not declared or that differ from the configuration"),
    }
    for (kind, items) in &kinds {
        say!("  {kind} ({}):", items.len());
        items.iter().for_each(|i| say!("    {}", i.replace('\n', "\n    ")));
    }
    say!("pacnix: apply corrects the managed files, the declared units, the boot files and the TPM2 keys. It never removes a package that it did not install. Declare each other item, add it to claims or audit.ignore, or remove it.");
}

/// Lists entries of ~/.config, ~/.local/share, ~/.cache and dot entries of
/// each home directory that no claim and no Home Manager file explains.
fn home(g: &Generation, origins: &Origins, report: &mut Vec<String>) -> Result<()> {
    let claims = globs(g.m.claims.values().flat_map(|c| c.home.clone()).chain(["**/*.pacnix-hm-backup".to_string()]));
    for (dir, managed) in homes()? {
        let dir = dir.as_str();
        let mut entries = Vec::new();
        for sub in [".config", ".local/share", ".cache", ""] {
            let Ok(rd) = std::fs::read_dir(Path::new(dir).join(sub)) else { continue };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if sub.is_empty() && (!name.starts_with('.') || [".config", ".local", ".cache"].contains(&name.as_str())) {
                    continue;
                }
                entries.push(if sub.is_empty() { name } else { format!("{sub}/{name}") });
            }
        }
        entries.sort();
        for rel in entries {
            let explained = claims.is_match(&rel)
                || managed.iter().any(|m| m == &rel || m.starts_with(&format!("{rel}/")));
            if !explained {
                report.push(format!("unclaimed home path: {dir}/{rel}{}", origins.of(&rel, true)));
            }
        }
    }
    Ok(())
}

/// The home directories of the users, with the paths of the active Home
/// Manager files in each, relative to the home directory.
fn homes() -> Result<Vec<(String, BTreeSet<String>)>> {
    let mut list = Vec::new();
    for line in std::fs::read_to_string("/etc/passwd")?.lines() {
        let f: Vec<&str> = line.split(':').collect();
        let uid: u32 = f.get(2).and_then(|u| u.parse().ok()).unwrap_or(0);
        if !(1000..60000).contains(&uid) || f.len() < 6 || !Path::new(f[5]).is_dir() {
            continue;
        }
        let mut managed: BTreeSet<String> = BTreeSet::new();
        if let Some(current) = crate::home::current(f[5]) {
            let files = format!("{current}/home-files");
            for e in walkdir::WalkDir::new(&files).min_depth(1).into_iter().flatten() {
                managed.insert(e.path().strip_prefix(&files).unwrap().display().to_string());
            }
        }
        list.push((f[5].to_string(), managed));
    }
    Ok(list)
}

/// What the target generation keeps: the files of packages, the declared
/// files, the claimed paths and the Home Manager files.
struct Keep {
    owned: BTreeSet<String>,
    files: BTreeSet<String>,
    system: GlobSet,
    home: GlobSet,
    homes: Vec<(String, BTreeSet<String>)>,
}

impl Keep {
    fn new(target: &Generation) -> Result<Self> {
        Ok(Keep {
            owned: output(cmd("pacman", &["-Qlq"]))?.lines().map(|l| l.trim_end_matches('/').to_string()).collect(),
            files: target.m.files.keys().cloned().collect(),
            system: globs(target.m.claims.values().flat_map(|c| c.system.clone())),
            home: globs(target.m.claims.values().flat_map(|c| c.home.clone())),
            homes: homes()?,
        })
    }

    fn covers(&self, path: &str) -> bool {
        let p = Path::new(path);
        self.owned.contains(path)
            || self.files.contains(path)
            || p.ancestors().any(|a| self.system.is_match(a))
            || self.homes.iter().any(|(home, managed)| {
                p.strip_prefix(home).is_ok_and(|rel| managed.contains(&rel.display().to_string()) || rel.ancestors().any(|a| self.home.is_match(a)))
            })
    }
}

/// The existing paths that a claim pattern matches, without the paths below a match.
fn expand(pattern: &str) -> Vec<String> {
    let pattern = Path::new(pattern.trim_end_matches('/'));
    let literal: std::path::PathBuf = pattern.components().take_while(|c| !c.as_os_str().to_string_lossy().contains(['*', '?', '[', '{'])).collect();
    if literal == pattern {
        return pattern.symlink_metadata().map(|_| pattern.display().to_string()).into_iter().collect();
    }
    let set = globs([pattern.display().to_string()]);
    let (mut found, mut walk) = (Vec::new(), walkdir::WalkDir::new(literal).follow_root_links(false).into_iter());
    while let Some(entry) = walk.next() {
        let Ok(e) = entry else { continue };
        if set.is_match(e.path()) {
            found.push(e.path().display().to_string());
            if e.file_type().is_dir() {
                walk.skip_current_dir();
            }
        }
    }
    found
}

/// The existing paths of the features that `old` claims and `target` does
/// not, by feature. A path that the target keeps is not data of the feature.
pub fn leftovers(old: &Generation, target: &Generation) -> Result<Vec<(String, Vec<String>)>> {
    let removed: Vec<_> = old.m.claims.iter().filter(|(f, _)| !target.m.claims.contains_key(*f)).collect();
    if removed.is_empty() {
        return Ok(vec![]);
    }
    let keep = Keep::new(target)?;
    let mut list = Vec::new();
    for (feature, c) in removed {
        let mut paths: Vec<String> = c.system.iter().flat_map(|p| expand(p)).collect();
        for (home, _) in &keep.homes {
            paths.extend(c.home.iter().flat_map(|p| expand(&format!("{home}/{p}"))));
        }
        paths.retain(|p| !keep.covers(p));
        if !paths.is_empty() {
            list.push((feature.clone(), paths));
        }
    }
    Ok(list)
}

/// The items of the question for the data of removed features.
pub fn asks(data: &[(String, Vec<String>)]) -> Vec<String> {
    data.iter().map(|(f, paths)| format!("Delete the data of removed feature {f}: {}.", paths.join(", "))).collect()
}

/// Deletes the data of removed features. A path inside that the target keeps
/// stays, with its directories. Outside the home directories of the users, the
/// empty directories above a deleted path that no package owns go too.
pub fn delete(target: &Generation, data: &[(String, Vec<String>)]) -> Result<()> {
    let keep = Keep::new(target)?;
    for (feature, paths) in data {
        crate::report::progress(&format!("delete the data of removed feature {feature}"));
        for path in paths {
            for e in walkdir::WalkDir::new(path).follow_root_links(false).contents_first(true) {
                let e = e?;
                if keep.covers(&e.path().display().to_string()) {
                    continue;
                }
                let done = if e.file_type().is_dir() { std::fs::remove_dir(e.path()) } else { std::fs::remove_file(e.path()) };
                match done {
                    Err(err) if err.raw_os_error() == Some(libc::ENOTEMPTY) => {}
                    r => r.with_context(|| format!("delete {}", e.path().display()))?,
                }
            }
            if !keep.homes.iter().any(|(home, _)| Path::new(path).starts_with(home)) {
                files::prune_dirs(path);
            }
        }
    }
    Ok(())
}
