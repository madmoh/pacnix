use crate::generation::{Generation, build, history};
use crate::{Ctx, cmd, files, output, output_any};
use anyhow::{Context, Result, bail};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{IsTerminal, Write};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// What the removed packages can leave: the pacman hook pkg/removal.hook
/// records it from the files of each package before pacman removes it, and the
/// audit shows the paths that it explains as leftovers.
const REMOVED: &str = "/var/lib/pacnix/removed.json";

/// The evidence of one removed package: its name, programs and desktop IDs;
/// the system and home paths that its tmpfiles rules, units and /etc files
/// give, with a note; and the system users of its sysusers files.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Removed {
    names: BTreeSet<String>,
    paths: BTreeMap<String, String>,
    home: BTreeMap<String, String>,
    users: BTreeMap<u32, String>,
}

type Records = BTreeMap<String, Removed>;

fn records() -> Records {
    std::fs::read(REMOVED).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(records: &Records) -> Result<()> {
    std::fs::create_dir_all(crate::generation::STATE)?;
    std::fs::write(format!("{REMOVED}.new"), serde_json::to_vec(records)?)?;
    std::fs::rename(format!("{REMOVED}.new"), REMOVED).context("save the record of the removed packages")
}

/// The path of a tmpfiles rule that creates it, without specifiers and globs:
/// a type of these letters, with modifiers.
fn rule_path(line: &str) -> Option<&str> {
    let mut w = line.split_whitespace();
    let (t, p) = (w.next()?, w.next()?);
    let creates = t.starts_with(['f', 'd', 'D', 'v', 'q', 'Q', 'p', 'L', 'c', 'b', 'C']) && t[1..].chars().all(|c| "+!-=~^?$".contains(c));
    (creates && p.starts_with('/') && !p.contains(['%', '*', '?', '[', '"', '\'', '\\'])).then(|| p.trim_end_matches('/'))
}

/// The system users that the sysusers files declare, by name.
fn sysusers(text: &str) -> impl Iterator<Item = &str> {
    text.lines().filter_map(|l| {
        let mut w = l.split_whitespace();
        (w.next()?.trim_end_matches('!') == "u").then(|| w.next()).flatten()
    })
}

/// The system users of /etc/passwd: UID and name.
fn passwd() -> BTreeMap<u32, String> {
    let text = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    text.lines().filter_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        Some((f.get(2)?.parse().ok()?, f[0].to_string()))
    }).collect()
}

/// Records what the packages can leave, before pacman removes them.
pub fn record(packages: &[&str]) -> Result<()> {
    if packages.is_empty() {
        return Ok(());
    }
    let mut records = records();
    let users: BTreeMap<String, u32> = passwd().into_iter().map(|(u, n)| (n, u)).collect();
    let list = output(cmd("pacman", &["-Ql"]).args(packages))?;
    for (pkg, path) in list.lines().filter_map(|l| l.split_once(' ')) {
        let r = records.entry(pkg.to_string()).or_default();
        r.names.insert(pkg.to_string());
        if let Some(n) = path.strip_prefix("/usr/bin/").filter(|n| !n.is_empty() && !n.contains('/')) {
            r.names.insert(n.to_string());
        }
        if let Some(d) = path.strip_prefix("/usr/share/applications/").and_then(|d| d.strip_suffix(".desktop")) {
            r.names.insert(d.to_string());
        }
        if path.starts_with("/etc/") && !path.ends_with('/') {
            r.paths.insert(format!("{path}.pacsave"), format!("the saved copy of {path}"));
        }
        let unit = path.strip_prefix("/usr/lib/systemd/system/").map(|u| (u, false)).or_else(|| path.strip_prefix("/usr/lib/systemd/user/").map(|u| (u, true)));
        let reads = path.starts_with("/usr/lib/tmpfiles.d/") || path.starts_with("/usr/lib/sysusers.d/") || unit.is_some_and(|(u, _)| !u.contains('/'));
        let Some(text) = reads.then(|| std::fs::read_to_string(path).ok()).flatten() else { continue };
        if path.starts_with("/usr/lib/tmpfiles.d/") {
            r.paths.extend(text.lines().filter_map(rule_path).map(|p| (p.to_string(), format!("a tmpfiles rule in {path}"))));
        } else if path.starts_with("/usr/lib/sysusers.d/") {
            r.users.extend(sysusers(&text).filter_map(|n| Some((*users.get(n)?, n.to_string()))));
        } else if let Some((unit, user)) = unit {
            // The directories that systemd makes for a unit: systemd.exec(5).
            let dynamic = text.lines().any(|l| l.trim().strip_prefix("DynamicUser=").is_some_and(|v| ["yes", "true", "on", "1"].contains(&v)));
            for (key, value) in text.lines().filter_map(|l| l.trim().split_once('=')) {
                let base = match (key, user) {
                    ("StateDirectory", false) => "/var/lib",
                    ("CacheDirectory", false) => "/var/cache",
                    ("LogsDirectory", false) => "/var/log",
                    ("ConfigurationDirectory", false) => "/etc",
                    ("StateDirectory", true) => ".local/state",
                    ("CacheDirectory", true) => ".cache",
                    ("LogsDirectory", true) => ".local/state/log",
                    ("ConfigurationDirectory", true) => ".config",
                    _ => continue,
                };
                for d in value.split_whitespace().flat_map(|v| v.split(':')).filter(|d| !d.is_empty()) {
                    let note = format!("the {key} of unit {unit}");
                    match user {
                        true => _ = r.home.insert(format!("{base}/{d}"), note),
                        false => {
                            if dynamic && base != "/etc" {
                                r.paths.insert(format!("{base}/private/{d}"), note.clone());
                            }
                            r.paths.insert(format!("{base}/{d}"), note);
                        }
                    }
                }
            }
        }
    }
    save(&records)
}

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

/// The directories of a home directory whose children are named after an application.
const NAMESPACES: &[&str] = &[".config", ".cache", ".local/share", ".local/state", ".local/bin", ".var/app"];
/// The application name of a home path: the name below a directory of
/// NAMESPACES, or the name of a hidden entry of the home directory without its dot.
fn app_name(rel: &str) -> Option<&str> {
    NAMESPACES.iter().find_map(|ns| rel.strip_prefix(ns)?.strip_prefix('/')?.split('/').next()).or_else(|| rel.split('/').next()?.strip_prefix('.'))
}

