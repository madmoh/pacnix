use crate::generation::{Generation, STATE};
use crate::{Ctx, cmd, failed, forward, output, output_any, report, run_cmd, run_quiet, run_silent, trust};
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::process::Command;

const REPO: &str = "/var/lib/pacnix/repo";
const BUILD: &str = "/var/lib/pacnix/build";
const CONF: &str = "/var/lib/pacnix/pacman.conf";
const LOCAL_CONF: &str = "/var/lib/pacnix/pacman-local.conf";
const SOURCES: &str = "/var/lib/pacnix/local-sources.json";
/// The signing key of the local repository, which only root reads. Hosts that
/// install from this repository check the fingerprint of its public key.
const GNUPG: &str = "/var/lib/pacnix/gnupg";
const KEY: &str = "/var/lib/pacnix/repo/pacnix-local.asc";
const PROTECTED: &[&str] = &["base", "pacman", "nix", "pacnix"];

#[derive(Default)]
pub struct Plan {
    notes: Vec<String>,
    unknown: Vec<String>,
    conflicts: Vec<String>,
    build: Vec<String>,
    local: Vec<String>,
    drop: Vec<String>,
    install: Vec<String>,
    archive: Vec<String>,
    explicit: Vec<String>,
    deps: Vec<String>,
    remove: Vec<String>,
    protected: Vec<String>,
    declared: usize,
}

impl Plan {
    /// The packages of the transaction: the packages to install and the local
    /// packages that build again, which pacman -S --needed upgrades.
    fn targets(&self) -> Vec<&str> {
        let mut t: Vec<&str> = self.install.iter().chain(&self.local).map(String::as_str).collect();
        t.sort();
        t.dedup();
        t
    }

    pub fn print(&self, l: &mut Vec<String>) {
        let mut add = |prefix: &str, list: &[String]| l.extend(list.iter().map(|p| format!("{prefix} {p}")));
        add("note:", &self.notes);
        add("unknown package", &self.unknown);
        add("build AUR package", &self.build);
        add("build local package", &self.local);
        add("drop from local repository", &self.drop);
        add("install package", &self.install);
        add("install package from the Arch Linux Archive", &self.archive);
        add("mark package explicit", &self.explicit);
        add("mark package as dependency", &self.deps);
        add("remove package", &self.remove);
    }
}

#[derive(Default, Clone, Debug)]
struct Info {
    name: String,
    version: String,
    depends: Vec<String>,
    makedepends: Vec<String>,
    provides: Vec<String>,
    conflicts: Vec<String>,
    required: Vec<String>,
    explicit: bool,
    filename: String,
}

pub fn dep_name(d: &str) -> &str {
    d.split(['<', '>', '=', ':']).next().unwrap_or(d).trim()
}

/// Parses the output of pacman -Qi or pacman -Si (LC_ALL=C).
fn parse_info(text: &str) -> Vec<Info> {
    let mut list = Vec::new();
    let mut cur = Info::default();
    let mut key = String::new();
    let list_of = |v: &str| -> Vec<String> {
        if v == "None" { vec![] } else { v.split_whitespace().map(String::from).collect() }
    };
    for line in text.lines() {
        if line.is_empty() {
            if !cur.name.is_empty() {
                list.push(std::mem::take(&mut cur));
            }
            continue;
        }
        let (k, v) = match line.split_once(" : ") {
            Some((k, v)) if !line.starts_with(' ') => (k.trim().to_string(), v.trim()),
            _ => (key.clone(), line.trim()),
        };
        key = k;
        match key.as_str() {
            "Name" => cur.name = v.into(),
            "Version" => cur.version = v.into(),
            "Depends On" => cur.depends.extend(list_of(v)),
            "Provides" => cur.provides.extend(list_of(v)),
            "Conflicts With" => cur.conflicts.extend(list_of(v)),
            "Required By" => cur.required.extend(list_of(v)),
            "Install Reason" => cur.explicit = v.starts_with("Explicitly"),
            _ => {}
        }
    }
    if !cur.name.is_empty() {
        list.push(cur);
    }
    list
}

fn pacman(args: &[&str]) -> Command {
    let mut c = cmd("pacman", args);
    c.env("LC_ALL", "C");
    c
}

fn installed() -> Result<HashMap<String, Info>> {
    Ok(parse_info(&output(&mut pacman(&["-Qi"]))?).into_iter().map(|i| (i.name.clone(), i)).collect())
}

/// The declared package names, and the names in packages.repo that are no
/// package and no group. As with pacman -S, a group that has no package of the
/// same name stands for each of its members.
pub fn declared(g: &Generation, sync: &BTreeSet<String>) -> Result<(BTreeSet<String>, Vec<String>)> {
    let p = &g.m.packages;
    let mut names: BTreeSet<String> = p.repo.iter().filter(|n| sync.contains(*n)).chain(&p.aur).chain(p.local.keys()).cloned().collect();
    let other: Vec<&String> = p.repo.iter().filter(|n| !sync.contains(*n)).collect();
    let mut groups = BTreeSet::new();
    if !other.is_empty() {
        for (group, member) in output_any(pacman(&["-Sg"]).args(&other))?.lines().filter_map(|l| l.split_once(' ')) {
            groups.insert(group.to_string());
            names.insert(member.to_string());
        }
    }
    Ok((names, other.into_iter().filter(|n| !groups.contains(*n)).cloned().collect()))
}

/// Package names in the sync databases of every repository except pacnix-local.
pub fn sync_names() -> Result<BTreeSet<String>> {
    let text = output(&mut pacman(&["-Sl"]))?;
    Ok(text
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let repo = f.next()?;
            (repo != "pacnix-local").then(|| f.next().map(String::from)).flatten()
        })
        .collect())
}

fn is_builder(g: &Generation) -> bool {
    g.m.packages.server.is_none()
}

/// True when systemd runs the system and pacnix is not in a chroot, the test
/// of systemd-detect-virt --chroot. In arch-chroot, for example during an
/// install from a live medium, clean chroot builds cannot run.
pub fn booted() -> bool {
    use std::os::unix::fs::MetadataExt;
    let same_root = match (std::fs::metadata("/"), std::fs::metadata("/proc/1/root")) {
        (Ok(a), Ok(b)) => (a.dev(), a.ino()) == (b.dev(), b.ino()),
        _ => false,
    };
    same_root && Path::new("/run/systemd/system").exists()
}

