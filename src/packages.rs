use crate::generation::{Generation, STATE};
use crate::{Ctx, cmd, confirm, output, output_any, run_cmd};
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::process::Command;

const REPO: &str = "/var/lib/pacnix/repo";
const BUILD: &str = "/var/lib/pacnix/build";
const CONF: &str = "/var/lib/pacnix/pacman.conf";
const LOCAL_CONF: &str = "/var/lib/pacnix/pacman-local.conf";
const SOURCES: &str = "/var/lib/pacnix/local-sources.json";
const PROTECTED: &[&str] = &["base", "pacman", "nix", "pacnix"];

#[derive(Default)]
pub struct Plan {
    notes: Vec<String>,
    unknown: Vec<String>,
    build: Vec<String>,
    local: Vec<String>,
    drop: Vec<String>,
    install: Vec<String>,
    explicit: Vec<String>,
    deps: Vec<String>,
    remove: Vec<String>,
    protected: Vec<String>,
    declared: usize,
}

impl Plan {
    pub fn print(&self, l: &mut Vec<String>) {
        let mut add = |prefix: &str, list: &[String]| l.extend(list.iter().map(|p| format!("{prefix} {p}")));
        add("note:", &self.notes);
        add("unknown package", &self.unknown);
        add("build AUR package", &self.build);
        add("build local package", &self.local);
        add("drop from local repository", &self.drop);
        add("install package", &self.install);
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
    explicit: bool,
    filename: String,
}

fn dep_name(d: &str) -> &str {
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

/// Package names in the sync databases of every repository except pacnix-local.
fn sync_names() -> Result<BTreeSet<String>> {
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
/// the first boot.
fn defer_builds(g: &Generation) -> bool {
    is_builder(g) && !booted()
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
    let section = format!("\n[pacnix-local]\nSigLevel = Optional TrustAll\nServer = {server}\n");
    let system = std::fs::read_to_string("/etc/pacman.conf")?;
    std::fs::create_dir_all(STATE)?;
    std::fs::write(CONF, format!("{system}{section}"))?;
    std::fs::write(LOCAL_CONF, format!("[options]\nArchitecture = auto\n{section}"))?;
    Ok(())
}

/// Refreshes the database of the pacnix-local repository only. The sync databases of
/// the Arch repositories stay as they are.
fn sync_pacnix_db() -> Result<()> {
    recover_lock()?;
    run_cmd(pacman(&["-Sy", "--config", LOCAL_CONF]).stdout(std::process::Stdio::null()))
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
    let declared: BTreeSet<String> = p.repo.iter().chain(&p.aur).chain(&local_names).cloned().collect();
    plan.declared = declared.len();

    plan.unknown.extend(p.repo.iter().filter(|n| !sync.contains(*n)).cloned());
    let locals = local_dirs(g);
    let mut built_aur = built_aur(g);
    built_aur.extend(p.providers.values().filter(|v| !sync.contains(*v) && !locals.contains_key(*v)).cloned());
    built_aur.sort();
    built_aur.dedup();
    let missing_aur: Vec<String> = built_aur.iter().filter(|n| !repo_names.contains(*n)).cloned().collect();
    let mut table = Vec::new();
    if defer_builds(g) {
        let all: Vec<String> = built_aur.iter().cloned().chain(local_dirs(g).into_keys()).collect();
        plan.notes.push(format!("the first boot builds and installs: {}", all.join(" ")));
    } else if is_builder(g) && !has_aurutils() {
        plan.notes.push("AUR names and dependencies are checked after apply builds aurutils".to_string());
        plan.build = missing_aur.clone();
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
    if !from_aur.is_empty() && is_builder(g) && has_aurutils() {
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
    if !plan.protected.is_empty() && !ctx.opts.allow_protected_removal {
        bail!(
            "the plan removes protected packages: {}. Declare them, or give --allow-protected-removal",
            plan.protected.join(", ")
        );
    }
    if !plan.remove.is_empty() {
        confirm(&format!("Remove {} packages?", plan.remove.len()), &ctx.opts)?;
    }
    Ok(())
}

fn as_builder(args: &[&str]) -> Command {
    let mut c = cmd("runuser", &["-u", "pacnix", "--", "env", &format!("HOME={BUILD}"), "AUR_PACMAN_AUTH=sudo", "LC_ALL=C"]);
    c.args(args).current_dir(BUILD);
    c
}

const AUR_ARGS: &[&str] = &[
    "--chroot",
    "--database=pacnix-local",
    "--root=/var/lib/pacnix/repo",
    "--pacman-conf=/var/lib/pacnix/pacman.conf",
    "--makepkg-conf=/usr/share/devtools/makepkg.conf.d/x86_64.conf",
    "--no-confirm",
    "--no-sync",
];

fn has_aurutils() -> bool {
    Path::new("/usr/bin/aur").exists()
}

/// Creates the local repository if it does not exist.
fn ensure_repo() -> Result<()> {
    if !Path::new(&format!("{REPO}/pacnix-local.db.tar.gz")).exists() {
        run_cmd(as_builder(&["repo-add", "-q", &format!("{REPO}/pacnix-local.db.tar.gz")]))?;
    }
    Ok(())
}

/// True on a new builder host, which has no aurutils yet.
pub fn needs_bootstrap(g: &Generation) -> bool {
    !defer_builds(g) && is_builder(g) && !has_aurutils() && g.m.packages.aur.contains(&"aurutils".to_string())
}

/// Builds aurutils from the AUR with makepkg into the local repository and
/// installs it, because aurutils itself comes from the AUR and the plan needs
/// it for the AUR graph.
pub fn bootstrap() -> Result<()> {
    println!("pacnix: build aurutils from the AUR, because the plan needs it");
    ensure_repo()?;
    let dir = format!("{BUILD}/aurutils");
    let _ = std::fs::remove_dir_all(&dir);
    run_cmd(as_builder(&["git", "clone", "-q", "--depth", "1", "https://aur.archlinux.org/aurutils.git", &dir]))?;
    let (_, deps) = srcinfo(&dir)?;
    transaction(&[&["-S", "--needed", "--asdeps", "--noconfirm"][..], &deps.iter().map(String::as_str).collect::<Vec<_>>()].concat())?;
    run_cmd(as_builder(&["env", &format!("PKGDEST={REPO}"), "makepkg", "--noconfirm"]).current_dir(&dir))?;
    let pkg = output(as_builder(&["env", &format!("PKGDEST={REPO}"), "makepkg", "--packagelist"]).current_dir(&dir))?;
    let pkg = pkg.lines().next().context("makepkg gave no package of aurutils")?.to_string();
    run_cmd(as_builder(&["repo-add", "-q", &format!("{REPO}/pacnix-local.db.tar.gz"), &pkg]))?;
    transaction(&["-U", "--noconfirm", &pkg])
}

fn ensure_chroot() -> Result<()> {
    if !Path::new("/var/lib/aurbuild/x86_64/root").exists() {
        run_cmd(as_builder(&["aur", "chroot", "--create", "--pacman-conf", CONF, "--makepkg-conf", AUR_ARGS[4].trim_start_matches("--makepkg-conf=")]))?;
    }
    Ok(())
}

/// Builds AUR and local packages into the local repository and drops packages
/// that no declaration needs.
fn build(g: &Generation, plan: &Plan) -> Result<()> {
    let mut sources = local_sources();
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
    if !aur.is_empty() {
        run_cmd(as_builder(&["aur", "sync", "--no-view"]).args(AUR_ARGS).args(ignored(g)).args(&aur))?;
    }
    if !dirs.is_empty() {
        let queue = format!("{BUILD}/local.queue");
        std::fs::write(&queue, dirs.join("\n") + "\n")?;
        run_cmd(as_builder(&["aur", "build", "--force", "--arg-file", &queue]).args(AUR_ARGS))?;
        for name in &plan.local {
            sources.insert(name.clone(), dirs_of[name].clone());
        }
        std::fs::write(SOURCES, serde_json::to_string_pretty(&sources)?)?;
    }
    if !plan.drop.is_empty() {
        let files: Vec<String> = repo_db(g)?.into_iter().filter(|i| plan.drop.contains(&i.name)).map(|i| i.filename).collect();
        run_cmd(as_builder(&["repo-remove", "-q", &format!("{REPO}/pacnix-local.db.tar.gz")]).args(&plan.drop))?;
        for f in files {
            let _ = std::fs::remove_file(format!("{REPO}/{f}"));
            let _ = std::fs::remove_file(format!("{REPO}/{f}.sig"));
        }
    }
    Ok(())
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
    let out = pacman(args).stderr(std::process::Stdio::piped()).spawn()?.wait_with_output()?;
    let err = String::from_utf8_lossy(&out.stderr);
    eprint!("{err}");
    if out.status.success() {
        return Ok(());
    }
    let conflicts: Vec<&str> = err
        .lines()
        .filter_map(|l| l.split_once(": ")?.1.strip_suffix(" exists in filesystem"))
        .filter(|f| output_any(&mut pacman(&["-Qqo", f])).is_ok_and(|o| o.trim().is_empty()))
        .collect();
    if conflicts.is_empty() {
        bail!("pacman {} failed", args.join(" "));
    }
    eprintln!("pacnix: replace {} files that no installed package owns, left by an interrupted transaction", conflicts.len());
    let mut c = pacman(args);
    for f in conflicts {
        c.args(["--overwrite", f]);
    }
    run_cmd(&mut c)
}

/// Builds packages and, when units stop before the transaction, downloads the
/// packages to install, so that a change of the network service still
/// downloads. A download costs about 2 s even from the cache, so it is skipped
/// when no unit stops.
pub fn prepare(g: &Generation, plan: &Plan, download: bool) -> Result<()> {
    write_confs(g)?;
    if is_builder(g) && (!g.m.packages.aur.is_empty() || !g.m.packages.local.is_empty() || Path::new(REPO).join("pacnix-local.db.tar.gz").exists()) {
        ensure_repo()?;
        if !defer_builds(g) {
            // The build asks pacman for the dependencies, which needs the
            // pacnix database also on a new host.
            sync_pacnix_db()?;
            ensure_chroot()?;
            build(g, plan)?;
        }
    }
    sync_pacnix_db()?;
    if download && !plan.install.is_empty() {
        run_cmd(pacman(&["-Sw", "--noconfirm", "--ask=4", "--config", CONF]).args(&plan.install).stdout(std::process::Stdio::null()))?;
    }
    Ok(())
}

pub fn apply(plan: &Plan) -> Result<()> {
    if !plan.install.is_empty() {
        let mut args = vec!["-S", "--needed", "--noconfirm", "--ask=4", "--config", CONF];
        args.extend(plan.install.iter().map(String::as_str));
        transaction(&args)?;
    }
    if !plan.explicit.is_empty() {
        run_cmd(pacman(&["-D", "--asexplicit"]).args(&plan.explicit))?;
    }
    if !plan.deps.is_empty() {
        run_cmd(pacman(&["-D", "--asdeps"]).args(&plan.deps))?;
    }
    if !plan.remove.is_empty() {
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

/// Full upgrade: all sync databases, the system packages, then changed AUR packages.
pub fn upgrade(_ctx: &Ctx, g: &Generation) -> Result<()> {
    write_confs(g)?;
    transaction(&["-Syu", "--noconfirm", "--ask=4", "--config", CONF])?;
    if is_builder(g) && Path::new(REPO).join("pacnix-local.db.tar.gz").exists() {
        ensure_repo()?;
        ensure_chroot()?;
        run_cmd(as_builder(&["aur", "chroot", "--update", "--pacman-conf", CONF, "--makepkg-conf", AUR_ARGS[4].trim_start_matches("--makepkg-conf=")]))?;
        run_cmd(as_builder(&["aur", "sync", "--no-view", "--upgrades"]).args(AUR_ARGS).args(ignored(g)))?;
        sync_pacnix_db()?;
        transaction(&["-Su", "--noconfirm", "--ask=4", "--config", CONF])?;
    }
    Ok(())
}