/// The packaging parts of a package name that a path name does not have.
const PREFIXES: &[&str] = &["gnome-shell-extension-", "python-", "perl-", "lib32-"];
const SUFFIXES: &[&str] = &["-bin", "-git", "-nightly", "-appimage", "-beta"];
/// The file extensions that a path name can add to the name of an application.
const EXTENSIONS: &[&str] = &["desktop", "png", "svg", "json", "conf", "db", "cache", "list", "xml", "ini", "toml", "yaml", "yml", "log", "lock", "sqlite", "txt"];

/// The name in lower case, with only its letters and digits.
fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// The last part of a reverse-DNS ID, for example T3Code of com.t3tools.T3Code.
fn reverse_dns(s: &str) -> Option<&str> {
    let parts: Vec<&str> = s.split('.').collect();
    (parts.len() >= 3 && parts.iter().all(|p| !p.is_empty())).then(|| parts[parts.len() - 1])
}

/// The kinds of the keys of a path name and of the forms of the index.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    /// A key: the whole name. A form: the name of a package, a GNOME Shell
    /// extension, a tool or a feature, which agrees with each key.
    Whole,
    /// The last part of a reverse-DNS ID, which agrees only with the same part.
    Tail,
    /// A key: a part of the name. A form: a program or a desktop ID, which
    /// agrees only with a whole name, because a program name can be a general
    /// word, for example test.
    Part,
}

/// The forms of a name of installed software that a path name can agree with:
/// the name, the name without its packaging parts and the last part of a
/// reverse-DNS ID, each with only letters and digits, of 3 characters or more.
/// The name of a program or a desktop ID gives forms of the kind Part.
fn forms(name: &str, program: bool) -> Vec<(String, Kind)> {
    let mut bare = PREFIXES.iter().find_map(|p| name.strip_prefix(p)).unwrap_or(name);
    while let Some(b) = SUFFIXES.iter().find_map(|s| bare.strip_suffix(s)) {
        bare = b;
    }
    let kind = if program { Kind::Part } else { Kind::Whole };
    let mut forms = vec![(norm(name), kind), (norm(bare), kind)];
    forms.extend(reverse_dns(name).map(|t| (norm(t), Kind::Tail)));
    forms.retain(|f| f.0.len() >= 3);
    forms.dedup();
    forms
}

/// The keys of a path name, in order: the name and the name without its file
/// extensions (Whole), the last part of a reverse-DNS ID (Tail), the name
/// without the rc, config or history of a configuration file, for example git
/// of .gitconfig, and the first word (Part), each with only letters and digits,
/// of 3 characters or more.
fn keys(name: &str) -> Vec<(String, Kind)> {
    let name = name.trim_start_matches('.');
    let mut base = name;
    while let Some((b, _)) = base.rsplit_once('.').filter(|(b, e)| !b.is_empty() && EXTENSIONS.contains(&e.to_ascii_lowercase().as_str())) {
        base = b;
    }
    let mut keys = vec![(norm(name), Kind::Whole), (norm(base), Kind::Whole)];
    keys.extend(reverse_dns(base).map(|t| (norm(t), Kind::Tail)));
    keys.extend(["rc", "config", "history"].iter().find_map(|s| norm(base).strip_suffix(s).map(|k| (k.to_string(), Kind::Part))));
    keys.extend(base.split(['-', '_', '@', ' ']).next().map(|w| (norm(w), Kind::Part)));
    keys.retain(|k| k.0.len() >= 3);
    let mut seen = BTreeSet::new();
    keys.retain(|k| seen.insert(k.0.clone()));
    keys
}

/// Whether a key agrees with a form of the index.
fn agree(key: Kind, form: Kind) -> bool {
    match form {
        Kind::Whole => true,
        Kind::Tail => key == Kind::Tail,
        Kind::Part => key == Kind::Whole,
    }
}

/// What the audit knows about the origin of a path that nothing explains: the
/// claims of earlier generations that the current generation does not have,
/// the names of the installed packages, of their programs in /usr/bin, of
/// their desktop files and GNOME Shell extensions, of the uv and pipx tools
/// and of the home claims, the directories of the packages, and the literal
/// paths that package tmpfiles rules create.
struct Evidence<'a> {
    former: Vec<(u64, String, bool, Vec<String>, GlobSet, GlobSet)>,
    names: BTreeSet<String>,
    programs: BTreeMap<String, String>,
    desktop: BTreeSet<String>,
    rules: BTreeMap<String, String>,
    /// The notes of DKMS paths, and the DKMS directories whose entries the audit lists.
    dkms: BTreeMap<String, String>,
    containers: BTreeSet<String>,
    /// The records of the packages that are not installed, and the names of the records that explain a path.
    removed: Records,
    used: RefCell<BTreeSet<String>>,
    /// The forms of the names of installed software, with what has the name:
    /// installed packages with their programs, desktop IDs and GNOME Shell
    /// extensions, uv and pipx tools, and the features of the claims.
    index: BTreeMap<String, BTreeSet<(Kind, String)>>,
    /// The mount points of /etc/fstab and of the configuration, and the
    /// directories of removable media: a link into them can work again.
    mounts: Vec<String>,
    /// The packages that own each directory, and the package of each other path.
    dirs: BTreeMap<&'a str, BTreeSet<&'a str>>,
    files: BTreeMap<&'a str, &'a str>,
    /// The claims of the configuration, by feature: system patterns and home patterns.
    claims: Vec<(String, GlobSet, GlobSet)>,
    homes: Vec<String>,
}