/// A builder host in a chroot builds nothing: AUR and local packages wait for
/// the first pacnix upgrade on the booted system.
pub fn defer_builds(g: &Generation) -> bool {
    is_builder(g) && !booted()
}

/// The AUR and local packages that wait for the booted system.
pub fn deferred(g: &Generation) -> Vec<String> {
    built_aur(g).into_iter().chain(local_dirs(g).into_keys()).collect()
}

/// Reads the pacnix-local database from the local repository or from the server.
fn repo_db(g: &Generation) -> Result<Vec<Info>> {
    let db = match &g.m.packages.server {
        None => {
            let p = format!("{REPO}/pacnix-local.db");
            if !Path::new(&p).exists() {
                return Ok(vec![]);
            }
            p
        }
        Some(url) => {
            let p = format!("{STATE}/server.db");
            std::fs::create_dir_all(STATE)?;
            let url = format!("{}/pacnix-local.db", url.trim_end_matches('/'));
            output(cmd("curl", &["-fsSL", "--retry", "3", "-o", &p, &url])).with_context(|| format!("download {url}"))?;
            p
        }
    };
    let text = output(cmd("bsdtar", &["-xOf", &db]))?;
    let mut list = Vec::new();
    let mut cur = Info::default();
    let mut section = "";
    for line in text.lines() {
        if line.starts_with('%') && line.ends_with('%') {
            section = match line {
                "%FILENAME%" => {
                    if !cur.name.is_empty() {
                        list.push(std::mem::take(&mut cur));
                    }
                    "file"
                }
                "%NAME%" => "name",
                "%VERSION%" => "version",
                "%DEPENDS%" => "depends",
                "%MAKEDEPENDS%" | "%CHECKDEPENDS%" => "makedepends",
                "%PROVIDES%" => "provides",
                "%CONFLICTS%" => "conflicts",
                _ => "",
            };
            continue;
        }
        if line.is_empty() {
            continue;
        }
        let v = line.to_string();
        match section {
            "file" => cur.filename = v,
            "name" => cur.name = v,
            "version" => cur.version = v,
            "depends" => cur.depends.push(v),
            "makedepends" => cur.makedepends.push(v),
            "provides" => cur.provides.push(v),
            "conflicts" => cur.conflicts.push(v),
            _ => {}
        }
    }
    if !cur.name.is_empty() {
        list.push(cur);
    }
    Ok(list)
}

/// Writes the pacman configurations: the system one plus the pacnix-local repository,
/// and one with the pacnix-local repository only, to refresh that database alone.
fn write_confs(g: &Generation) -> Result<()> {
    let server = match &g.m.packages.server {
        Some(url) => url.trim_end_matches('/').to_string(),
        None => format!("file://{REPO}"),
    };
    // root writes the local repository; its signatures are for the other hosts.
    let level = if g.m.packages.server.is_some() { "Required" } else { "Never" };
    // A builder host without AUR and local packages has no local repository.
    let section = match has_local(g) {
        true => format!("\n[pacnix-local]\nSigLevel = {level}\nServer = {server}\n"),
        false => String::new(),
    };
    let mut system = std::fs::read_to_string("/etc/pacman.conf")?;
    if SERIAL.load(std::sync::atomic::Ordering::Relaxed) {
        system = system
            .lines()
            .filter(|l| !l.trim_start().starts_with("ParallelDownloads"))
            .map(|l| if l.trim() == "[options]" { format!("{l}\nParallelDownloads = 1\n") } else { format!("{l}\n") })
            .collect();
    }
    std::fs::create_dir_all(STATE)?;
    std::fs::write(CONF, format!("{system}{section}"))?;
    std::fs::write(LOCAL_CONF, format!("[options]\nArchitecture = auto\n{section}"))?;
    Ok(())
}

fn has_local(g: &Generation) -> bool {
    !is_builder(g) || Path::new(REPO).join("pacnix-local.db.tar.gz").exists()
}

/// Refreshes the database of the pacnix-local repository only. The sync databases of
/// the Arch repositories stay as they are.
fn sync_pacnix_db() -> Result<()> {
    recover_lock()?;
    run_quiet(pacman(&["-Sy", "--config", LOCAL_CONF]))
}

fn aur_table(names: &[String]) -> Result<Vec<(String, String, String)>> {
    if names.is_empty() {
        return Ok(vec![]);
    }
    let text = output_any(cmd("aur", &["depends", "-t"]).args(names))?;
    Ok(text
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            (f.len() >= 5).then(|| (f[0].to_string(), f[1].to_string(), f[4].to_string()))
        })
        .collect())
}

fn srcinfo(dir: &str) -> Result<(Vec<String>, Vec<String>)> {
    let text = output(cmd("runuser", &["-u", "pacnix", "--", "makepkg", "--printsrcinfo"]).current_dir(dir))
        .with_context(|| format!("read the PKGBUILD in {dir}"))?;
    let (mut names, mut deps) = (vec![], vec![]);
    for line in text.lines() {
        if let Some((k, v)) = line.trim().split_once(" = ") {
            match k {
                "pkgname" => names.push(v.to_string()),
                "depends" | "makedepends" | "checkdepends" => deps.push(dep_name(v).to_string()),
                _ => {}
            }
        }
    }
    Ok((names, deps))
}

