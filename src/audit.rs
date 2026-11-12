use crate::generation::{Generation, build, history};
use crate::{Ctx, cmd, files, output, output_any};
use anyhow::Result;
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::collections::BTreeSet;
use std::path::Path;

const SKIP: &[&str] = &[
    "/proc", "/sys", "/dev", "/run", "/tmp", "/var/tmp", "/home", "/root", "/nix", "/mnt", "/media", "/lost+found",
    "/var/lib/pacman", "/var/lib/pacnix", "/var/cache/pacman/pkg",
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
            report.push(format!("modified file of package {pkg}: {path}"));
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

    let units = crate::units::plan(&g, &g)?;
    report.extend(units.disable.iter().map(|u| format!("enabled unit not declared and not enabled by a vendor preset: {u}")));
    report.extend(units.enable.iter().map(|u| format!("declared unit not enabled: {u}")));

    home(&g, &origins, &mut report)?;
    if report.is_empty() {
        println!("The configuration explains the state of the system.");
    }
    for l in &report {
        println!("{l}");
    }
    Ok(())
}

/// Lists entries of ~/.config, ~/.local/share, ~/.cache and dot entries of
/// each home directory that no claim and no Home Manager file explains.
fn home(g: &Generation, origins: &Origins, report: &mut Vec<String>) -> Result<()> {
    let claims = globs(g.m.claims.values().flat_map(|c| c.home.clone()).chain(["**/*.pacnix-hm-backup".to_string()]));
    for line in std::fs::read_to_string("/etc/passwd")?.lines() {
        let f: Vec<&str> = line.split(':').collect();
        let uid: u32 = f.get(2).and_then(|u| u.parse().ok()).unwrap_or(0);
        if !(1000..60000).contains(&uid) || f.len() < 6 || !Path::new(f[5]).is_dir() {
            continue;
        }
        let dir = f[5];
        let mut managed: BTreeSet<String> = BTreeSet::new();
        if let Some(current) = crate::home::current(dir) {
            let files = format!("{current}/home-files");
            for e in walkdir::WalkDir::new(&files).min_depth(1).into_iter().flatten() {
                managed.insert(e.path().strip_prefix(&files).unwrap().display().to_string());
            }
        }
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