impl<'a> Evidence<'a> {
    fn new(current: &Generation, inventory: &'a str, owned: &BTreeSet<String>) -> Result<Self> {
        let mut former = Vec::new();
        for (n, g) in history()?.into_iter().rev() {
            for (feature, c) in g.m.claims {
                let now = current.m.claims.get(&feature);
                let system: Vec<String> = c.system.into_iter().filter(|p| !now.is_some_and(|n| n.system.contains(p))).collect();
                let home: Vec<String> = c.home.into_iter().filter(|p| !now.is_some_and(|n| n.home.contains(p))).collect();
                if !system.is_empty() || !home.is_empty() {
                    former.push((n, feature, now.is_some(), home.clone(), globs(system), globs(home)));
                }
            }
        }
        let desktop = owned.iter().filter_map(|p| p.strip_prefix("/usr/share/applications/")?.strip_suffix(".desktop")).filter(|d| !d.contains('/'));
        // A rule creates a path when its type is one of these letters, with modifiers.
        let mut rules = BTreeMap::new();
        let mut file = String::new();
        // --tldr shows only the file names and the rules, without comments.
        for l in output(cmd("systemd-tmpfiles", &["--tldr", "--no-pager"])).unwrap_or_default().lines() {
            if let Some(f) = l.strip_prefix("# ") {
                file = f.to_string();
                continue;
            }
            if let Some(p) = rule_path(l).filter(|_| owned.contains(&file)) {
                rules.entry(p.to_string()).or_insert_with(|| file.clone());
            }
        }
        let (dkms, containers) = dkms(inventory, owned);
        let names: BTreeSet<String> = inventory.lines().filter_map(|l| l.split_once(' ')).map(|(n, _)| n.to_string()).collect();
        // A user that an installed package also declares, or that has a different UID now, is no evidence.
        let shared: BTreeSet<String> = owned.iter().filter(|p| p.starts_with("/usr/lib/sysusers.d/")).filter_map(|p| std::fs::read_to_string(p).ok()).flat_map(|t| sysusers(&t).map(str::to_string).collect::<Vec<_>>()).collect();
        let users = passwd();
        let mut removed = records();
        removed.retain(|p, _| !names.contains(p));
        // A path that an installed package owns is no leftover, and neither are the paths below it.
        removed.values_mut().for_each(|r| {
            r.users.retain(|u, n| *u != 0 && users.get(u) == Some(n) && !shared.contains(n));
            r.paths.retain(|p, _| !owned.contains(p));
        });
        let homes: Vec<String> = homes()?.into_iter().map(|(h, _)| h).collect();
        let mut index: BTreeMap<String, BTreeSet<(Kind, String)>> = BTreeMap::new();
        let mut add = |name: &str, program: bool, what: String| forms(name, program).into_iter().for_each(|(f, k)| _ = index.entry(f).or_default().insert((k, what.clone())));
        let (mut dirs, mut files) = (BTreeMap::new(), BTreeMap::new());
        for (pkg, path) in inventory.lines().filter_map(|l| l.split_once(' ')) {
            match path.strip_suffix('/') {
                Some(dir) => _ = dirs.entry(if dir.is_empty() { "/" } else { dir }).or_insert_with(BTreeSet::new).insert(pkg),
                None => _ = files.insert(path, pkg),
            }
            let program = path.strip_prefix("/usr/bin/").filter(|n| !n.is_empty() && !n.contains('/'))
                .or_else(|| path.strip_prefix("/usr/share/applications/")?.strip_suffix(".desktop").filter(|d| !d.contains('/')));
            // The part before @ of the UUID of a GNOME Shell extension.
            let extension = path.strip_prefix("/usr/share/gnome-shell/extensions/").and_then(|e| e.strip_suffix('/')).filter(|u| !u.contains('/')).and_then(|u| u.split('@').next());
            for (n, program) in program.map(|n| (n, true)).into_iter().chain(extension.map(|n| (n, false))) {
                add(n, program, format!("installed package {pkg}"));
            }
        }
        for pkg in &names {
            add(pkg, false, format!("installed package {pkg}"));
        }
        for home in &homes {
            for (dir, kind) in [(".local/share/uv/tools", "uv tool"), (".local/share/pipx/venvs", "pipx package")] {
                for e in std::fs::read_dir(Path::new(home).join(dir)).into_iter().flatten().flatten() {
                    let tool = e.file_name().to_string_lossy().into_owned();
                    add(&tool, false, format!("{kind} {tool}"));
                }
            }
        }
        // The application name of a home claim, for example t3code of .config/t3code. The names of system claims are too general, for example cache of /var/cache.
        for (feature, c) in &current.m.claims {
            let names = c.home.iter().filter_map(|p| app_name(p)).filter(|n| !n.is_empty() && !n.contains(['*', '?', '[', '{', '\\']));
            names.for_each(|n| add(n, false, format!("feature {feature}")));
        }
        let fstab = std::fs::read_to_string("/etc/fstab").unwrap_or_default();
        let fstab = fstab.lines().filter(|l| !l.trim_start().starts_with('#')).filter_map(|l| l.split_whitespace().nth(1));
        let mounts = fstab.chain(current.m.mounts.keys().map(String::as_str)).chain(["/mnt", "/media"]).filter(|m| m.starts_with('/') && *m != "/").map(str::to_string).collect();
        let claims = current.m.claims.iter().map(|(f, c)| (f.clone(), globs(c.system.clone()), globs(c.home.clone()))).collect();
        Ok(Evidence {
            mounts,
            index,
            dirs,
            files,
            claims,
            homes,
            removed,
            used: RefCell::default(),
            dkms,
            containers,
            former,
            names,
            programs: inventory.lines().filter_map(|l| l.split_once(' ')).filter_map(|(n, p)| Some((p.strip_prefix("/usr/bin/")?.to_string(), n.to_string()))).filter(|(p, _)| !p.is_empty() && !p.contains('/')).collect(),
            desktop: desktop.map(str::to_string).collect(),
            rules,
        })
    }