fn local_sources() -> BTreeMap<String, String> {
    std::fs::read_to_string(SOURCES).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// AUR packages that the local repository holds: declared and served ones.
fn built_aur(g: &Generation) -> Vec<String> {
    let mut list: Vec<String> = g.m.packages.aur.iter().chain(&g.m.packages.serve.aur).cloned().collect();
    list.sort();
    list.dedup();
    list
}

/// Local PKGBUILD directories that the local repository holds: declared and served ones.
fn local_dirs(g: &Generation) -> BTreeMap<String, String> {
    let mut map = g.m.packages.serve.local.clone();
    map.extend(g.m.packages.local.clone());
    map
}

pub fn plan(_ctx: &Ctx, g: &Generation) -> Result<Plan> {
    let p = &g.m.packages;
    let mut plan = Plan::default();
    let sync = sync_names()?;
    let repo = repo_db(g)?;
    let repo_names: BTreeSet<String> = repo.iter().map(|i| i.name.clone()).collect();
    let local_names: Vec<String> = p.local.keys().cloned().collect();
    let (declared, unknown) = declared(g, &sync)?;
    plan.declared = declared.len();
    plan.unknown = unknown;
    let locals = local_dirs(g);
    let mut built_aur = built_aur(g);
    built_aur.extend(p.providers.values().filter(|v| !sync.contains(*v) && !locals.contains_key(*v)).cloned());
    built_aur.sort();
    built_aur.dedup();
    let missing_aur: Vec<String> = built_aur.iter().filter(|n| !repo_names.contains(*n)).cloned().collect();
    let mut table = Vec::new();
    if defer_builds(g) {
        plan.notes.push(format!("the first pacnix upgrade on the booted system builds and installs: {}", deferred(g).join(" ")));
    } else if is_builder(g) {
        table = aur_table(&missing_aur)?;
        let found: BTreeSet<&String> = table.iter().map(|t| &t.0).collect();
        plan.unknown.extend(missing_aur.iter().filter(|n| !found.contains(n)).cloned());
        plan.build = missing_aur;
        let built = local_sources();
        for (name, dir) in &local_dirs(g) {
            if !Path::new(dir).join("PKGBUILD").exists() {
                plan.unknown.push(format!("{name} (no PKGBUILD in {dir})"));
            } else if !repo_names.contains(name) || built.get(name) != Some(dir) {
                plan.local.push(name.clone());
            }
        }
    } else {
        plan.unknown.extend(missing_aur.iter().map(|n| format!("{n} (not in the repository of {})", p.server.as_deref().unwrap_or(""))));
        plan.unknown.extend(local_names.iter().filter(|n| !repo_names.contains(*n)).map(|n| format!("{n} (not in the repository of the server)")));
    }

    // Packages that the local repository keeps: declared AUR and local packages and their dependencies.
    if is_builder(g) {
        let by_name: HashMap<&str, &Info> = repo.iter().map(|i| (i.name.as_str(), i)).collect();
        let mut keep: BTreeSet<String> = BTreeSet::new();
        let mut queue: Vec<String> = built_aur.iter().cloned().chain(local_dirs(g).into_keys()).collect();
        queue.extend(table.iter().map(|t| t.0.clone()).filter(|n| !p.providers.contains_key(n)));
        while let Some(n) = queue.pop() {
            let n = p.providers.get(&n).cloned().unwrap_or(n);
            let target = by_name.get(n.as_str()).copied().or_else(|| repo.iter().find(|i| i.provides.iter().any(|v| dep_name(v) == n)));
            if let Some(i) = target {
                if keep.insert(i.name.clone()) {
                    queue.extend(i.depends.iter().chain(&i.makedepends).map(|d| dep_name(d).to_string()));
                }
            }
        }
        // makepkg splits debug symbols into <name>-debug. It stays with its package.
        let kept = |n: &String| keep.contains(n) || n.strip_suffix("-debug").is_some_and(|b| keep.contains(b));
        plan.drop = repo_names.iter().filter(|n| !kept(n)).cloned().collect();
    }

    // Reconciliation of the installed packages.
    let inst = installed()?;
    plan.install = declared.iter().filter(|n| !inst.contains_key(*n) && !(defer_builds(g) && !sync.contains(*n))).cloned().collect();
    plan.explicit = declared.iter().filter(|n| inst.get(*n).is_some_and(|i| !i.explicit)).cloned().collect();
    plan.deps = inst.values().filter(|i| i.explicit && !declared.contains(&i.name)).map(|i| i.name.clone()).collect();
    plan.deps.sort();
    pin(g, &mut plan, &inst, &sync, &repo)?;

    // The local or served repository database is current, the synced copy of it can be older.
    let mut universe: HashMap<String, Info> = repo.iter().map(|i| (i.name.clone(), i.clone())).collect();
    let wanted: Vec<String> = plan.install.iter().filter(|n| sync.contains(*n) && !repo_names.contains(*n)).cloned().collect();
    if !wanted.is_empty() {
        let mut c = pacman(&["-Si", "--config", CONF]);
        write_confs(g)?;
        for i in parse_info(&output_any(c.args(&wanted))?) {
            universe.insert(i.name.clone(), i);
        }
    }
    for (pkg, dep, kind) in &table {
        if kind == "Depends" {
            let e = universe.entry(pkg.clone()).or_insert_with(|| Info { name: pkg.clone(), ..Default::default() });
            e.depends.push(dep.clone());
        }
    }
    // Every installed package that provides a dependency counts as required,
    // as with pacman -Qdt.
    let mut providers: HashMap<String, Vec<String>> = HashMap::new();
    for i in inst.values() {
        for p in &i.provides {
            providers.entry(dep_name(p).to_string()).or_default().push(i.name.clone());
        }
    }
    let mut keep: BTreeSet<String> = BTreeSet::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut queue: Vec<String> = declared.iter().cloned().collect();
    let mut pending_sync: Vec<String> = Vec::new();
    let mut chosen: BTreeSet<String> = BTreeSet::new();
    loop {
        while let Some(d) = queue.pop() {
            let d = p.providers.get(&d).cloned().unwrap_or(d);
            if p.providers.values().any(|v| v == &d) && !inst.contains_key(&d) {
                chosen.insert(d.clone());
            }
            if !seen.insert(d.clone()) {
                continue;
            }
            let resolved: Vec<String> = if inst.contains_key(&d) {
                vec![d.clone()]
            } else {
                providers.get(&d).cloned().unwrap_or_default()
            };
            for n in &resolved {
                keep.insert(n.clone());
                queue.extend(inst[n].depends.iter().map(|x| dep_name(x).to_string()));
            }
            if resolved.is_empty() {
                match universe.get(&d) {
                    Some(i) => queue.extend(i.depends.iter().map(|x| dep_name(x).to_string())),
                    None if sync.contains(&d) => pending_sync.push(d),
                    None => {}
                }
            }
        }
        if pending_sync.is_empty() {
            break;
        }
        // pacman resolves the closure of the new packages in one call, so that
        // one pacman -Si gives every package that the walk needs. The chosen
        // providers go with them, so that pacman does not pick a default.
        let mut names = std::mem::take(&mut pending_sync);
        let mut resolve = pacman(&["-Sp", "--noconfirm", "--print-format", "%n"]);
        resolve.args(&names).args(p.providers.values().filter(|v| sync.contains(*v) && !inst.contains_key(*v)));
        names.extend(output_any(&mut resolve)?.lines().map(String::from));
        names.sort();
        names.dedup();
        for i in parse_info(&output_any(pacman(&["-Si"]).args(&names))?) {
            queue.push(i.name.clone());
            seen.remove(&i.name);
            universe.insert(i.name.clone(), i);
        }
    }
    // pacman removes one of two declared packages that conflict, and the next
    // apply installs it again. The plan refuses them before any change.
    // Conflicts with a version constraint are left to pacman.
    let wanted: BTreeSet<&String> = declared.iter().chain(&chosen).collect();
    let info = |n: &String| inst.get(n).or_else(|| universe.get(n));
    for a in &wanted {
        for c in info(a).map(|i| i.conflicts.as_slice()).unwrap_or_default().iter().filter(|c| dep_name(c) == c.as_str()) {
            let hit = |b: &&&String| a != *b && (b.as_str() == c || info(b).is_some_and(|i| i.provides.iter().any(|v| dep_name(v) == c)));
            plan.conflicts.extend(wanted.iter().filter(hit).map(|b| format!("{} and {}", a.min(b), a.max(b))));
        }
    }
    // pacman -S marks its targets explicit. The chosen providers become dependencies after it.
    chosen.retain(|n| !declared.contains(n) && !(defer_builds(g) && !sync.contains(n)));
    plan.install.extend(chosen.iter().cloned());
    plan.deps.extend(chosen);
    plan.unknown.extend(wrong_providers(g, &repo, &sync)?);
    plan.remove = inst.keys().filter(|n| !keep.contains(*n)).cloned().collect();
    plan.remove.sort();
    let kernel = output_any(pacman(&["-Qqo", &format!("/usr/lib/modules/{}/vmlinuz", uname())]))?;
    plan.protected = plan
        .remove
        .iter()
        .filter(|n| PROTECTED.contains(&n.as_str()) || kernel.trim() == n.as_str())
        .cloned()
        .collect();
    Ok(plan)
}

/// Installs each pinned package at its version from the source that has that
/// version: the sync repositories, the local repository or the Arch Linux Archive.
fn pin(g: &Generation, plan: &mut Plan, inst: &HashMap<String, Info>, sync: &BTreeSet<String>, repo: &[Info]) -> Result<()> {
    for (name, version) in &g.m.packages.versions {
        if inst.get(name).is_some_and(|i| &i.version == version) || (defer_builds(g) && !sync.contains(name)) {
            continue;
        }
        plan.install.retain(|n| n != name);
        let source = if sync.contains(name) {
            parse_info(&output_any(pacman(&["-Si"]).arg(name))?).into_iter().next().map(|i| i.version)
        } else if let Some(i) = repo.iter().find(|i| &i.name == name) {
            Some(i.version.clone())
        } else if is_builder(g) {
            let info: serde_json::Value = serde_json::from_str(&output_any(cmd("aur", &["query", "-t", "info", name]))?).unwrap_or_default();
            info["results"][0]["Version"].as_str().map(String::from)
        } else {
            None
        };
        if source.as_ref() == Some(version) {
            plan.install.push(name.clone());
        } else if sync.contains(name) {
            match archive_url(name, version) {
                Some(url) => plan.archive.push(url),
                None => plan.unknown.push(format!("{name} {version} (not in the Arch Linux Archive)")),
            }
        } else {
            let has = source.unwrap_or_else(|| "no version".to_string());
            plan.unknown.push(format!("{name} {version} (the AUR or the local repository has {has}; declare a PKGBUILD of that version in packages.local)"));
        }
    }
    Ok(())
}

/// The URL of a package version in the Arch Linux Archive, if the archive has it.
fn archive_url(name: &str, version: &str) -> Option<String> {
    let first = name.chars().next()?;
    let urls = ["x86_64", "any"].into_iter().flat_map(|arch| {
        ["zst", "xz"].map(|ext| format!("https://archive.archlinux.org/packages/{first}/{name}/{name}-{version}-{arch}.pkg.tar.{ext}"))
    });
    urls.into_iter().find(|u| cmd("curl", &["-fsIL", "--retry", "2", "-o", "/dev/null", u]).status().is_ok_and(|s| s.success()))
}

/// pacman and aur sync arguments that keep the pinned packages at their version.
fn pinned(g: &Generation) -> Vec<String> {
    let names: Vec<&str> = g.m.packages.versions.keys().map(String::as_str).collect();
    if names.is_empty() { vec![] } else { vec!["--ignore".to_string(), names.join(",")] }
}

/// Providers that do not provide their dependency, from the sync databases, the
/// pacnix-local repository or the AUR.
fn wrong_providers(g: &Generation, repo: &[Info], sync: &BTreeSet<String>) -> Result<Vec<String>> {
    let chosen = &g.m.packages.providers;
    let mut provides: HashMap<String, Vec<String>> = repo.iter().map(|i| (i.name.clone(), i.provides.clone())).collect();
    let from_sync: Vec<&String> = chosen.values().filter(|v| sync.contains(*v)).collect();
    if !from_sync.is_empty() {
        provides.extend(parse_info(&output_any(pacman(&["-Si"]).args(&from_sync))?).into_iter().map(|i| (i.name, i.provides)));
    }
    let from_aur: Vec<&String> = chosen.values().filter(|v| !provides.contains_key(*v) && !local_dirs(g).contains_key(*v)).collect();
    if !from_aur.is_empty() && is_builder(g) {
        let info: serde_json::Value = serde_json::from_str(&output_any(cmd("aur", &["query", "-t", "info"]).args(&from_aur))?).unwrap_or_default();
        for r in info["results"].as_array().into_iter().flatten() {
            let list = r["Provides"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from));
            provides.insert(r["Name"].as_str().unwrap_or_default().to_string(), list.collect());
        }
    }
    Ok(chosen
        .iter()
        .filter(|(dep, v)| provides.get(*v).is_some_and(|l| !l.iter().any(|x| dep_name(x) == dep.as_str())))
        .map(|(dep, v)| format!("{v} (it does not provide {dep})"))
        .collect())
}

