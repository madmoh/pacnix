use crate::generation::Generation;
use crate::{Ctx, cmd, count, failed, forward, isolated, output, output_any, report, run_cmd, trust};
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::process::Command;

const REPO: &str = "/var/lib/pacnix/repo";
const BUILD: &str = "/var/lib/pacnix/build";
/// The pacman configurations of pacnix. Each run writes them again, thus they
/// are in /run, and pacnix plan does not change /var/lib/pacnix.
pub const CONF: &str = "/run/pacnix/pacman.conf";
const LOCAL_CONF: &str = "/run/pacnix/pacman-local.conf";
/// The first cache directory of the pacman configuration of pacnix. pacman
/// downloads only into the first cache directory and reads the others, thus
/// the cache directories of the system stay as they are. An apply without a
/// failure deletes the files at the end; after a failure, the next attempt
/// uses them.
pub const STAGING: &str = "/var/lib/pacnix/cache";
const SOURCES: &str = "/var/lib/pacnix/local-sources.json";
/// The packages that pacnix installed or marked explicit for a declaration.
/// Only these become dependencies again when their declaration goes.
/// The build directories that can have sources: see `build_cache`.
const CACHE: &str = "/var/lib/pacnix/build-cache.json";
/// The AUR checkouts of aur sync, in the home directory of the builder user.
const AURDEST: &str = "/var/lib/pacnix/build/.cache/aurutils/sync";
/// The package bases that did not build in this run.
static FAILED_BASES: std::sync::Mutex<BTreeSet<String>> = std::sync::Mutex::new(BTreeSet::new());
const MADE: &str = "/var/lib/pacnix/packages.json";
/// The packages that left the local repository after their install, with the
/// installed version: see `prune`.
const PRUNED: &str = "/var/lib/pacnix/pruned.json";
/// The signing key of the local repository, which only root reads. Hosts that
/// install from this repository check the fingerprint of its public key.
const GNUPG: &str = "/var/lib/pacnix/gnupg";
const KEY: &str = "/var/lib/pacnix/repo/pacnix-local.asc";
const PROTECTED: &[&str] = &["base", "pacman", "nix", "pacnix"];

#[derive(Default)]
pub struct Plan {
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
    /// The record of the packages that pacnix made.
    made: BTreeSet<String>,
    /// The orphans before the apply, which stay.
    orphans: BTreeSet<String>,
    /// The dependencies of the packages that are not installed.
    depends: HashMap<String, Vec<String>>,
    /// The local repository has package files that its database does not name.
    stale: bool,
}

impl Plan {
    /// The packages of the transaction: the packages to install and the local
    /// packages that build again, which pacman -S --needed upgrades, less the
    /// skipped packages.
    fn targets(&self, skip: &BTreeSet<String>) -> Vec<&str> {
        let mut t: Vec<&str> = self.install.iter().chain(&self.local).map(String::as_str).filter(|n| !skip.contains(*n)).collect();
        t.sort();
        t.dedup();
        t
    }

    /// The packages that did not build and the targets that need one of them,
    /// also through other targets.
    fn skipped(&self, failed: BTreeSet<String>) -> BTreeSet<String> {
        let mut skip = failed;
        let needs = |t: &str, skip: &BTreeSet<String>| self.depends.get(t).and_then(|d| d.iter().find(|d| skip.contains(*d))).cloned();
        while let Some((t, d)) = self.targets(&skip).into_iter().find_map(|t| needs(t, &skip).map(|d| (t.to_string(), d))) {
            report::failure(&format!("install package {t}"), &format!("it needs {d}, which did not build."));
            skip.insert(t);
        }
        skip
    }