    /// Whether a path is a leftover (a former claim, a path of a removed package
    /// or a link to a path that does not exist), and its note. A system path compares its own name, a home path
    /// its `app_name`; rel is the path in the home directory. A clue whose name is also the name of a removed package tells
    /// it, and the path is no leftover.
    fn of(&self, path: &str, rel: Option<&str>) -> (bool, String) {
        let found = self.former.iter().find(|(.., s, h)| match rel {
            // A path below a former claim is part of the data of that claim.
            Some(r) => Path::new(r).ancestors().any(|a| h.is_match(a)),
            None => Path::new(path).ancestors().any(|a| s.is_match(a)),
        });
        if let Some((n, f, kept, ..)) = found {
            let why = if *kept { "the feature no longer claims it" } else { "the feature is no longer enabled" };
            return (true, format!(" (claimed by feature {f} in generation {n}; {why})"));
        }
        let owner = std::fs::symlink_metadata(path).ok().map(|m| m.uid());
        for (pkg, r) in &self.removed {
            let exact = match rel {
                Some(rel) => Path::new(rel).ancestors().find_map(|a| r.home.get(a.to_str()?)).cloned(),
                None => Path::new(path).ancestors().find_map(|a| r.paths.get(a.to_str()?)).cloned(),
            };
            if let Some(note) = exact.or_else(|| Some(format!("the owner is user {}", owner.and_then(|u| r.users.get(&u))?))) {
                self.used.borrow_mut().insert(pkg.clone());
                return (true, format!(" ({note} of removed package {pkg})"));
            }
        }
        let link = std::fs::read_link(path).ok().map(|t| t.display().to_string());
        // A target in /run and the other runtime directories, or on a file system that is not mounted, can come back.
        let back = |t: &Path| ["/run", "/proc", "/sys", "/dev", "/tmp"].iter().any(|r| t.starts_with(r)) || self.mounts.iter().any(|m| t.starts_with(m));
        if let Some(t) = &link {
            let abs = Path::new(path).parent().unwrap_or(Path::new("/")).join(t);
            if !back(&abs) && std::fs::metadata(&abs).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
                return (true, format!(" (a link to {t}, which does not exist)"));
            }
        }
        let name = match rel {
            Some(r) => app_name(r),
            None => Path::new(path).file_name().and_then(|n| n.to_str()),
        };
        let removed = name.and_then(|n| self.removed.iter().find(|(_, r)| r.names.contains(n)).map(|(p, _)| (n, p)));
        let what = |(n, p): (&str, &String)| {
            self.used.borrow_mut().insert(p.clone());
            let what = if n == p { String::new() } else { format!("program or desktop ID {n} of ") };
            format!("{what}removed package {p}")
        };
        if let Some(clue) = self.clue(path, rel, name, link.as_deref()) {
            return match removed {
                Some(r) => (false, format!(" ({clue}; the name is also the same as {})", what(r))),
                None => (false, format!(" ({clue})")),
            };
        }
        if let Some(r) = removed {
            return (true, format!(" (the name is the same as {})", what(r)));
        }
        let weak = rel.is_none().then(|| self.in_package_dir(path)).flatten().or_else(|| self.holds(path)).or_else(|| link.map(|t| format!("a link to {t}")));
        (false, weak.map(|clue| format!(" ({clue})")).unwrap_or_default())
    }

    /// The clue of a path, strongest first: the target of a link, DKMS, the same
    /// name, a tmpfiles rule and a name that agrees.
    fn clue(&self, path: &str, rel: Option<&str>, name: Option<&str>, link: Option<&str>) -> Option<String> {
        if let Some(t) = link {
            let unit = Path::new(path).parent().is_some_and(|d| d == Path::new("/etc/systemd/system") || d == Path::new("/etc/systemd/user"));
            if unit && t == "/dev/null" {
                return Some(format!("a mask of unit {}, which units.mask does not declare", name?));
            }
            if let Some(real) = std::fs::canonicalize(path).ok() {
                let home = self.homes.iter().find_map(|h| real.strip_prefix(h).ok());
                let feature = self.claims.iter().find(|(_, s, h)| match home {
                    Some(r) => r.ancestors().any(|a| h.is_match(a)),
                    None => real.ancestors().any(|a| s.is_match(a)),
                });
                if let Some((f, ..)) = feature {
                    return Some(format!("a link to {t}, which feature {f} claims"));
                }
                let real = real.display().to_string();
                if let Some(p) = self.files.get(real.as_str()) {
                    return Some(format!("a link to {t} of installed package {p}"));
                }
                if let Some(owners) = self.dirs.get(real.as_str()) {
                    return Some(format!("a link to {t}, a directory of {}", packages(owners)));
                }
            }
        }
        if let Some(note) = rel.is_none().then(|| self.dkms.get(path)).flatten() {
            return Some(note.trim_start_matches(" (").trim_end_matches(')').to_string());
        }
        match name? {
            n if self.names.contains(n) => Some(format!("the name is the same as installed package {n}")),
            n if self.programs.contains_key(n) => Some(format!("the name is the same as program /usr/bin/{n} of installed package {}", self.programs[n])),
            n if self.desktop.contains(n) => Some(format!("the name is the same as desktop ID {n}")),
            n => match rel.is_none().then(|| self.rules.get(path)).flatten() {
                Some(file) => Some(format!("a tmpfiles rule in {file} has this path")),
                None => self.agrees(n).map(|w| format!("the name agrees with {w}")),
            },
        }
    }

    /// What has a name that agrees with the path name: the most specific key of
    /// `keys` that agrees with a form of the index, with at most two of what has it.
    fn agrees(&self, name: &str) -> Option<String> {
        let found = keys(name).iter().find_map(|(k, kind)| {
            let found: BTreeSet<&str> = self.index.get(k)?.iter().filter(|(f, _)| agree(*kind, *f)).map(|(_, w)| w.as_str()).collect();
            (!found.is_empty()).then_some(found)
        })?;
        let first: Vec<&str> = found.iter().take(2).copied().collect();
        Some(match found.len() {
            1 | 2 => first.join(" and "),
            n => format!("{}, {} and {} more", first[0], first[1], n - 2),
        })
    }

    /// A system path in a directory that one or two installed packages own,
    /// except filesystem: a package that has a path below a directory also owns
    /// the directory, thus no other package has a path in it.
    fn in_package_dir(&self, path: &str) -> Option<String> {
        let (dir, owners) = Path::new(path).ancestors().skip(1).find_map(|a| Some((a, self.dirs.get(a.to_str()?)?)))?;
        let verb = if owners.len() == 1 { "owns" } else { "own" };
        (owners.len() <= 2 && !owners.contains("filesystem")).then(|| format!("in directory {}, which only {} {verb}", dir.display(), packages(owners)))
    }

    /// A directory that holds an application ID, for example the icon
    /// com.t3tools.T3Code.png, whose name agrees with installed software or a
    /// feature. Other entry names are too general, for example logs. It reads
    /// at most 200 entries.
    fn holds(&self, path: &str) -> Option<String> {
        std::fs::symlink_metadata(path).ok().filter(|m| m.is_dir())?;
        let mut names: Vec<String> = std::fs::read_dir(path).ok()?.take(200).flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        let id = |e: &&String| e.split('.').filter(|p| !p.is_empty() && !EXTENSIONS.contains(&p.to_ascii_lowercase().as_str())).count() >= 3;
        names.iter().filter(id).find_map(|e| Some(format!("it holds {e}, whose name agrees with {}", self.agrees(e)?)))
    }
}

/// The installed packages of a set: one or two by name, more by number.
fn packages(owners: &BTreeSet<&str>) -> String {
    match owners.len() {
        1 => format!("installed package {}", owners.iter().next().unwrap()),
        2 => format!("installed packages {}", owners.iter().copied().collect::<Vec<_>>().join(" and ")),
        n => format!("{n} installed packages"),
    }
}