/// aur sync arguments that keep the dependencies with a chosen provider out of the AUR build.
fn ignored(g: &Generation) -> Vec<String> {
    let keys: Vec<&str> = g.m.packages.providers.keys().map(String::as_str).collect();
    if keys.is_empty() { vec![] } else { vec!["--ignore".to_string(), keys.join(",")] }
}

fn uname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default().trim().to_string()
}

/// Refuses a plan that is not safe. Runs before any change.
pub fn check(ctx: &Ctx, plan: &Plan) -> Result<()> {
    if plan.declared == 0 {
        bail!("the configuration declares no packages; pacnix refuses to remove every package");
    }
    if !plan.unknown.is_empty() {
        bail!("unknown packages: {}", plan.unknown.join(", "));
    }
    if !plan.conflicts.is_empty() {
        let mut pairs = plan.conflicts.clone();
        pairs.sort();
        pairs.dedup();
        bail!("declared packages conflict: {}. Declare one package of each pair", pairs.join(", "));
    }
    if !plan.protected.is_empty() && !ctx.opts.allow_protected_removal {
        bail!(
            "the plan removes protected packages: {}. Declare them, or give --allow-protected-removal",
            plan.protected.join(", ")
        );
    }
    Ok(())
}

/// The item of the question for the packages that the plan removes.
pub fn removals(plan: &Plan) -> Vec<String> {
    match plan.remove.is_empty() {
        true => vec![],
        false => vec![format!("Remove {} packages: {}.", plan.remove.len(), plan.remove.join(" "))],
    }
}