    /// The lines of the packages, and the lines of the changes of their install reason.
    pub fn print(&self, l: &mut crate::generation::Lines, marks: &mut crate::generation::Lines) {
        let mut add = |prefix: &str, list: &[String]| l.extend(list.iter().map(|p| (p.clone(), format!("{prefix} {p}"))));
        add("Build AUR package", &self.build);
        add("Build local package", &self.local);
        add("Drop from local repository", &self.drop);
        add("Install package", &self.install);
        // An archive package is a URL: its item is the name of the package.
        let name = |url: &String| url.rsplit('/').next().and_then(|f| f.rsplitn(4, '-').nth(3)).unwrap_or(url).to_string();
        l.extend(self.archive.iter().map(|u| (name(u), format!("Install package from the Arch Linux Archive {u}"))));
        l.extend(self.remove.iter().map(|p| (p.clone(), format!("Remove package {p} (apply asks first)"))));
        marks.extend(self.explicit.iter().map(|p| (p.clone(), format!("Set the install reason of {p} to explicit (it is installed as a dependency)"))));
        // A package that leaves in the same run gets its reason only as a step of the removal.
        let deps = self.deps.iter().filter(|p| !self.remove.contains(p));
        marks.extend(deps.map(|p| (p.clone(), format!("{p}: set the install reason to installed as a dependency"))));
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

/// pacman with the configuration of pacnix, when it exists: the declared
/// pacman.conf and the local repository.
fn pacman(args: &[&str]) -> Command {
    let mut c = cmd("pacman", args);
    if !args.contains(&"--config") && Path::new(CONF).exists() {
        c.args(["--config", CONF]);
    }
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
/// of systemd-detect-virt --chroot.
pub fn booted() -> bool {
    use std::os::unix::fs::MetadataExt;
    let same_root = match (std::fs::metadata("/"), std::fs::metadata("/proc/1/root")) {
        (Ok(a), Ok(b)) => (a.dev(), a.ino()) == (b.dev(), b.ino()),
        _ => false,
    };
    same_root && Path::new("/run/systemd/system").exists()
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
            let p = "/run/pacnix/server.db".to_string();
            std::fs::create_dir_all("/run/pacnix")?;
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
    // The declared pacman.conf applies from the start, also in the chroot of an
    // installation, where /etc/pacman.conf is still the file of the package.
    let declared = g.m.files.get("/etc/pacman.conf").filter(|f| f.target.is_none()).map_or("/etc/pacman.conf", |f| f.source.as_str());
    // STAGING comes first, then the cache directories of the system, which
    // include the default when the configuration names none.
    let mut options = std::iter::once(STAGING.to_string()).chain(output(cmd("pacman-conf", &["--config", declared, "CacheDir"]))?.lines().map(String::from)).map(|d| format!("CacheDir = {d}\n")).collect::<String>();
    let serial = SERIAL.load(std::sync::atomic::Ordering::Relaxed);
    if serial {
        options.push_str("ParallelDownloads = 1\n");
    }
    let key = |l: &str| l.split('=').next().unwrap_or("").trim().to_string();
    let system: String = std::fs::read_to_string(declared)?
        .lines()
        .filter(|l| key(l) != "CacheDir" && !(serial && key(l) == "ParallelDownloads"))
        .map(|l| if l.trim() == "[options]" { format!("{l}\n{options}") } else { format!("{l}\n") })
        .collect();
    // pacman warns about a repository without a synced database. The plan reads
    // the database of pacnix-local from the repository, thus the section of
    // pacnix-local goes into the system configuration after its first sync.
    let db = output(cmd("pacman-conf", &["--config", declared, "DBPath"]))?;
    let synced = Path::new(db.trim()).join("sync/pacnix-local.db").exists();
    std::fs::create_dir_all("/run/pacnix")?;
    std::fs::write(CONF, format!("{system}{}", if synced { section.as_str() } else { "" }))?;
    std::fs::write(LOCAL_CONF, format!("[options]\nArchitecture = auto\n{section}"))?;
    Ok(())
}

fn has_local(g: &Generation) -> bool {
    !is_builder(g) || Path::new(REPO).join("pacnix-local.db.tar.gz").exists()
}

/// Refreshes the database of the pacnix-local repository only. The sync databases of
/// the Arch repositories stay as they are.
fn sync_pacnix_db(g: &Generation) -> Result<()> {
    recover_lock()?;
    run_cmd(pacman(&["-Sy", "--config", LOCAL_CONF]))?;
    write_confs(g)
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

/// Writes the .SRCINFO of a local package, as an AUR package has one, and
/// returns its dependencies.
fn srcinfo(dir: &str) -> Result<Vec<String>> {
    let text = read_srcinfo(dir, &[BUILD])?;
    std::fs::write(Path::new(dir).join(".SRCINFO"), text)?;
    Ok(srcinfo_file(dir).2)
}

fn read_srcinfo(dir: &str, writable: &[&str]) -> Result<String> {
    let mut c = cmd("makepkg", &["--printsrcinfo"]);
    c.env("HOME", BUILD).env("LC_ALL", "C").current_dir(dir);
    if writable.is_empty() {
        // makepkg checks these directories even when it only prints metadata.
        c.envs([("BUILDDIR", "/tmp"), ("PKGDEST", "/tmp"), ("SRCDEST", "/tmp")]);
    }
    output(&mut sandboxed(c, Some("pacnix"), writable)).with_context(|| format!("read the PKGBUILD in {dir}"))
}

/// Reads the runtime dependencies of one output from makepkg's SRCINFO.
fn local_depends(text: &str, name: &str) -> Vec<String> {
    let arch_depends = format!("depends_{}", std::env::consts::ARCH);
    let mut base = BTreeMap::new();
    let mut package = BTreeMap::new();
    let mut section = None;
    for (key, value) in text.lines().filter_map(|l| l.trim_start().split_once('=')) {
        let (key, value) = (key.trim_end(), value.trim());
        if key == "pkgname" {
            section = Some(value);
        }
        if key == "depends" || key == arch_depends {
            let fields = match section {
                None => &mut base,
                Some(n) if n == name => &mut package,
                _ => continue,
            };
            let deps: &mut Vec<String> = fields.entry(key).or_default();
            if !value.is_empty() {
                deps.push(dep_name(value).to_string());
            }
        }
    }
    base.extend(package);
    base.into_values().flatten().collect()
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

/// The record of the packages that pacnix made. Without the record, the
/// packages of the earlier generation, which pacnix 0.10 made explicit.
fn made(old: &BTreeSet<String>) -> BTreeSet<String> {
    std::fs::read(MADE).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_else(|| old.clone())
}

/// Writes the record: the earlier record and the packages of this apply, less
/// the packages that are no longer explicitly installed.
fn save_made(plan: &Plan) -> Result<()> {
    let explicit: BTreeSet<String> = output_any(&mut pacman(&["-Qqe"]))?.split_whitespace().map(String::from).collect();
    let list: BTreeSet<&String> = plan.made.iter().chain(&plan.install).chain(&plan.explicit).filter(|n| explicit.contains(*n)).collect();
    std::fs::write(MADE, serde_json::to_vec(&list)?)?;
    Ok(())
}

/// pacnix changes only the declared packages and the packages that it made:
/// these become dependencies again when their declaration goes, and pacman
/// removes the orphans that this makes. The other packages and the orphans of
/// before stay.
pub fn plan(_ctx: &Ctx, g: &Generation, old: &Generation) -> Result<Plan> {
    let p = &g.m.packages;
    let mut plan = Plan::default();
    write_confs(g)?;
    let sync = sync_names()?;
    let repo = repo_db(g)?;
    let repo_names: BTreeSet<String> = repo.iter().map(|i| i.name.clone()).collect();
    let inst = installed()?;
    // A package that left the local repository is built while the installed
    // version is the built one, on a host that serves no packages, and while
    // the configuration declares no file of the package: the restore of the
    // file needs its package file.
    let mut have = repo_names.clone();
    if !serves(g) {
        let gone: BTreeMap<String, String> = pruned().into_iter().filter(|(n, v)| inst.get(n).is_some_and(|i| &i.version == v)).collect();
        let owners = if gone.is_empty() { BTreeMap::new() } else { crate::files::owners(&g.m.files.keys().chain(old.m.files.keys()).map(String::as_str).collect::<Vec<_>>()) };
        have.extend(gone.into_keys().filter(|n| !owners.values().any(|o| o == n)));
    }
    let local_names: Vec<String> = p.local.keys().cloned().collect();
    let (declared, unknown) = declared(g, &sync)?;
    plan.unknown = unknown.into_iter().map(|n| format!("packages.repo: {}", suggest(n, &sync))).collect();
    let locals = local_dirs(g);
    let mut built_aur = built_aur(g);
    built_aur.extend(p.providers.values().filter(|v| !sync.contains(*v) && !locals.contains_key(*v)).cloned());
    built_aur.sort();
    built_aur.dedup();
    let missing_aur: Vec<String> = built_aur.iter().filter(|n| !have.contains(*n)).cloned().collect();
    let mut table = Vec::new();
    if is_builder(g) {
        plan.stale = !stale(&repo).is_empty();
        table = aur_table(&missing_aur)?;
        let found: BTreeSet<&String> = table.iter().map(|t| &t.0).collect();
        plan.unknown.extend(missing_aur.iter().filter(|n| !found.contains(n)).map(|n| format!("packages.aur: {n} (not in the AUR; search at https://aur.archlinux.org/packages)")));
        plan.build = missing_aur;
        let built = local_sources();
        for (name, dir) in &local_dirs(g) {
            if !Path::new(dir).join("PKGBUILD").exists() {
                plan.unknown.push(format!("packages.local: {name} (no PKGBUILD in {dir})"));
            } else if !have.contains(name) || built.get(name) != Some(dir) {
                plan.local.push(name.clone());
            }
        }
    } else {
        plan.unknown.extend(missing_aur.iter().map(|n| format!("packages.aur: {n} (not in the repository of {})", p.server.as_deref().unwrap_or(""))));
        plan.unknown.extend(local_names.iter().filter(|n| !repo_names.contains(*n)).map(|n| format!("packages.local: {n} (not in the repository of the server)")));
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
    plan.made = made(&self::declared(old, &sync)?.0);
    plan.install = declared.iter().filter(|n| !inst.contains_key(*n)).cloned().collect();
    plan.explicit = declared.iter().filter(|n| inst.get(*n).is_some_and(|i| !i.explicit)).cloned().collect();
    plan.deps = inst.values().filter(|i| i.explicit && !declared.contains(&i.name) && plan.made.contains(&i.name)).map(|i| i.name.clone()).collect();
    plan.deps.sort();
    plan.orphans = output_any(&mut pacman(&["-Qqdtt"]))?.split_whitespace().map(String::from).collect();
    pin(g, &mut plan, &inst, &sync, &repo)?;

    // A local replacement may not have a repository entry until it builds.
    // Read its current dependencies before planning which old packages can go.
    let mut local_needs = HashMap::new();
    if !plan.deps.is_empty() {
        for name in &plan.local {
            let src = read_srcinfo(&locals[name], &[])?;
            local_needs.insert(name.clone(), local_depends(&src, name));
        }
    }

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
    queue.extend(inst.values().filter(|i| i.explicit && !plan.deps.contains(&i.name)).map(|i| i.name.clone()));
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
            if let Some(deps) = local_needs.get(&d) {
                keep.insert(d);
                queue.extend(deps.iter().cloned());
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
    chosen.retain(|n| !declared.contains(n));
    plan.install.extend(chosen.iter().cloned());
    plan.deps.extend(chosen);
    plan.unknown.extend(wrong_providers(g, &repo, &sync)?);
    plan.depends = universe.into_iter().map(|(n, i)| (n, i.depends.iter().map(|d| dep_name(d).to_string()).collect())).collect();
    plan.depends.extend(local_needs);
    // Only the packages that become dependencies and their dependencies can leave.
    let mut reach: BTreeSet<String> = BTreeSet::new();
    let mut queue: Vec<String> = plan.deps.clone();
    while let Some(n) = queue.pop() {
        for n in inst.contains_key(&n).then(|| vec![n.clone()]).unwrap_or_else(|| providers.get(&n).cloned().unwrap_or_default()) {
            if reach.insert(n.clone()) {
                queue.extend(inst[&n].depends.iter().map(|d| dep_name(d).to_string()));
            }
        }
    }
    plan.remove = inst.keys().filter(|n| reach.contains(*n) && !keep.contains(*n) && !plan.orphans.contains(*n)).cloned().collect();
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
        if inst.get(name).is_some_and(|i| &i.version == version) {
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
                None => plan.unknown.push(format!("packages.versions: {name} {version} (not in the Arch Linux Archive)")),
            }
        } else {
            let has = source.unwrap_or_else(|| "no version".to_string());
            plan.unknown.push(format!("packages.versions: {name} {version} (the AUR or the local repository has {has}; declare a PKGBUILD of that version in packages.local)"));
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
        .map(|(dep, v)| format!("packages.providers: {v} (it does not provide {dep})"))
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
/// The name with the sync packages of the closest names, for a name that does not exist.
fn suggest(name: String, sync: &BTreeSet<String>) -> String {
    let mut close: Vec<(usize, &String)> = sync.iter().map(|s| (strsim::levenshtein(&name, s), s)).filter(|(d, _)| *d <= 2).collect();
    close.sort();
    // Only the closest names.
    let best = close.first().map_or(0, |c| c.0);
    close.retain(|c| c.0 == best);
    match close.len() {
        0 => format!("{name} (not in the sync databases; search with pacman -Ss, or declare an AUR package in packages.aur)"),
        _ => format!("{name} (did you mean {}?)", close.iter().take(3).map(|c| c.1.as_str()).collect::<Vec<_>>().join(", ")),
    }
}

/// The installed packages that the local sync databases have in a newer version.
pub fn newer() -> Result<Vec<String>> {
    Ok(output_any(pacman(&["-Qqu", "--config", CONF]))?.lines().map(String::from).collect())
}

/// Whether pacnix installed the package or set it to explicitly installed.
impl Plan {
    pub fn made(&self, name: &str) -> bool {
        self.made.contains(name)
    }
}

pub fn check(ctx: &Ctx, plan: &Plan) -> Result<()> {
    if !plan.unknown.is_empty() {
        let line = |u: &String| {
            let (option, rest) = u.split_once(": ").unwrap_or(("packages", u));
            let name = rest.split(' ').next().unwrap_or(rest);
            let at = crate::generation::locate(ctx, &format!("\"{name}\"")).map(|at| format!(", in {at}")).unwrap_or_default();
            format!("{option}: unknown package {rest}{at}")
        };
        bail!("{}", plan.unknown.iter().map(line).collect::<Vec<_>>().join("\n"));
    }
    if !plan.conflicts.is_empty() {
        let mut pairs = plan.conflicts.clone();
        pairs.sort();
        pairs.dedup();
        bail!("declared packages conflict: {}. Declare one package of each pair.", pairs.join(", "));
    }
    if !plan.protected.is_empty() && !ctx.opts.allow_protected_removal {
        bail!(
            "the plan removes protected packages: {}. Declare them, or give --allow-protected-removal.",
            plan.protected.join(", ")
        );
    }
    Ok(())
}

/// The item of the question for the packages that the plan removes.
pub fn removals(plan: &Plan) -> Vec<String> {
    match plan.remove.is_empty() {
        true => vec![],
        false if plan.remove.len() == 1 => vec![format!("Remove 1 package that is no longer declared and that pacnix installed: {}.", plan.remove[0])],
        false => vec![format!(
            "Remove {} packages that are no longer declared and that pacnix installed, with the dependencies that only they need: {}.",
            plan.remove.len(),
            plan.remove.join(", ")
        )],
    }
}

/// Set after a network failure: the pacman configuration of pacnix then
/// downloads one file at a time, because pacman stops the transaction when
/// one of the parallel downloads stalls. The configuration of the user stays
/// as it is.
pub static SERIAL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Downloads packages into the pacman cache before the transaction, with
/// pacman -Sw and the arguments: the names or -u for the upgrade. pacman -Sw
/// keeps the complete files, thus a new attempt continues. Without a
/// terminal, pacman shows no progress, thus pacnix shows the packages and
/// MiB in the cache every 30 s, from the sizes that pacman -Sp gives.
fn download(args: &[&str]) -> Result<()> {
    std::fs::create_dir_all(STAGING)?;
    let list = output(pacman(&["-Sp", "--print-format", "%s %l", "--config", CONF]).args(args))?;
    let files: Vec<(String, u64)> = list
        .lines()
        .filter_map(|l| l.split_once(' '))
        .filter(|(_, url)| !url.starts_with("file://"))
        .filter_map(|(size, url)| Some((url.rsplit('/').next()?.to_string(), size.parse().ok()?)))
        .collect();
    if files.is_empty() {
        return Ok(());
    }
    let names: Vec<&str> = files.iter().filter_map(|f| f.0.rsplitn(4, '-').nth(3)).collect();
    let label = format!("Downloading {}", if names.len() <= 3 { names.join(", ") } else { count(files.len(), "package") });
    let size = files.iter().map(|f| f.1).sum::<u64>() as f64;
    report::progress(&match size < 1048576.0 {
        true => format!("{label}: {:.0} KiB", (size / 1024.0).ceil()),
        false => format!("{label}: {:.1} MiB", size / 1048576.0),
    });
    report::download(&label, STAGING, Some(files));
    let result = run_cmd(pacman(&["-Sw", "--noconfirm", "--ask=4", "--config", CONF]).args(args));
    report::download("", "", None);
    result
}

/// A command of the builder user in the sandbox, which can write only to the
/// build directory.
fn as_builder(args: &[&str]) -> Command {
    let mut c = cmd(args[0], &args[1..]);
    c.env("HOME", BUILD).env("LC_ALL", "C").current_dir(BUILD);
    sandboxed(c, Some("pacnix"), &[BUILD])
}

/// Runs a command in a sandbox: a mount namespace in which the file system is
/// read-only apart from the writable paths, the home directories are empty,
/// and /tmp, /var/tmp and /dev/shm are private. A PKGBUILD runs on the host in
/// it, as the builder user, for example to download its sources, and cannot
/// get root.
fn sandboxed(c: Command, user: Option<&str>, writable: &[&str]) -> Command {
    const SCRIPT: &str = r#"set -e
user=$1; shift
rw="/proc /dev /sys/fs/cgroup"
while [ "$1" != -- ]; do rw="$rw $1"; shift; done; shift
for d in $rw; do mount --rbind "$d" "$d"; done
mount -o remount,bind,ro=recursive /
for d in $rw; do mount -o remount,bind,rw=recursive "$d"; done
for d in /home /root /tmp /var/tmp /dev/shm; do mount -t tmpfs -o mode=1777 tmpfs "$d"; done
if [ -n "$user" ]; then exec setpriv --reuid="$user" --regid="$user" --clear-groups --no-new-privs -- "$@"; fi
exec "$@""#;
    let mut s = cmd("unshare", &["--mount", "--propagation", "private", "--", "sh", "-c", SCRIPT, "sh", user.unwrap_or("")]);
    s.args(writable).arg("--").arg(c.get_program()).args(c.get_args());
    if let Some(d) = c.get_current_dir() {
        s.current_dir(d);
    }
    for (k, v) in c.get_envs() {
        match v {
            Some(v) => s.env(k, v),
            None => s.env_remove(k),
        };
    }
    s
}

/// The makepkg configuration of the host. The clean chroot copies it at each update.
const MAKEPKG_CONF: &str = "/etc/makepkg.conf";

const AUR_ARGS: &[&str] = &[
    "--chroot",
    "--database=pacnix-local",
    "--root=/var/lib/pacnix/repo",
    "--pacman-conf=/run/pacnix/pacman.conf",
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
        run_cmd(gpg(&["--passphrase", "", "--quick-gen-key", &format!("pacnix-local of {}", host.trim()), "ed25519", "sign", "never"]))?;
    }
    let key = fingerprints(&output(gpg(&["--with-colons", "--list-secret-keys"]))?).into_iter().next().context("the signing key of the local repository has no fingerprint")?;
    if !Path::new(KEY).exists() {
        run_cmd(gpg(&["--armor", "--output", KEY, "--export", &key]))?;
        report::last(format!("• New key of the local repository, for packages.serverKey of other hosts: {key}"));
    }
    Ok(key)
}

/// The package files and signatures in the local repository that its signed
/// database does not name. The other files of the repository are not packages.
fn stale(db: &[Info]) -> Vec<String> {
    let named: BTreeSet<&str> = db.iter().map(|i| i.filename.as_str()).collect();
    let files = std::fs::read_dir(REPO).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned());
    files.filter(|f| f.contains(".pkg.tar") && !named.contains(f.strip_suffix(".sig").unwrap_or(f))).collect()
}

/// Signs packages and adds them to the local repository, which then has the
/// new versions only.
fn repo_add(files: &[std::path::PathBuf]) -> Result<()> {
    let key = signing_key()?;
    for f in files {
        run_cmd(gpg(&["--yes", "--local-user", &key, "--detach-sign"]).arg(f))?;
    }
    let mut c = cmd("repo-add", &["-q", "-R", "-s", "-k", &key, &format!("{REPO}/pacnix-local.db.tar.gz")]);
    run_cmd(c.args(files).env("GNUPGHOME", GNUPG))
}

/// Trusts the signing key of packages.server with pacman-key, after a check of
/// its fingerprint against packages.serverKey.
fn trust_server(g: &Generation) -> Result<()> {
    let (Some(url), Some(key)) = (&g.m.packages.server, &g.m.packages.server_key) else { return Ok(()) };
    let key = key.replace(' ', "").to_uppercase();
    if run_cmd(cmd("pacman-key", &["--list-keys", &key])).is_ok() {
        return Ok(());
    }
    report::progress("Trusting the signing key of the server");
    let url = format!("{}/pacnix-local.asc", url.trim_end_matches('/'));
    let file = format!("/run/pacnix/server-{}.asc", std::process::id());
    std::fs::create_dir_all("/run/pacnix")?;
    output(cmd("curl", &["-fsSL", "--retry", "3", "-o", &file, &url])).with_context(|| format!("download {url}"))?;
    let found = fingerprints(&output(cmd("gpg", &["--homedir", "/etc/pacman.d/gnupg", "--no-permission-warning", "--batch", "--with-colons", "--show-keys", &file]))?);
    if found.first() != Some(&key) {
        let _ = std::fs::remove_file(&file);
        bail!("the key in {url} has the fingerprint {}, but packages.serverKey is {key}.", found.first().map_or("none", |f| f.as_str()));
    }
    run_cmd(cmd("pacman-key", &["--add", &file]))?;
    let _ = std::fs::remove_file(&file);
    run_cmd(cmd("pacman-key", &["--lsign-key", &key]))
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
    std::fs::create_dir_all(STAGING)?;
    let root = Path::new("/var/lib/aurbuild/x86_64/root");
    if root.join(".arch-chroot").exists() {
        report::progress("Updating the build chroot");
        return run_cmd(chroot(&["--update"]));
    }
    report::progress("Creating the build chroot");
    if root.exists() {
        run_cmd(cmd("rm", &["-rf", "--one-file-system"]).arg(root))?;
    }
    // mkarchroot takes the cache directories from /etc/pacman.conf, thus it
    // sees the configuration of pacnix there, in a mount namespace of its own.
    let create = chroot(&["--create"]);
    let mut c = cmd("unshare", &["--mount", "--propagation", "private", "--", "sh", "-c", r#"mount --bind "$1" /etc/pacman.conf && shift && exec "$@""#, "sh", CONF]);
    c.arg(create.get_program()).args(create.get_args()).envs(create.get_envs().filter_map(|(k, v)| Some((k, v?))));
    run_cmd(c)
}

/// The packages to build, in build order, with a flag for local packages, the
/// new records of the AUR packages and the changes that need a confirmation.
#[derive(Default)]
pub struct Builds {
    pub queue: Vec<(String, bool)>,
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
    if !is_builder(g) || (plan.build.is_empty() && plan.local.is_empty()) {
        return Ok(Builds::default());
    }
    ensure_repo()?;
    write_confs(g)?;
    sync_pacnix_db(g)?;
    let mut aur: Vec<String> = plan.build.clone();
    let mut dirs = Vec::new();
    let sync = sync_names()?;
    let dirs_of = local_dirs(g);
    let in_repo: BTreeSet<String> = repo_db(g)?.into_iter().map(|i| i.name).collect();
    // A local package that a local package to build needs, and that the local
    // repository does not have, builds too. A local package builds after the
    // local packages that it needs.
    let mut names = plan.local.clone();
    let mut needs: HashMap<String, Vec<String>> = HashMap::new();
    let mut i = 0;
    while i < names.len() {
        let name = names[i].clone();
        i += 1;
        let dir = format!("{BUILD}/local/{name}");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(format!("{BUILD}/local"))?;
        run_cmd(cmd("cp", &["-rT", &dirs_of[&name], &dir]))?;
        run_cmd(cmd("chown", &["-R", "pacnix:pacnix", &format!("{BUILD}/local")]))?;
        run_cmd(cmd("chmod", &["-R", "u+w", &dir]))?;
        let deps = srcinfo(&dir)?;
        // Dependencies that no sync package satisfies, by name or by provides, come from the AUR.
        let satisfied = |d: &String| {
            sync.contains(d) || dirs_of.contains_key(d) || g.m.packages.providers.contains_key(d) || pacman(&["-Sddp", "--config", CONF, d]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success())
        };
        for d in &deps {
            if dirs_of.contains_key(d) && !in_repo.contains(d) && !names.contains(d) {
                names.push(d.clone());
            }
        }
        needs.insert(name, deps.iter().filter(|d| dirs_of.contains_key(*d)).cloned().collect());
        aur.extend(deps.into_iter().filter(|d| !satisfied(d)));
    }
    fn visit(n: &str, needs: &HashMap<String, Vec<String>>, seen: &mut BTreeSet<String>, order: &mut Vec<String>) {
        if needs.contains_key(n) && seen.insert(n.to_string()) {
            needs[n].iter().for_each(|d| visit(d, needs, seen, order));
            order.push(format!("{BUILD}/local/{n}"));
        }
    }
    let mut seen = BTreeSet::new();
    names.iter().for_each(|n| visit(n, &needs, &mut seen, &mut dirs));
    aur.sort();
    aur.dedup();
    let mut queue = Vec::new();
    if !aur.is_empty() {
        report::progress(&format!("Downloading {} and their AUR dependencies", count(aur.len(), "AUR package")));
        let fetched = fetch(as_builder(&["aur", "sync", "--no-view", "--no-build"]).args(AUR_ARGS).args(ignored(g)).args(&aur))?;
        queue.extend(fetched.into_iter().map(|d| (d, false)));
    }
    queue.extend(dirs.into_iter().map(|d| (d, true)));
    Builds::of(queue, &[])
}

/// Builds AUR and local packages into the local repository and drops packages
/// that no declaration needs. Returns the packages that did not build. A local
/// package that did not build keeps its old record, thus it builds again at
/// the next apply. Then the package files and signatures that the signed
/// database does not name go: the database names the current version of each
/// built and served package.
fn build(g: &Generation, plan: &Plan, builds: Builds) -> Result<BTreeSet<String>> {
    let failed = build_dirs(g, builds)?;
    let dirs_of = local_dirs(g);
    if !plan.local.is_empty() {
        let mut sources = local_sources();
        for name in plan.local.iter().filter(|n| !failed.contains(*n)) {
            sources.insert(name.clone(), dirs_of[name].clone());
        }
        std::fs::write(SOURCES, serde_json::to_string_pretty(&sources)?)?;
    }
    if !plan.drop.is_empty() {
        let key = signing_key()?;
        run_cmd(cmd("repo-remove", &["-q", "-s", "-k", &key, &format!("{REPO}/pacnix-local.db.tar.gz")]).args(&plan.drop).env("GNUPGHOME", GNUPG))?;
    }
    if plan.stale || !plan.drop.is_empty() {
        for f in stale(&repo_db(g)?) {
            std::fs::remove_file(format!("{REPO}/{f}"))?;
        }
    }
    Ok(failed)
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
/// the builder user cannot change. A build that fails is an isolated step: the
/// packages that need it do not build, and the others build. Returns the
/// names of the packages that did not build.
fn build_dirs(g: &Generation, mut builds: Builds) -> Result<BTreeSet<String>> {
    let out = format!("{BUILD}/out");
    let built: HashMap<String, String> = repo_db(g)?.into_iter().map(|i| (i.name, i.version)).collect();
    let count = builds.queue.len();
    let mut failed = BTreeSet::new();
    for (i, (dir, local)) in builds.queue.iter().enumerate() {
        let name = base(dir);
        let step = format!("build {} package {name}", if *local { "local" } else { "AUR" });
        report::progress(&format!("Building {} package {}/{count}: {name}", if *local { "local" } else { "AUR" }, i + 1));
        let (names, version, deps) = srcinfo_file(dir);
        if let Some(d) = deps.iter().find(|d| failed.contains(*d)) {
            report::failure(&step, &format!("it needs {d}, which did not build."));
            failed.extend(names);
            FAILED_BASES.lock().unwrap().insert(name);
            continue;
        }
        let mut cache = build_cache();
        if cache.insert(name.clone()) {
            save_build_cache(&cache)?;
        }
        let record = builds.records.remove(&name);
        let head = match local {
            true => String::new(),
            false => output(as_builder(&["git", "-C", dir, "rev-parse", "HEAD"]))?.trim().to_string(),
        };
        if *local || !names.iter().all(|n| built.get(n) == Some(&version)) {
            let _ = std::fs::remove_dir_all(&out);
            run_cmd(as_builder(&["mkdir", &out]))?;
            // makechrootpkg builds in a copy of the chroot that it removes after the build.
            let mut c = chroot(&["--build", "--temp"]);
            c.current_dir(dir).env("PKGDEST", &out);
            // The sources stay until the install, thus a new attempt continues a large download.
            let sources = format!("{BUILD}/sources/{name}");
            if *local {
                run_cmd(as_builder(&["mkdir", "-p", &sources]))?;
                c.env("SRCDEST", &sources);
            }
            // makechrootpkg downloads the sources on the host as the builder user, and
            // systemd-nspawn locks the chroot in /run.
            let writable = [BUILD, REPO, "/var/lib/aurbuild", STAGING, "/run"];
            let ok = isolated(&step, || run_cmd(sandboxed(c, None, &writable)));
            // No process of the builder user stays to change the packages.
            let _ = cmd("pkill", &["-KILL", "-u", "pacnix"]).status();
            if !ok? {
                failed.extend(names);
                FAILED_BASES.lock().unwrap().insert(name);
                continue;
            }
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
    Ok(failed)
}

/// The build directories that can have downloaded sources and build files, by
/// package base or local package name. Without a record, the directories of an
/// earlier pacnix version.
fn build_cache() -> BTreeSet<String> {
    match std::fs::read(CACHE) {
        Ok(b) => serde_json::from_slice(&b).unwrap_or_default(),
        Err(_) => [AURDEST.to_string(), format!("{BUILD}/local"), format!("{BUILD}/sources")]
            .iter()
            .flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
    }
}

fn save_build_cache(names: &BTreeSet<String>) -> Result<()> {
    let tmp = format!("{CACHE}.tmp");
    std::fs::write(&tmp, serde_json::to_vec(names)?)?;
    Ok(std::fs::rename(tmp, CACHE)?)
}

/// Removes the downloaded sources and the build files of the builds that are in
/// the local repository and installed, except of the names in
/// packages.keepBuildCache. The sources of a build that failed in this run stay
/// for the next attempt. The git checkout of an AUR package stays for the review
/// of its next changes. Each name is one directory entry: no path goes outside
/// the build directory, and links are removed, not followed.
pub fn discard(g: &Generation) -> Result<()> {
    let before = build_cache();
    let (failed, keep) = (FAILED_BASES.lock().unwrap(), &g.m.packages.keep_build_cache);
    let mut after = before.clone();
    for name in before.iter().filter(|n| !failed.contains(*n) && !keep.contains(n)) {
        after.remove(name);
        if name.is_empty() || name.starts_with('.') || name.contains('/') {
            continue;
        }
        let checkout = format!("{AURDEST}/{name}");
        if std::fs::symlink_metadata(&checkout).is_ok_and(|m| m.is_dir()) && Path::new(&checkout).join(".git").is_dir() {
            run_cmd(as_builder(&["git", "-C", &checkout, "clean", "-ffdxq"]))?;
        }
        for dir in [format!("{BUILD}/local/{name}"), format!("{BUILD}/sources/{name}")] {
            match std::fs::remove_dir_all(&dir) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e).context(dir),
                _ => {}
            }
        }
    }
    if after != before || (!after.is_empty() && !Path::new(CACHE).exists()) {
        save_build_cache(&after)?;
    }
    Ok(())
}

/// The package names, the version and the dependencies, also of the build, in
/// the .SRCINFO of a package.
fn srcinfo_file(dir: &str) -> (Vec<String>, String, Vec<String>) {
    let text = std::fs::read_to_string(Path::new(dir).join(".SRCINFO")).unwrap_or_default();
    let (mut names, mut fields, mut deps) = (Vec::new(), BTreeMap::new(), Vec::new());
    for (k, v) in text.lines().filter_map(|l| l.trim().split_once(" = ")) {
        match k.split('_').next().unwrap_or(k) {
            "pkgname" => names.push(v.to_string()),
            "depends" | "makedepends" | "checkdepends" => deps.push(dep_name(v).to_string()),
            "epoch" | "pkgver" | "pkgrel" => {
                fields.entry(k).or_insert(v);
            }
            _ => {}
        }
    }
    let epoch = fields.get("epoch").filter(|e| **e != "0").map(|e| format!("{e}:")).unwrap_or_default();
    (names, format!("{epoch}{}-{}", fields.get("pkgver").unwrap_or(&""), fields.get("pkgrel").unwrap_or(&"")), deps)
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
            bail!("pacman is running.");
        }
        esay!("• Removing the stale pacman lock of an interrupted transaction");
        std::fs::remove_file(lock)?;
    }
    for e in std::fs::read_dir("/var/lib/pacman/local")?.flatten() {
        if e.path().is_dir() && !(e.path().join("desc").exists() && e.path().join("files").exists()) {
            esay!("• Removing the incomplete database entry {}", e.path().display());
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
    std::fs::create_dir_all(STAGING)?;
    let mut c = pacman(args);
    // The lists of packages and optional dependencies stay out of the output.
    // The summary shows the install notes and the .pacnew files.
    let shown = |l: &str| (l.starts_with("error:") || l.starts_with("warning:")) && !l.contains(" installed as ") && !l.ends_with("-- skipping");
    let (status, lines) = forward(&mut c, usize::MAX, true, shown)?;
    if status.success() {
        return Ok(());
    }
    let files: Vec<&str> = lines.iter().filter_map(|l| l.split_once(": ")?.1.strip_suffix(" exists in filesystem")).collect();
    let owned = crate::files::owners(&files);
    let conflicts: Vec<&str> = files.into_iter().filter(|f| !owned.contains_key(*f)).collect();
    if conflicts.is_empty() {
        return Err(failed(&c, status, &lines));
    }
    report::progress(&format!("Replacing {} left by an interrupted transaction", count(conflicts.len(), "file")));
    let mut c = pacman(args);
    for f in conflicts {
        c.args(["--overwrite", f]);
    }
    let (status, lines) = forward(&mut c, 20, true, shown)?;
    if !status.success() {
        return Err(failed(&c, status, &lines));
    }
    Ok(())
}

/// Builds packages and downloads the packages to install, before units stop,
/// so that a change of the network service still downloads, and the
/// transaction then installs from the cache. Without packages to install,
/// nothing downloads. Returns the packages that the transaction skips: the
/// packages that did not build and the packages that need them.
pub fn prepare(g: &Generation, plan: &Plan, builds: Builds) -> Result<BTreeSet<String>> {
    let mut failed = BTreeSet::new();
    write_confs(g)?;
    // pacnix 0.11 kept its pacman configurations in /var/lib/pacnix.
    let _ = std::fs::remove_file("/var/lib/pacnix/pacman.conf");
    let _ = std::fs::remove_file("/var/lib/pacnix/pacman-local.conf");
    let _ = std::fs::remove_file("/var/lib/pacnix/server.db");
    trust_server(g)?;
    if is_builder(g) && (!g.m.packages.aur.is_empty() || !g.m.packages.local.is_empty() || has_local(g)) {
        ensure_repo()?;
        write_confs(g)?;
        // The build asks pacman for the dependencies, which needs the
        // pacnix database also on a new host.
        sync_pacnix_db(g)?;
        if !builds.queue.is_empty() {
            ensure_chroot()?;
        }
        failed = build(g, plan, builds)?;
    }
    if has_local(g) {
        sync_pacnix_db(g)?;
    }
    let skip = plan.skipped(failed);
    let targets = plan.targets(&skip);
    if !targets.is_empty() {
        download(&targets)?;
    }
    Ok(skip)
}

/// Installs, marks and removes packages. With skipped packages, the removals
/// wait: a skipped package can be the replacement of a package that leaves,
/// for example of a network service.
pub fn apply(plan: &Plan, skip: &BTreeSet<String>) -> Result<()> {
    // The record has the packages of this apply before the transaction, so that an interruption does not lose them.
    let mut list: BTreeSet<&String> = plan.made.iter().chain(&plan.install).chain(&plan.explicit).collect();
    list.retain(|n| !plan.deps.contains(*n) || plan.made.contains(*n));
    std::fs::write(MADE, serde_json::to_vec(&list)?)?;
    let targets = plan.targets(skip);
    let installs = !targets.is_empty() || !plan.archive.is_empty();
    if !targets.is_empty() {
        report::progress(&format!("Installing {}: {}", count(targets.len(), "package"), report::items(&targets)));
        let mut args = vec!["-S", "--needed", "--noconfirm", "--ask=4", "--config", CONF];
        args.extend(targets);
        transaction(&args)?;
    }
    if !plan.archive.is_empty() {
        report::progress(&format!("Installing {} from the Arch Linux Archive: {}", count(plan.archive.len(), "package"), report::items(&plan.archive)));
        let mut args = vec!["-U", "--noconfirm", "--ask=4", "--config", CONF];
        args.extend(plan.archive.iter().map(String::as_str));
        // pacman downloads the files of -U first and stops before a change when a download fails.
        transaction(&args)?;
    }
    if !plan.explicit.is_empty() {
        run_cmd(pacman(&["-D", "--asexplicit"]).args(&plan.explicit))?;
    }
    if !skip.is_empty() {
        if !plan.remove.is_empty() || !plan.deps.is_empty() {
            let step = "remove the packages that the configuration does not declare";
            report::failure(step, "it waits until each declared package builds, because a package that did not build can replace a package that leaves.");
        }
        return save_made(plan);
    }
    // An install can remove an old package that conflicts with its replacement.
    let installed = if installs && (!plan.deps.is_empty() || !plan.remove.is_empty()) {
        Some(output(pacman(&["-Qq"]))?)
    } else {
        None
    };
    let present: BTreeSet<&str> = installed.as_deref().unwrap_or_default().split_whitespace().collect();
    let remains = |name: &&str| installed.is_none() || present.contains(name);
    let deps: Vec<&str> = plan.deps.iter().map(String::as_str).filter(remains).collect();
    if !deps.is_empty() {
        run_cmd(pacman(&["-D", "--asdeps"]).args(deps))?;
    }
    let remove: Vec<&str> = plan.remove.iter().map(String::as_str).filter(remains).collect();
    if !remove.is_empty() {
        report::progress(&format!("Removing {}: {}", count(remove.len(), "package"), report::items(&remove)));
        transaction(&[&["-Rns", "--noconfirm"][..], &remove].concat())?;
    }
    let orphans = output_any(&mut pacman(&["-Qqdtt"]))?;
    let orphans: Vec<&str> = orphans.split_whitespace().filter(|n| !plan.orphans.contains(*n)).collect();
    if !orphans.is_empty() {
        report::progress(&format!("Removing {} that no declared package needs: {}", count(orphans.len(), "package"), report::items(&orphans)));
        transaction(&[&["-Rns", "--noconfirm"][..], &orphans].concat())?;
    }
    save_made(plan)
}

/// The AUR packages that upgrade, before any change, with their records and the
/// changes that need a confirmation. A host without a local repository has
/// none yet: the apply after the upgrade builds them.
pub fn upgrade_builds(g: &Generation) -> Result<Builds> {
    if !is_builder(g) || !Path::new(REPO).join("pacnix-local.db.tar.gz").exists() {
        return Ok(Builds::default());
    }
    write_confs(g)?;
    // aur sync keeps the pinned AUR packages, the dependencies with a chosen
    // provider and the local packages, which can have the name of an AUR package.
    let (local, aur) = (local_dirs(g), built_aur(g));
    let built: Vec<String> = local_sources().into_keys().filter(|n| !aur.contains(n)).collect();
    let ignore: Vec<&str> = g.m.packages.providers.keys().chain(g.m.packages.versions.keys()).chain(local.keys()).chain(&built).map(String::as_str).collect();
    let mut c = as_builder(&["aur", "sync", "--no-view", "--upgrades", "--no-build"]);
    if !ignore.is_empty() {
        c.args(["--ignore", &ignore.join(",")]);
    }
    // aur sync checks the packages of the repository;
    // aur vercmp checks the installed packages that left it, at the built version, in the same way.
    let inst = installed()?;
    let gone: String = pruned()
        .into_iter()
        .filter(|(n, v)| inst.get(n).is_some_and(|i| &i.version == v) && !local.contains_key(n) && !built.contains(n) && !ignore.contains(&n.as_str()))
        .map(|(n, v)| format!("{n}\t{v}\n"))
        .collect();
    if !gone.is_empty() {
        use std::io::Write;
        let mut v = cmd("aur", &["vercmp", "--quiet"]);
        let mut child = v.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn()?;
        child.stdin.take().context("aur vercmp")?.write_all(gone.as_bytes())?;
        let out = child.wait_with_output()?;
        if !out.status.success() {
            return Err(failed(&v, out.status, &String::from_utf8_lossy(&out.stderr).lines().map(String::from).collect::<Vec<_>>()));
        }
        c.args(String::from_utf8_lossy(&out.stdout).split_whitespace());
    }
    report::progress("Downloading AUR package updates");
    let dirs = fetch(c.args(AUR_ARGS))?;
    Builds::of(dirs.into_iter().map(|d| (d, false)).collect(), &[])
}

/// The modification times of the local package database and of the sync
/// databases: a transaction or a refresh changes them.
pub fn databases() -> Vec<Option<std::time::SystemTime>> {
    let mut paths: Vec<_> = std::fs::read_dir("/var/lib/pacman/sync").into_iter().flatten().flatten().map(|e| e.path()).collect();
    paths.sort();
    paths.push(Path::new("/var/lib/pacman/local").to_path_buf());
    paths.iter().map(|p| p.metadata().and_then(|m| m.modified()).ok()).collect()
}

/// Full upgrade: all sync databases, the system packages, then changed AUR packages.
pub fn upgrade(g: &Generation, builds: Builds) -> Result<()> {
    write_confs(g)?;
    trust_server(g)?;
    recover_lock()?;
    let pins = pinned(g);
    let pins: Vec<&str> = pins.iter().map(String::as_str).collect();
    // After a handoff, the system upgrade is done, also for the next attempts.
    if !crate::handoff::resumed() {
        let stage = report::stage("Refreshing package databases");
        run_cmd(pacman(&["-Sy", "--config", CONF]))?;
        drop(stage);
        let newer = newer()?;
        if newer.is_empty() && pins.is_empty() {
            say!("• System upgrade: the packages are up to date.");
        } else {
            report::progress(&format!("Upgrading the system: {}", crate::named("package", "to upgrade", &newer)));
            download(&[&["-u"][..], &pins].concat())?;
            report::step("system upgrade");
            transaction(&[&["-Su", "--noconfirm", "--config", CONF][..], &pins].concat())?;
            crate::handoff::check()?;
        }
    }
    if !builds.queue.is_empty() {
        ensure_chroot()?;
        // An AUR package that did not build stays at its installed version.
        build_dirs(g, builds)?;
        sync_pacnix_db(g)?;
        transaction(&[&["-Su", "--noconfirm", "--config", CONF][..], &pins].concat())?;
    }
    Ok(())
}

/// Deletes the package files, signatures and partial downloads of pacnix
/// after an apply without a failure.
pub fn clean_staging() -> Result<()> {
    let dir = match std::fs::read_dir(STAGING) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        dir => dir.context(STAGING)?,
    };
    for e in dir {
        let e = e?;
        match e.file_type()?.is_dir() {
            true => std::fs::remove_dir_all(e.path())?,
            false => std::fs::remove_file(e.path())?,
        }
    }
    Ok(())
}

/// True when the host serves AUR or local packages to other hosts.
fn serves(g: &Generation) -> bool {
    !g.m.packages.serve.aur.is_empty() || !g.m.packages.serve.local.is_empty()
}

fn pruned() -> BTreeMap<String, String> {
    std::fs::read(PRUNED).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Removes the packages from the local repository after an apply without a
/// failure, on a host that serves no packages: pacman needs the file of a
/// package only to install it, and a package that is not there builds again.
/// The record keeps the version of each installed package, which the plan
/// and the upgrade take as the built version. These packages stay: the pinned
/// packages with their dependencies, because the AUR does not keep old
/// versions, and the packages that own a declared file, for its restore. The
/// database names only the files that remain.
pub fn prune(g: &Generation) -> Result<()> {
    if !is_builder(g) || serves(g) || !Path::new(REPO).join("pacnix-local.db.tar.gz").exists() {
        return Ok(());
    }
    let repo = repo_db(g)?;
    if repo.is_empty() {
        return Ok(());
    }
    let by_name: HashMap<&str, &Info> = repo.iter().map(|i| (i.name.as_str(), i)).collect();
    let mut keep = BTreeSet::new();
    let mut queue: Vec<String> = g.m.packages.versions.keys().cloned().collect();
    while let Some(n) = queue.pop() {
        if let Some(i) = by_name.get(n.as_str()).filter(|_| keep.insert(n.clone())) {
            queue.extend(i.depends.iter().map(|d| dep_name(d).to_string()));
        }
    }
    keep.extend(crate::files::owners(&g.m.files.keys().map(String::as_str).collect::<Vec<_>>()).into_values());
    let gone: Vec<&Info> = repo.iter().filter(|i| !keep.contains(&i.name) && !i.name.strip_suffix("-debug").is_some_and(|b| keep.contains(b))).collect();
    if gone.is_empty() {
        return Ok(());
    }
    let inst = installed()?;
    let mut record = pruned();
    record.retain(|n, v| inst.get(n).is_some_and(|i| &i.version == v));
    record.extend(gone.iter().filter(|i| inst.get(&i.name).is_some_and(|x| x.version == i.version)).map(|i| (i.name.clone(), i.version.clone())));
    let tmp = format!("{PRUNED}.tmp");
    std::fs::write(&tmp, serde_json::to_vec(&record)?)?;
    std::fs::rename(&tmp, PRUNED)?;
    let key = signing_key()?;
    run_cmd(cmd("repo-remove", &["-q", "-s", "-k", &key, &format!("{REPO}/pacnix-local.db.tar.gz")]).args(gone.iter().map(|i| &i.name)).env("GNUPGHOME", GNUPG))?;
    for f in stale(&repo_db(g)?) {
        std::fs::remove_file(format!("{REPO}/{f}"))?;
    }
    sync_pacnix_db(g)
}

/// The packages that pacnix uninstall removes: pacnix, aurutils and nix, less
/// the ones that a different package needs, and the dependencies that only
/// they need.
pub fn uninstall_plan() -> Result<Vec<String>> {
    let inst = installed()?;
    let mut targets: Vec<&str> = ["pacnix", "pacnix-keyring", "aurutils", "nix"].into_iter().filter(|n| inst.contains_key(*n)).collect();
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
        run_cmd(cmd("systemctl", &["--quiet", "disable", "--now"]).args(&units))?;
    }
    report::progress(&format!("Removing {}", count(remove.len(), "package")));
    transaction(&[&["-Rns", "--noconfirm"][..], &remove.iter().map(String::as_str).collect::<Vec<_>>()].concat())
}