pub fn audit(ctx: &Ctx) -> Result<()> {
    let g = build(ctx)?;
    let _stage = crate::report::stage("Checking the system");
    let mut report: Vec<String> = Vec::new();
    // Paths with whether they are leftovers: Evidence::of.
    let mut paths: Vec<Found> = Vec::new();
    let inventory = output(cmd("pacman", &["-Ql"]))?;
    let owned: BTreeSet<String> = inventory.lines().filter_map(|l| l.split_once(' ')).map(|(_, p)| p.trim_end_matches('/').to_string()).collect();
    let evidence = Evidence::new(&g, &inventory, &owned)?;
    // The walk enters the directories above a declared file and above the fixed part of a system claim.
    let fixed = g.m.claims.values().flat_map(|c| &c.system).map(|p| {
        let p = Path::new(p.trim_end_matches('/'));
        let literal: std::path::PathBuf = p.components().take_while(|c| !c.as_os_str().to_string_lossy().contains(['*', '?', '[', '{'])).collect();
        if literal == p { literal.parent().map(Path::to_path_buf).unwrap_or_default() } else { literal }
    });
    let parents: BTreeSet<String> = g.m.files.keys().filter_map(|p| Some(Path::new(p).parent()?.to_path_buf())).chain(fixed).flat_map(|p| p.ancestors().map(|a| a.display().to_string()).collect::<Vec<_>>()).collect();
    let claimed = globs(g.m.claims.values().flat_map(|c| c.system.clone()).chain(g.m.audit.ignore.clone()));
    let mut walk = walkdir::WalkDir::new("/").into_iter();
    while let Some(entry) = walk.next() {
        // An entry that cannot be read or that disappears does not stop the walk.
        let Ok(e) = entry else { continue };
        let path = e.path().display().to_string();
        if SKIP.contains(&path.as_str()) || claimed.is_match(&path) || g.m.files.contains_key(&path) {
            if e.file_type().is_dir() {
                walk.skip_current_dir();
            }
            continue;
        }
        if owned.contains(&path) || parents.contains(&path) || path == "/" {
            continue;
        }
        if path.ends_with(".pacnew") || path.ends_with(".pacsave") {
            match evidence.of(&path, None) {
                (true, note) => paths.push((true, format!("{path}{note}"), path.clone())),
                _ => report.push(format!("File {}: {path}", &path[path.rfind('.').unwrap()..])),
            }
            continue;
        }
        // A link to a file of a package, for example a fontconfig link in /etc/fonts/conf.d, is explained.
        let target = if e.path_is_symlink() { std::fs::canonicalize(e.path()).ok() } else { None };
        if target.is_some_and(|t| !t.is_dir() && owned.contains(&t.display().to_string())) {
            continue;
        }
        // The mask of a unit that units.mask declares.
        let mask = path.strip_prefix("/etc/systemd/system/").is_some_and(|u| g.m.units.mask.iter().any(|m| m == u));
        if mask && std::fs::read_link(e.path()).is_ok_and(|t| t == Path::new("/dev/null")) {
            continue;
        }
        if e.file_type().is_dir() && evidence.containers.contains(&path) && std::fs::read_dir(e.path()).is_ok_and(|mut d| d.next().is_some()) {
            continue;
        }
        let (leftover, note) = evidence.of(&path, None);
        paths.push((leftover, format!("{path}{note}"), path.clone()));
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
            report.push(format!("Modified package file: {path} ({pkg})"));
        }
    }

    for (path, spec) in &g.m.files {
        if !files::matches(path, spec) {
            report.push(format!("Managed file differs from the generation: {path}"));
        }
    }

    let (declared, _) = crate::packages::declared(&g, &crate::packages::sync_names()?)?;
    for p in output(cmd("pacman", &["-Qqe"]))?.lines() {
        if !declared.contains(p) {
            report.push(format!("Explicit package not declared: {p}"));
        }
    }

    // A pattern of the configuration that matches no path is out of date. The claims of the pacnix modules are not the configuration.
    let none = |p: &str| expand(p, 1).is_empty();
    let own = |p: &&String| !g.m.audit.builtin.contains(*p);
    for (feature, c) in &g.m.claims {
        report.extend(c.system.iter().filter(own).filter(|p| none(p)).map(|p| format!("Claim that matches no path: {p} (feature {feature})")));
        let stale = c.home.iter().filter(own).filter(|p| evidence.homes.iter().all(|h| none(&format!("{}/{p}", globset::escape(h)))));
        report.extend(stale.map(|p| format!("Claim that matches no path: {p} (home claim of feature {feature})")));
    }
    report.extend(g.m.audit.ignore.iter().filter(|p| none(p)).map(|p| format!("Pattern of audit.ignore that matches no path: {p}")));

    let units = crate::units::plan(&g, &g, true)?;
    report.extend(units.disable.iter().map(|u| format!("Enabled unit not declared and not enabled by a vendor preset: {u}")));
    report.extend(units.enable.iter().filter(|u| g.m.units.enable.contains(*u)).map(|u| format!("Declared unit not enabled: {u}")));

    home(&g, &evidence, &mut paths)?;
    // A record goes when its package is installed again or when it explains no path.
    let used = evidence.used.take();
    let mut kept = records();
    if kept.len() != used.len() {
        kept.retain(|p, _| used.contains(p));
        save(&kept)?;
    }
    if let Some(state) = crate::secure::audit(&g, &mut report)? {
        say!("• {state}");
    }
    drop(_stage);
    let mut notes = Vec::new();
    crate::report::audit(&mut notes);
    notes.iter().for_each(|n| say!("• {n}"));
    g.m.warnings.iter().for_each(|w| say!("• Warning: {w}"));
    // A path that pacnix never deletes, for example a bash file, is no leftover to delete.
    let keep = Keep::new(&g)?;
    paths.iter_mut().filter(|p| p.0 && keep.covers(&p.2)).for_each(|p| p.0 = false);
    paths.sort_by(|a, b| a.2.cmp(&b.2));
    print(&report, &paths);
    let leftovers: Vec<&str> = paths.iter().filter(|p| p.0).map(|p| p.2.as_str()).collect();
    // The claims of a configuration that apply did not activate yet are not the claims of the system.
    let deleted = match crate::generation::active()?.is_some_and(|a| a.path == g.path) {
        true => clean(&g, &leftovers)?,
        false if !leftovers.is_empty() => {
            say!("• The configuration differs from the active configuration. Run pacnix apply, then pacnix audit can delete the leftovers.");
            0
        }
        false => 0,
    };
    if !report.is_empty() || !paths.is_empty() {
        crate::report::gap();
        let deleted = if deleted == 0 { String::new() } else { format!(", {deleted} deleted") };
        say!("• Audit done: {}{deleted}.", crate::count(report.len() + paths.len(), "difference"));
    }
    Ok(())
}

/// A path that the audit found: whether it is a leftover (Evidence::of), its line and the path.
type Found = (bool, String, String);