/// Set after a network failure: the pacman configuration of pacnix then
/// downloads one file at a time, because pacman stops the transaction when
/// one of the parallel downloads stalls. The configuration of the user stays
/// as it is.
pub static SERIAL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Downloads packages into the pacman cache before the transaction. pacman
/// -Sw keeps the complete files, thus a new attempt continues.
fn download(names: &[&str]) -> Result<()> {
    if names.is_empty() {
        return Ok(());
    }
    report::progress(&format!("download {} packages", names.len()));
    run_quiet(pacman(&["-Sw", "--noconfirm", "--ask=4", "--config", CONF]).args(names))
}

fn as_builder(args: &[&str]) -> Command {
    let mut c = cmd("runuser", &["-u", "pacnix", "--", "env", &format!("HOME={BUILD}"), "LC_ALL=C"]);
    c.args(args).current_dir(BUILD);
    c
}

/// The makepkg configuration of the host. The clean chroot copies it at each update.
const MAKEPKG_CONF: &str = "/etc/makepkg.conf";

const AUR_ARGS: &[&str] = &[
    "--chroot",
    "--database=pacnix-local",
    "--root=/var/lib/pacnix/repo",
    "--pacman-conf=/var/lib/pacnix/pacman.conf",
    "--makepkg-conf=/etc/makepkg.conf",
    "--no-confirm",
    "--no-sync",
];

/// Creates the local repository if it does not exist. A repository without
/// signatures, for example a new one, gets them.
fn ensure_repo() -> Result<()> {
    if Path::new(&format!("{REPO}/pacnix-local.db.tar.gz.sig")).exists() {
        return Ok(());
    }
    let packages: Vec<std::path::PathBuf> = std::fs::read_dir(REPO)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().contains(".pkg.tar") && !p.to_string_lossy().ends_with(".sig"))
        .collect();
    repo_add(&packages)
}

fn gpg(args: &[&str]) -> Command {
    let mut c = cmd("gpg", &["--homedir", GNUPG, "--batch"]);
    c.args(args);
    c
}

/// The fingerprints in the output of gpg --with-colons.
fn fingerprints(text: &str) -> Vec<String> {
    text.lines().filter(|l| l.starts_with("fpr:")).filter_map(|l| l.split(':').nth(9)).map(String::from).collect()
}

/// The fingerprint of the signing key of the local repository. The first call
/// makes the key and its public file in the repository, and shows the fingerprint.
fn signing_key() -> Result<String> {
    if !Path::new(GNUPG).join("pubring.kbx").exists() {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(GNUPG)?;
        let host = std::fs::read_to_string("/etc/hostname").unwrap_or_default();
        run_quiet(gpg(&["--passphrase", "", "--quick-gen-key", &format!("pacnix-local of {}", host.trim()), "ed25519", "sign", "never"]))?;
    }
    let key = fingerprints(&output(gpg(&["--with-colons", "--list-secret-keys"]))?).into_iter().next().context("the signing key of the local repository has no fingerprint")?;
    if !Path::new(KEY).exists() {
        run_quiet(gpg(&["--armor", "--output", KEY, "--export", &key]))?;
        report::last(format!("pacnix: the key {key} signs the local repository. A host that installs from it declares packages.serverKey = \"{key}\";"));
    }
    Ok(key)
}

/// Signs packages and adds them to the local repository, which then has the
/// new versions only.
fn repo_add(files: &[std::path::PathBuf]) -> Result<()> {
    let key = signing_key()?;
    for f in files {
        run_quiet(gpg(&["--yes", "--local-user", &key, "--detach-sign"]).arg(f))?;
    }
    let mut c = cmd("repo-add", &["-q", "-R", "-s", "-k", &key, &format!("{REPO}/pacnix-local.db.tar.gz")]);
    run_cmd(c.args(files).env("GNUPGHOME", GNUPG))
}