/// Asks which leftovers to delete, as the AUR helpers
/// ask: a for all, Enter or n for none, or the numbers and ranges, for example
/// 1 3-5. As with pacman, the answer comes from standard input, also without a
/// terminal, and nothing is deleted at its end. The leftovers that stay show
/// again in the next audit. Returns the number of deleted leftovers.
fn clean(g: &Generation, leftovers: &[&str]) -> Result<usize> {
    if leftovers.is_empty() {
        return Ok(0);
    }
    // With the output in a pager or a file, the user does not see the question.
    if std::io::stdin().is_terminal() && !std::io::stdout().is_terminal() {
        say!("• To delete leftovers, run pacnix audit with the output in the terminal.");
        return Ok(0);
    }
    print!("Delete leftovers? [a]ll, [N]one, or numbers such as 1 3-5: ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer)? == 0 {
        println!();
    }
    let answer = answer.trim().to_lowercase();
    let chosen: BTreeSet<usize> = match answer.as_str() {
        "" | "n" | "none" => BTreeSet::new(),
        "a" | "all" => (1..=leftovers.len()).collect(),
        _ => {
            let mut set = BTreeSet::new();
            for w in answer.split([' ', ',']).filter(|w| !w.is_empty()) {
                let (a, b) = w.split_once('-').unwrap_or((w, w));
                match (a.parse::<usize>(), b.parse::<usize>()) {
                    (Ok(a), Ok(b)) if 1 <= a && a <= b && b <= leftovers.len() => set.extend(a..=b),
                    _ => bail!("{w} is not a number or a range of the list, thus nothing is deleted. Run pacnix audit again"),
                }
            }
            set
        }
    };
    let data: Vec<(String, Vec<String>)> = chosen.iter().map(|i| (leftovers[i - 1].to_string(), vec![leftovers[i - 1].to_string()])).collect();
    delete(g, &data)?;
    Ok(chosen.len())
}

/// Shows the number of items first. Then the paths with no evidence, the
/// paths with a clue and the other items, under one heading for each kind.
/// Last, the numbered leftovers: the paths with the evidence of a removed
/// package or a former claim, and the links to paths that do not exist, for
/// the question of `clean`.
fn print(report: &[String], paths: &[Found]) {
    if report.is_empty() && paths.is_empty() {
        say!("• The system agrees with the configuration.");
        return;
    }
    let unowned: Vec<&str> = paths.iter().filter(|p| !p.0).map(|p| p.1.as_str()).collect();
    if !unowned.is_empty() {
        say!("• Paths that no package, declaration or claim owns ({}):", unowned.len());
        unowned.iter().for_each(|p| say!("  {p}"));
    }
    let mut kinds: Vec<(&str, Vec<&str>)> = Vec::new();
    for line in report {
        let (kind, item) = line.split_once(": ").unwrap_or((line, ""));
        match kinds.iter_mut().find(|k| k.0 == kind) {
            Some(k) => k.1.push(item),
            None => kinds.push((kind, vec![item])),
        }
    }
    for (kind, items) in &kinds {
        say!("• {kind} ({}):", items.len());
        items.iter().for_each(|i| say!("  {i}"));
    }
    say!("• Apply corrects the managed files, the declared units, the boot files and the TPM2 keys. It never removes a package that it did not install. Declare each other difference, add it to claims or audit.ignore, or remove it.");
    let leftovers: Vec<&str> = paths.iter().filter(|p| p.0).map(|p| p.1.as_str()).collect();
    if !leftovers.is_empty() {
        say!("• Leftovers of removed packages and features, and broken links ({}):", leftovers.len());
        for (i, l) in leftovers.iter().enumerate() {
            say!("  {:>2}  {}", i + 1, l);
        }
    }
}

/// The notes of the paths of DKMS builds that are active: a registration
/// /var/lib/dkms/MODULE/kernel-KERNEL-ARCH links to VERSION/KERNEL/ARCH, the
/// dkms.conf of the source of VERSION is a regular file of a package, and the
/// kernel is installed. An installed module in /usr/lib/modules/KERNEL/updates/dkms
/// has a note when the build has a module file (.ko, also compressed) with the
/// same name and the same bytes, and a build directory when all its entries are
/// the source link and kernels with an active registration. Also the
/// directories whose entries the audit lists: each module directory and the
/// updates directories of each kernel.
fn dkms(inventory: &str, owned: &BTreeSet<String>) -> (BTreeMap<String, String>, BTreeSet<String>) {
    let confs: BTreeMap<&str, &str> = inventory.lines().filter_map(|l| l.split_once(' ')).filter(|(_, p)| p.ends_with("/dkms.conf")).map(|(n, p)| (p, n)).collect();
    let is_dir = |p: &Path| p.symlink_metadata().is_ok_and(|m| m.is_dir());
    let mut notes = BTreeMap::new();
    let mut containers = BTreeSet::new();
    for m in std::fs::read_dir("/var/lib/dkms").into_iter().flatten().flatten().filter(|m| is_dir(&m.path())) {
        let module = m.file_name().to_string_lossy().to_string();
        containers.insert(m.path().display().to_string());
        let mut live: BTreeMap<String, (String, BTreeSet<String>)> = BTreeMap::new();
        for l in std::fs::read_dir(m.path()).into_iter().flatten().flatten() {
            let Ok(target) = std::fs::read_link(l.path()) else { continue };
            let parts: Vec<&str> = target.to_str().unwrap_or_default().split('/').collect();
            let [version, kernel, arch] = parts[..] else { continue };
            if l.file_name().to_str() != Some(&format!("kernel-{kernel}-{arch}")) || parts.iter().any(|c| c.is_empty() || *c == "." || *c == "..") {
                continue;
            }
            let build = m.path().join(&target);
            let link = m.path().join(version).join("source");
            let source = link.symlink_metadata().is_ok_and(|l| l.is_symlink()).then(|| std::fs::canonicalize(&link).ok()).flatten();
            let conf = source.map(|s| s.join("dkms.conf")).filter(|c| c.symlink_metadata().is_ok_and(|m| m.is_file()));
            let package = conf.and_then(|c| confs.get(c.to_str()?).copied());
            let (Some(package), true) = (package, [m.path().join(version), m.path().join(version).join(kernel), build.clone(), build.join("module")].iter().all(|p| is_dir(p))) else { continue };
            if !owned.contains(&format!("/usr/lib/modules/{kernel}")) {
                continue;
            }
            let what = format!("DKMS module {module} {version} for kernel {kernel}; the source is in package {package}");
            notes.insert(l.path().display().to_string(), format!(" (active registration of {what})"));
            let modules = std::fs::read_dir(build.join("module")).into_iter().flatten().flatten();
            for f in modules.filter(|f| [".ko", ".ko.gz", ".ko.xz", ".ko.zst"].iter().any(|x| f.file_name().to_string_lossy().ends_with(x))) {
                let installed = Path::new("/usr/lib/modules").join(kernel).join("updates/dkms").join(f.file_name());
                if same(&f.path(), &installed) {
                    notes.insert(installed.display().to_string(), format!(" (the same bytes as the build of {what})"));
                }
            }
            live.entry(version.to_string()).or_insert_with(|| (package.to_string(), BTreeSet::new())).1.insert(kernel.to_string());
        }
        for (version, (package, kernels)) in live {
            let dir = m.path().join(&version);
            if std::fs::read_dir(&dir).into_iter().flatten().flatten().all(|e| e.file_name() == "source" || e.file_name().to_str().is_some_and(|k| kernels.contains(k))) {
                notes.insert(dir.display().to_string(), format!(" (DKMS module {module} {version}: the source is in package {package}, and each kernel directory has an active registration)"));
            }
        }
    }
    for k in std::fs::read_dir("/usr/lib/modules").into_iter().flatten().flatten() {
        containers.insert(k.path().join("updates").display().to_string());
        containers.insert(k.path().join("updates/dkms").display().to_string());
    }
    (notes, containers)
}

/// Opens a regular file and gives its size. It does not follow a link and does not wait, for example on a FIFO.
fn open_regular(path: &Path) -> Option<(std::fs::File, u64)> {
    use std::os::unix::fs::OpenOptionsExt;
    if !path.symlink_metadata().ok()?.is_file() {
        return None;
    }
    let f = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path).ok()?;
    let m = f.metadata().ok()?;
    m.is_file().then_some((f, m.len()))
}

/// Whether two paths are regular files with the same bytes, read in parts.
fn same(a: &Path, b: &Path) -> bool {
    use std::io::Read;
    let (Some((mut x, n)), Some((mut y, m))) = (open_regular(a), open_regular(b)) else { return false };
    let (mut p, mut q) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
    if n != m {
        return false;
    }
    loop {
        let Ok(i) = x.read(&mut p) else { return false };
        if i == 0 {
            return y.read(&mut q).is_ok_and(|j| j == 0);
        }
        if y.read_exact(&mut q[..i]).is_err() || p[..i] != q[..i] {
            return false;
        }
    }
}

/// The directories of a home directory whose entries the audit lists.
const CONTAINERS: &[&str] = &[".config", ".cache", ".local", ".local/share", ".local/state", ".local/bin"];
/// The skeleton files that are explained while they equal the file in /etc/skel.
const SKELETON: &[&str] = &[".bashrc", ".bash_profile", ".bash_logout"];
/// Home files that the deletion of the data of a removed feature keeps.
const SHELL_FILES: &[&str] = &[".bashrc", ".bash_profile", ".bash_logout", ".bash_history"];

/// Lists the entries of each home directory that no claim, no audit.ignore
/// pattern, no Home Manager file, no XDG user directory and no unchanged
/// skeleton file explains. The walk lists the entries of the home directory,
/// of the directories in CONTAINERS, and of each directory above a Home
/// Manager file or above the fixed part of a pattern. Another directory shows
/// as one entry for all of its content. The walk does not follow links.
fn home(g: &Generation, evidence: &Evidence, paths: &mut Vec<Found>) -> Result<()> {
    let claims: Vec<&String> = g.m.claims.values().flat_map(|c| &c.home).collect();
    for (dir, managed) in homes()? {
        let base = globset::escape(&dir);
        let patterns: Vec<String> = claims.iter().map(|c| format!("{base}/{c}")).chain([format!("{base}/**/*.pacnix-hm-backup")]).chain(g.m.audit.ignore.iter().cloned()).collect();
        let set = globs(patterns.clone());
        // The walk also goes down to the paths that features claimed before.
        let patterns: Vec<String> = patterns.into_iter().chain(evidence.former.iter().flat_map(|f| f.3.iter().map(|c| format!("{base}/{c}")))).collect();
        // A Home Manager file has no managed path below it. Its directories are only on the way to it.
        let leaves: BTreeSet<&String> = managed.iter().filter(|m| !managed.range(format!("{m}/")..).next().is_some_and(|n| n.starts_with(&format!("{m}/")))).collect();
        let above: BTreeSet<String> = leaves.iter().flat_map(|l| Path::new(l.as_str()).ancestors().skip(1).map(|a| a.display().to_string())).collect();
        let xdg = xdg_dirs(&dir);
        let above: BTreeSet<String> = above.into_iter().chain(xdg.iter().flat_map(|x| Path::new(x.as_str()).ancestors().skip(1).map(|a| a.display().to_string()))).collect();
        let mut walk = walkdir::WalkDir::new(&dir).min_depth(1).sort_by_file_name().into_iter();
        while let Some(entry) = walk.next() {
            let Ok(e) = entry else { continue };
            let abs = e.path().display().to_string();
            let rel = e.path().strip_prefix(&dir)?.display().to_string();
            let is_dir = e.file_type().is_dir();
            let explained = set.is_match(&abs) || leaves.contains(&rel) || xdg.contains(&rel) || skeleton(&dir, &rel);
            if !explained && is_dir && (CONTAINERS.contains(&rel.as_str()) || above.contains(&rel) || enters(&patterns, &abs, &dir)) {
                continue;
            }
            if !explained {
                let (leftover, note) = evidence.of(&abs, Some(&rel));
                paths.push((leftover, format!("{abs}{note}"), abs.clone()));
            }
            if is_dir {
                walk.skip_current_dir();
            }
        }
    }
    Ok(())
}

/// Whether the walk enters a directory for a pattern: the directory is above
/// the fixed part of the pattern, or below a fixed part inside the home
/// directory, at any depth for a pattern with ** and above the depth of the
/// pattern without.
fn enters(patterns: &[String], dir: &str, home: &str) -> bool {
    let depth = dir.split('/').count();
    patterns.iter().any(|p| {
        let parts: Vec<&str> = p.trim_end_matches('/').split('/').collect();
        let fixed = parts.iter().take_while(|c| !c.contains(['*', '?', '[', '{', '\\'])).count();
        let literal = parts[..fixed].join("/");
        literal.starts_with(&format!("{dir}/"))
            || (literal.starts_with(&format!("{home}/"))
                && fixed < parts.len()
                && (p.contains("**") || depth < parts.len())
                && (dir == literal || dir.starts_with(&format!("{literal}/"))))
    })
}