/// Trusts the signing key of packages.server with pacman-key, after a check of
/// its fingerprint against packages.serverKey.
fn trust_server(g: &Generation) -> Result<()> {
    let (Some(url), Some(key)) = (&g.m.packages.server, &g.m.packages.server_key) else { return Ok(()) };
    let key = key.replace(' ', "").to_uppercase();
    if run_silent(cmd("pacman-key", &["--list-keys", &key])).is_ok() {
        return Ok(());
    }
    report::progress("trust the signing key of the server");
    let url = format!("{}/pacnix-local.asc", url.trim_end_matches('/'));
    let file = format!("/run/pacnix/server-{}.asc", std::process::id());
    std::fs::create_dir_all("/run/pacnix")?;
    output(cmd("curl", &["-fsSL", "--retry", "3", "-o", &file, &url])).with_context(|| format!("download {url}"))?;
    let found = fingerprints(&output(cmd("gpg", &["--homedir", "/etc/pacman.d/gnupg", "--batch", "--with-colons", "--show-keys", &file]))?);
    if found.first() != Some(&key) {
        let _ = std::fs::remove_file(&file);
        bail!("the key in {url} has the fingerprint {}, but packages.serverKey is {key}", found.first().map_or("none", |f| f.as_str()));
    }
    run_quiet(cmd("pacman-key", &["--add", &file]))?;
    let _ = std::fs::remove_file(&file);
    run_quiet(cmd("pacman-key", &["--lsign-key", &key]))
}

/// aur chroot as root. devtools needs root for the clean chroot, and the
/// builder user has no way to root: a PKGBUILD that runs on the host as the
/// builder user, for example to download its sources, cannot change the system.
fn chroot(args: &[&str]) -> Command {
    let mut c = cmd("aur", &["chroot", "--pacman-conf", CONF, "--makepkg-conf", MAKEPKG_CONF, "--user", "pacnix"]);
    c.args(args).env("AUR_PACMAN_AUTH", "env").env("LC_ALL", "C");
    c
}

/// Creates the clean chroot, or updates it and its copy of the makepkg
/// configuration. mkarchroot writes .arch-chroot at the end, thus a chroot
/// without it is the remainder of a failed attempt.
fn ensure_chroot() -> Result<()> {
    let root = Path::new("/var/lib/aurbuild/x86_64/root");
    if root.join(".arch-chroot").exists() {
        report::progress("update the clean chroot");
        return run_cmd(chroot(&["--update"]));
    }
    report::progress("create the clean chroot for the builds");
    if root.exists() {
        run_cmd(cmd("rm", &["-rf", "--one-file-system"]).arg(root))?;
    }
    run_cmd(chroot(&["--create"]))
}

/// The packages to build, in build order, with a flag for local packages, the
/// new records of the AUR packages and the changes that need a confirmation.
#[derive(Default)]
pub struct Builds {
    queue: Vec<(String, bool)>,
    records: BTreeMap<String, trust::Record>,
    pub asks: Vec<String>,
}

impl Builds {
    /// The records and the changes of AUR packages, from the AUR RPC.
    fn of(queue: Vec<(String, bool)>, extra: &[&str]) -> Result<Builds> {
        let names: Vec<String> = queue.iter().filter(|q| !q.1).map(|q| base(&q.0)).chain(extra.iter().map(|e| e.to_string())).collect();
        let (asks, records) = trust::changes(&names)?;
        Ok(Builds { queue, records, asks })
    }
}