/// The content of a regular file of at most 64 KiB. The file can belong to a
/// user: a link, a FIFO and a device give None, and no read waits.
fn read_small(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let (f, _) = open_regular(path)?;
    let mut buf = Vec::new();
    f.take(65537).read_to_end(&mut buf).ok()?;
    (buf.len() <= 65536).then_some(buf)
}

/// The XDG user directories in ~/.config/user-dirs.dirs, relative to the home
/// directory. Only a line XDG_NAME_DIR="$HOME/path" or "/absolute/path" inside
/// the home directory, with a plain path, counts: pacnix does not run the file
/// as shell, and a disabled directory, which is the home directory itself,
/// explains nothing.
fn xdg_dirs(home: &str) -> BTreeSet<String> {
    let config = Path::new(home).join(".config");
    if !config.symlink_metadata().is_ok_and(|m| m.is_dir()) {
        return BTreeSet::new();
    }
    let text = read_small(&config.join("user-dirs.dirs")).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let (key, value) = l.split_once('=')?;
            let name = key.strip_prefix("XDG_")?.strip_suffix("_DIR")?;
            let value = value.strip_prefix('"')?.strip_suffix('"')?;
            let rel = value.strip_prefix("$HOME/").or_else(|| value.strip_prefix(home)?.strip_prefix('/'))?.trim_end_matches('/');
            let plain = !name.is_empty() && name.chars().all(|c| c.is_ascii_uppercase() || c == '_');
            let safe = !rel.is_empty() && !rel.contains(['$', '`', '\\', '"']) && rel.split('/').all(|c| !c.is_empty() && c != "." && c != "..");
            (plain && safe).then(|| rel.to_string())
        })
        .collect()
}

/// Whether a bash skeleton file equals its counterpart in /etc/skel.
fn skeleton(home: &str, rel: &str) -> bool {
    SKELETON.contains(&rel) && read_small(&Path::new(home).join(rel)).is_some_and(|c| Some(c) == read_small(&Path::new("/etc/skel").join(rel)))
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
            home: globs(target.m.claims.values().flat_map(|c| c.home.clone()).chain(SHELL_FILES.iter().map(|f| f.to_string()))),
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

/// The existing paths that a claim pattern matches, without the paths below a
/// match, at most `limit`.
fn expand(pattern: &str, limit: usize) -> Vec<String> {
    let pattern = Path::new(pattern.trim_end_matches('/'));
    let literal: std::path::PathBuf = pattern.components().take_while(|c| !c.as_os_str().to_string_lossy().contains(['*', '?', '[', '{'])).collect();
    if literal == pattern {
        return pattern.symlink_metadata().map(|_| pattern.display().to_string()).into_iter().collect();
    }
    let set = globs([pattern.display().to_string()]);
    // Without **, a match is at the depth of the pattern.
    let depth = if pattern.to_string_lossy().contains("**") { usize::MAX } else { pattern.components().count() - literal.components().count() };
    let (mut found, mut walk) = (Vec::new(), walkdir::WalkDir::new(&literal).max_depth(depth).follow_root_links(false).into_iter());
    while let Some(entry) = walk.next() {
        let Ok(e) = entry else { continue };
        if set.is_match(e.path()) {
            found.push(e.path().display().to_string());
            if found.len() == limit {
                break;
            }
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
        let mut paths: Vec<String> = c.system.iter().flat_map(|p| expand(p, usize::MAX)).collect();
        for (home, _) in &keep.homes {
            paths.extend(c.home.iter().flat_map(|p| expand(&format!("{home}/{p}"), usize::MAX)));
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

/// The label of the deletion: the feature, or the path that audit deletes.
fn feature_or_path(feature: &str, paths: &[String]) -> String {
    match paths {
        [p] if p == feature => p.clone(),
        _ => format!("the data of removed feature {feature}"),
    }
}

/// Deletes the data of removed features with rm, as the user would type it:
/// pacnix gives the paths to rm in the interactive shell of the user whose home
/// directory holds them, else of root, thus an alias of rm applies, for example
/// to a trash command. A path inside that the target keeps stays, with its
/// directories. Outside the home directories of the users, the empty
/// directories above a deleted path that no package owns go too.
pub fn delete(target: &Generation, data: &[(String, Vec<String>)]) -> Result<()> {
    let keep = Keep::new(target)?;
    for (feature, paths) in data {
        crate::report::progress(&format!("Deleting {}", feature_or_path(feature, paths)));
        for path in paths {
            let entries = walkdir::WalkDir::new(path).follow_root_links(false).into_iter().collect::<Result<Vec<_>, _>>()?;
            // rm gets the largest parts that hold no kept path.
            let kept: BTreeSet<&Path> = entries.iter().filter(|e| keep.covers(&e.path().display().to_string())).flat_map(|e| e.path().ancestors()).collect();
            let parts: Vec<&Path> = entries
                .iter()
                .map(|e| e.path())
                .filter(|p| !kept.contains(p) && (*p == Path::new(path) || p.parent().is_some_and(|d| kept.contains(d))))
                .collect();
            if !parts.is_empty() {
                shell_rm(path, &parts)?;
            }
            if !keep.homes.iter().any(|(home, _)| Path::new(path).starts_with(home)) {
                files::prune_dirs(path);
            }
        }
    }
    Ok(())
}

/// Runs rm -r on the parts in the interactive shell of the user whose home
/// directory holds the path, else of root.
fn shell_rm(path: &str, parts: &[&Path]) -> Result<()> {
    let text = std::fs::read_to_string("/etc/passwd")?;
    let users: Vec<Vec<&str>> = text.lines().map(|l| l.split(':').collect::<Vec<_>>()).filter(|f| f.len() >= 7).collect();
    let person = |f: &&Vec<&str>| f[2].parse::<u32>().is_ok_and(|u| (1000..60000).contains(&u)) && Path::new(path).starts_with(f[5]);
    let owner = match users.iter().filter(person).max_by_key(|f| f[5].len()) {
        Some(f) => f,
        None => users.iter().find(|f| f[2] == "0").context("/etc/passwd has no root user")?,
    };
    let mut c = cmd("runuser", &["-u", owner[0], "--", owner[6], "-ic", r#"rm -r -- "$@""#, "rm"]);
    c.args(parts).env_clear().envs([("HOME", owner[5]), ("USER", owner[0]), ("LOGNAME", owner[0]), ("PATH", "/usr/local/bin:/usr/bin")]);
    c.current_dir(if Path::new(owner[5]).is_dir() { owner[5] } else { "/" });
    crate::run_cmd(c)
}