fn base(dir: &str) -> String {
    Path::new(dir).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Gets the PKGBUILDs of the AUR and local packages to build, before any
/// change: aur sync --no-build gives the AUR packages with their AUR
/// dependencies in build order.
pub fn fetch_builds(g: &Generation, plan: &Plan) -> Result<Builds> {
    if !is_builder(g) || defer_builds(g) || (plan.build.is_empty() && plan.local.is_empty()) {
        return Ok(Builds::default());
    }
    ensure_repo()?;
    write_confs(g)?;
    sync_pacnix_db()?;
    let mut aur: Vec<String> = plan.build.clone();
    let mut dirs = Vec::new();
    let sync = sync_names()?;
    let dirs_of = local_dirs(g);
    for name in &plan.local {
        let dir = format!("{BUILD}/local/{name}");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(format!("{BUILD}/local"))?;
        run_cmd(cmd("cp", &["-rT", &dirs_of[name], &dir]))?;
        run_cmd(cmd("chown", &["-R", "pacnix:pacnix", &format!("{BUILD}/local")]))?;
        run_cmd(cmd("chmod", &["-R", "u+w", &dir]))?;
        let (_, deps) = srcinfo(&dir)?;
        // Dependencies that no sync package satisfies, by name or by provides, come from the AUR.
        let satisfied = |d: &String| {
            sync.contains(d) || dirs_of.contains_key(d) || g.m.packages.providers.contains_key(d) || pacman(&["-Sddp", "--config", CONF, d]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success())
        };
        aur.extend(deps.into_iter().filter(|d| !satisfied(d)));
        dirs.push(dir);
    }
    aur.sort();
    aur.dedup();
    let mut queue = Vec::new();
    if !aur.is_empty() {
        report::progress(&format!("download {} AUR packages and their AUR dependencies", aur.len()));
        let fetched = fetch(as_builder(&["aur", "sync", "--no-view", "--no-build"]).args(AUR_ARGS).args(ignored(g)).args(&aur))?;
        queue.extend(fetched.into_iter().map(|d| (d, false)));
    }
    queue.extend(dirs.into_iter().map(|d| (d, true)));
    Builds::of(queue, &[])
}

/// Builds AUR and local packages into the local repository and drops packages
/// that no declaration needs.
fn build(g: &Generation, plan: &Plan, builds: Builds) -> Result<()> {
    build_dirs(g, builds)?;
    if !plan.local.is_empty() {
        let mut sources = local_sources();
        let dirs_of = local_dirs(g);
        for name in &plan.local {
            sources.insert(name.clone(), dirs_of[name].clone());
        }
        std::fs::write(SOURCES, serde_json::to_string_pretty(&sources)?)?;
    }
    if !plan.drop.is_empty() {
        let files: Vec<String> = repo_db(g)?.into_iter().filter(|i| plan.drop.contains(&i.name)).map(|i| i.filename).collect();
        let key = signing_key()?;
        run_cmd(cmd("repo-remove", &["-q", "-s", "-k", &key, &format!("{REPO}/pacnix-local.db.tar.gz")]).args(&plan.drop).env("GNUPGHOME", GNUPG))?;
        for f in files {
            let _ = std::fs::remove_file(format!("{REPO}/{f}"));
            let _ = std::fs::remove_file(format!("{REPO}/{f}.sig"));
        }
    }
    Ok(())
}

/// Runs aur sync --no-build and returns the package directories in build
/// order. The list goes to a file, so that all the output of aur sync goes
/// through run_cmd and a network failure is visible in it.
fn fetch(mut c: impl std::borrow::BorrowMut<Command>) -> Result<Vec<String>> {
    let list = format!("{BUILD}/sync.queue");
    let _ = std::fs::remove_file(&list);
    run_cmd(c.borrow_mut().args(["--save", &list]))?;
    Ok(std::fs::read_to_string(&list).unwrap_or_default().lines().map(String::from).collect())
}

/// Builds package directories in their order, one by one, so that the
/// progress shows each package. An AUR package that the local repository has
/// at the version of its .SRCINFO is skipped, thus a new attempt continues
/// after the last package that was built. Local packages build again each
/// time. Before the build of an AUR package, the output shows the changes of
/// its PKGBUILD since its last build; after the build, its record keeps the
/// built revision. root moves the packages into the local repository, which
/// the builder user cannot change.
fn build_dirs(g: &Generation, mut builds: Builds) -> Result<()> {
    let out = format!("{BUILD}/out");
    let built: HashMap<String, String> = repo_db(g)?.into_iter().map(|i| (i.name, i.version)).collect();
    let count = builds.queue.len();
    for (i, (dir, local)) in builds.queue.iter().enumerate() {
        let name = base(dir);
        let step = format!("build {} package {}/{count}: {name}", if *local { "local" } else { "AUR" }, i + 1);
        report::progress(&step);
        let record = builds.records.remove(&name);
        let head = match local {
            true => String::new(),
            false => output(as_builder(&["git", "-C", dir, "rev-parse", "HEAD"]))?.trim().to_string(),
        };
        // Without a record of a build, the changes start at the empty tree of Git: all files.
        let old = record.as_ref().map(|r| r.head.clone()).filter(|h| !h.is_empty()).unwrap_or("4b825dc642cb6eb9a060e54bf8d69288fbee4904".to_string());
        if !local && old != head {
            println!("pacnix: changes of the AUR package {name} since its last build:");
            run_cmd(as_builder(&["git", "-C", dir, "--no-pager", "diff", "--stat", "--patch", &old, &head]))?;
        }
        let (names, version) = srcinfo_file(dir);
        if *local || !names.iter().all(|n| built.get(n) == Some(&version)) {
            let _ = std::fs::remove_dir_all(&out);
            run_cmd(as_builder(&["mkdir", &out]))?;
            let mut c = chroot(&["--build"]);
            c.current_dir(dir).env("PKGDEST", &out);
            run_cmd(c).context(step)?;
            // No process of the builder user stays to change the packages.
            let _ = cmd("pkill", &["-KILL", "-u", "pacnix"]).status();
            let mut files = Vec::new();
            for e in std::fs::read_dir(&out)? {
                let path = e?.path();
                if path.to_string_lossy().contains(".pkg.tar") && !path.to_string_lossy().ends_with(".sig") {
                    let target = Path::new(REPO).join(path.file_name().unwrap_or_default());
                    std::os::unix::fs::chown(&path, Some(0), Some(0))?;
                    std::fs::rename(&path, &target)?;
                    files.push(target);
                }
            }
            repo_add(&files)?;
        }
        if let Some(mut r) = record {
            r.head = head;
            trust::save(&name, r)?;
        }
    }
    Ok(())
}

/// The package names and the version in the .SRCINFO of an AUR package.
fn srcinfo_file(dir: &str) -> (Vec<String>, String) {
    let text = std::fs::read_to_string(Path::new(dir).join(".SRCINFO")).unwrap_or_default();
    let (mut names, mut fields) = (Vec::new(), BTreeMap::new());
    for (k, v) in text.lines().filter_map(|l| l.trim().split_once(" = ")) {
        match k {
            "pkgname" => names.push(v.to_string()),
            "epoch" | "pkgver" | "pkgrel" => {
                fields.entry(k).or_insert(v);
            }
            _ => {}
        }
    }
    let epoch = fields.get("epoch").filter(|e| **e != "0").map(|e| format!("{e}:")).unwrap_or_default();
    (names, format!("{epoch}{}-{}", fields.get("pkgver").unwrap_or(&""), fields.get("pkgrel").unwrap_or(&"")))
}

/// Removes a stale pacman lock that a killed transaction left, and the local
/// database entries that the transaction did not complete. The package then
/// counts as not installed, and the transaction installs it again.
pub fn recover_lock() -> Result<()> {
    let lock = Path::new("/var/lib/pacman/db.lck");
    if lock.exists() {
        let running = std::fs::read_dir("/proc")?.flatten().any(|e| {
            std::fs::read_to_string(e.path().join("comm")).is_ok_and(|c| c.trim() == "pacman")
        });
        if running {
            bail!("pacman is running");
        }
        eprintln!("pacnix: remove the stale pacman lock of an interrupted transaction");
        std::fs::remove_file(lock)?;
    }
    for e in std::fs::read_dir("/var/lib/pacman/local")?.flatten() {
        if e.path().is_dir() && !(e.path().join("desc").exists() && e.path().join("files").exists()) {
            eprintln!("pacnix: remove the incomplete database entry {}", e.path().display());
            std::fs::remove_dir_all(e.path())?;
        }
    }
    Ok(())
}

/// Runs a pacman transaction. If an interrupted transaction left files of a
/// package that the database does not record, pacman reports them as conflicts.
/// pacnix then repeats the transaction with --overwrite for those files only,
/// when no installed package owns them.
fn transaction(args: &[&str]) -> Result<()> {
    recover_lock()?;
    let mut c = pacman(args);
    let (status, lines) = forward(&mut c, usize::MAX, true, |_| true)?;
    if status.success() {
        return Ok(());
    }
    let conflicts: Vec<&str> = lines
        .iter()
        .filter_map(|l| l.split_once(": ")?.1.strip_suffix(" exists in filesystem"))
        .filter(|f| output_any(&mut pacman(&["-Qqo", f])).is_ok_and(|o| o.trim().is_empty()))
        .collect();
    if conflicts.is_empty() {
        return Err(failed(&c, status, &lines));
    }
    eprintln!("pacnix: replace {} files that no installed package owns, left by an interrupted transaction", conflicts.len());
    let mut c = pacman(args);
    for f in conflicts {
        c.args(["--overwrite", f]);
    }
    run_cmd(&mut c)
}

/// Builds packages and downloads the packages to install, before units stop,
/// so that a change of the network service still downloads, and the
/// transaction then installs from the cache. Without packages to install,
/// nothing downloads.
pub fn prepare(g: &Generation, plan: &Plan, builds: Builds) -> Result<()> {
    write_confs(g)?;
    trust_server(g)?;
    if is_builder(g) && (!g.m.packages.aur.is_empty() || !g.m.packages.local.is_empty() || has_local(g)) {
        ensure_repo()?;
        write_confs(g)?;
        if !defer_builds(g) {
            // The build asks pacman for the dependencies, which needs the
            // pacnix database also on a new host.
            sync_pacnix_db()?;
            if !builds.queue.is_empty() {
                ensure_chroot()?;
            }
            build(g, plan, builds)?;
        }
    }
    if has_local(g) {
        sync_pacnix_db()?;
    }
    download(&plan.targets())
}

pub fn apply(plan: &Plan) -> Result<()> {
    if !plan.targets().is_empty() {
        report::progress(&format!("install {} packages", plan.targets().len()));
        let mut args = vec!["-S", "--needed", "--noconfirm", "--ask=4", "--config", CONF];
        args.extend(plan.targets());
        transaction(&args)?;
    }
    if !plan.archive.is_empty() {
        report::progress(&format!("install {} packages from the Arch Linux Archive", plan.archive.len()));
        let mut args = vec!["-U", "--noconfirm", "--ask=4", "--config", CONF];
        args.extend(plan.archive.iter().map(String::as_str));
        // pacman downloads the files of -U first and stops before a change when a download fails.
        transaction(&args)?;
    }
    if !plan.explicit.is_empty() {
        run_cmd(pacman(&["-D", "--asexplicit"]).args(&plan.explicit))?;
    }
    if !plan.deps.is_empty() {
        run_cmd(pacman(&["-D", "--asdeps"]).args(&plan.deps))?;
    }
    if !plan.remove.is_empty() {
        report::progress(&format!("remove {} packages", plan.remove.len()));
        transaction(&[&["-Rns", "--noconfirm"][..], &plan.remove.iter().map(String::as_str).collect::<Vec<_>>()].concat())?;
    }
    let orphans = output_any(&mut pacman(&["-Qqdtt"]))?;
    let orphans: Vec<&str> = orphans.split_whitespace().collect();
    if !orphans.is_empty() {
        eprintln!("pacnix: remove packages that no declared package needs: {}", orphans.join(" "));
        transaction(&[&["-Rns", "--noconfirm"][..], &orphans].concat())?;
    }
    Ok(())
}

/// The AUR packages that upgrade, before any change, with their records and the
/// changes that need a confirmation. A host without a local repository has
/// none yet: the apply after the upgrade builds them.
pub fn upgrade_builds(g: &Generation) -> Result<Builds> {
    if !is_builder(g) || !Path::new(REPO).join("pacnix-local.db.tar.gz").exists() {
        return Ok(Builds::default());
    }
    write_confs(g)?;
    // aur sync keeps the pinned AUR packages and the dependencies with a chosen provider.
    let ignore: Vec<&str> = g.m.packages.providers.keys().chain(g.m.packages.versions.keys()).map(String::as_str).collect();
    let mut c = as_builder(&["aur", "sync", "--no-view", "--upgrades", "--no-build"]);
    if !ignore.is_empty() {
        c.args(["--ignore", &ignore.join(",")]);
    }
    report::progress("download the changes of the AUR packages");
    let dirs = fetch(c.args(AUR_ARGS))?;
    Builds::of(dirs.into_iter().map(|d| (d, false)).collect(), &[])
}

/// Full upgrade: all sync databases, the system packages, then changed AUR packages.
pub fn upgrade(g: &Generation, builds: Builds) -> Result<()> {
    write_confs(g)?;
    trust_server(g)?;
    report::progress("upgrade the system with pacman -Syu");
    let pins = pinned(g);
    let pins: Vec<&str> = pins.iter().map(String::as_str).collect();
    run_quiet(pacman(&[&["-Syuw", "--noconfirm", "--config", CONF][..], &pins].concat()))?;
    transaction(&[&["-Su", "--noconfirm", "--config", CONF][..], &pins].concat())?;
    if !builds.queue.is_empty() {
        ensure_chroot()?;
        build_dirs(g, builds)?;
        sync_pacnix_db()?;
        transaction(&[&["-Su", "--noconfirm", "--config", CONF][..], &pins].concat())?;
    }
    Ok(())
}

/// The packages that pacnix uninstall removes: pacnix, aurutils and nix, less
/// the ones that a different package needs, and the dependencies that only
/// they need.
pub fn uninstall_plan() -> Result<Vec<String>> {
    let inst = installed()?;
    let mut targets: Vec<&str> = ["pacnix", "aurutils", "nix"].into_iter().filter(|n| inst.contains_key(*n)).collect();
    while let Some(i) = targets.iter().position(|t| inst[*t].required.iter().any(|r| !targets.contains(&r.as_str()))) {
        targets.remove(i);
    }
    if targets.is_empty() {
        return Ok(vec![]);
    }
    Ok(output(pacman(&["-Rsp", "--print-format", "%n"]).args(&targets))?.lines().map(String::from).collect())
}

/// Disables and stops the system units of the packages, then removes the packages.
pub fn uninstall(remove: &[String]) -> Result<()> {
    if remove.is_empty() {
        return Ok(());
    }
    let files = output(pacman(&["-Qlq"]).args(remove))?;
    let units: Vec<&str> = files.lines().filter_map(|l| l.strip_prefix("/usr/lib/systemd/system/")).filter(|u| u.contains('.') && !u.contains('/') && !u.contains("@.")).collect();
    if !units.is_empty() {
        run_cmd(cmd("systemctl", &["disable", "--now"]).args(&units))?;
    }
    report::progress(&format!("remove {} packages", remove.len()));
    transaction(&[&["-Rns", "--noconfirm"][..], &remove.iter().map(String::as_str).collect::<Vec<_>>()].concat())
}
