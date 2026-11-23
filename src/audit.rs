use crate::generation::{Generation, build, history};
use crate::{Ctx, cmd, files, output, output_any};
use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::collections::{BTreeMap, BTreeSet};
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

/// The directories of a home directory whose children are named after an application.
const NAMESPACES: &[&str] = &[".config", ".cache", ".local/share", ".local/state", ".var/app"];

/// What the audit knows about the origin of a path that nothing explains: the
/// claims of earlier generations that the current generation does not have,
/// the names of the installed packages, of their programs in /usr/bin and of
/// their desktop files, and the literal paths that package tmpfiles rules create.
struct Evidence {
    former: Vec<(u64, String, bool, Vec<String>, GlobSet, GlobSet)>,
    names: BTreeSet<String>,
    programs: BTreeMap<String, String>,
    desktop: BTreeSet<String>,
    rules: BTreeMap<String, String>,
    /// The notes of DKMS paths, and the DKMS directories whose entries the audit lists.
    dkms: BTreeMap<String, String>,
    containers: BTreeSet<String>,
}

impl Evidence {
    fn new(current: &Generation, inventory: &str, owned: &BTreeSet<String>) -> Result<Self> {
        let mut former = Vec::new();
        for (n, g) in history()?.into_iter().rev() {
            for (feature, c) in g.m.claims {
                let now = current.m.claims.get(&feature);
                let system: Vec<String> = c.system.into_iter().filter(|p| !now.is_some_and(|n| n.system.contains(p))).collect();
                // Before 0.16, feature arch claimed the bash files for the audit; that is no evidence of data.
                let builtin = |p: &String| feature == "arch" && SHELL_FILES.contains(&p.as_str());
                let home: Vec<String> = c.home.into_iter().filter(|p| !now.is_some_and(|n| n.home.contains(p)) && !builtin(p)).collect();
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
            let mut w = l.split_whitespace();
            let (Some(t), Some(p)) = (w.next(), w.next()) else { continue };
            if owned.contains(&file)
                && t.starts_with(['f', 'd', 'D', 'v', 'q', 'Q', 'p', 'L', 'c', 'b', 'C'])
                && t[1..].chars().all(|c| "+!-=~^?$".contains(c))
                && p.starts_with('/')
                && !p.contains(['%', '*', '?', '[', '"', '\'', '\\'])
            {
                rules.entry(p.trim_end_matches('/').to_string()).or_insert_with(|| file.clone());
            }
        }
        let (dkms, containers) = dkms(inventory, owned);
        Ok(Evidence {
            dkms,
            containers,
            former,
            names: inventory.lines().filter_map(|l| l.split_once(' ')).map(|(n, _)| n.to_string()).collect(),
            programs: inventory.lines().filter_map(|l| l.split_once(' ')).filter_map(|(n, p)| Some((p.strip_prefix("/usr/bin/")?.to_string(), n.to_string()))).filter(|(p, _)| !p.is_empty() && !p.contains('/')).collect(),
            desktop: desktop.map(str::to_string).collect(),
            rules,
        })
    }

    /// The rank and the note of a path: 0 for a former claim, 1 for no
    /// evidence, 2 for a name or a rule that agrees with the path. A system
    /// path compares its own name. A home path compares the name below a
    /// directory of NAMESPACES, or the name of a hidden entry of the home
    /// directory without its dot; rel is the path in the home directory.
    fn of(&self, path: &str, rel: Option<&str>) -> (u8, String) {
        let found = self.former.iter().find(|(.., s, h)| match rel {
            Some(r) => h.is_match(r),
            None => s.is_match(path),
        });
        if let Some((n, f, kept, ..)) = found {
            let why = if *kept { "the feature no longer claims it" } else { "the feature is no longer enabled" };
            return (0, format!(" (claimed by feature {f} in generation {n}; {why})"));
        }
        if let Some(note) = rel.is_none().then(|| self.dkms.get(path)).flatten() {
            return (2, note.clone());
        }
        let name = match rel {
            Some(r) => NAMESPACES.iter().find_map(|ns| r.strip_prefix(ns)?.strip_prefix('/')?.split('/').next()).or_else(|| r.split('/').next()?.strip_prefix('.')),
            None => Path::new(path).file_name().and_then(|n| n.to_str()),
        };
        match name {
            Some(n) if self.names.contains(n) => (2, format!(" (the name is the same as installed package {n})")),
            Some(n) if self.programs.contains_key(n) => (2, format!(" (the name is the same as program /usr/bin/{n} of installed package {})", self.programs[n])),
            Some(n) if self.desktop.contains(n) => (2, format!(" (the name is the same as desktop ID {n})")),
            _ => match rel.is_none().then(|| self.rules.get(path)).flatten() {
                Some(file) => (2, format!(" (a tmpfiles rule in {file} has this path)")),
                None => (1, String::new()),
            },
        }
    }
}

pub fn audit(ctx: &Ctx) -> Result<()> {
    let g = build(ctx)?;
    let _stage = crate::report::stage("Checking the system");
    let mut report: Vec<String> = Vec::new();
    // Paths with the rank of their evidence: Evidence::of.
    let mut paths: Vec<(u8, String)> = Vec::new();
    let inventory = output(cmd("pacman", &["-Ql"]))?;
    let owned: BTreeSet<String> = inventory.lines().filter_map(|l| l.split_once(' ')).map(|(_, p)| p.trim_end_matches('/').to_string()).collect();
    let evidence = Evidence::new(&g, &inventory, &owned)?;
    let parents: BTreeSet<&str> = g.m.files.keys().flat_map(|p| Path::new(p).ancestors().skip(1).filter_map(|a| a.to_str())).collect();
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
        if owned.contains(&path) || parents.contains(path.as_str()) || path == "/" {
            continue;
        }
        if path.ends_with(".pacnew") || path.ends_with(".pacsave") {
            report.push(format!("File {}: {path}", &path[path.rfind('.').unwrap()..]));
            continue;
        }
        // A link to a file of a package, for example a fontconfig link in /etc/fonts/conf.d, is explained.
        let target = if e.path_is_symlink() { std::fs::canonicalize(e.path()).ok() } else { None };
        if target.is_some_and(|t| !t.is_dir() && owned.contains(&t.display().to_string())) {
            continue;
        }
        if e.file_type().is_dir() && evidence.containers.contains(&path) && std::fs::read_dir(e.path()).is_ok_and(|mut d| d.next().is_some()) {
            continue;
        }
        let kind = if e.file_type().is_dir() { "directory" } else { "file" };
        let (rank, note) = evidence.of(&path, None);
        paths.push((rank, format!("Unowned {kind}: {path}{note}")));
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

    let units = crate::units::plan(&g, &g, true)?;
    report.extend(units.disable.iter().map(|u| format!("Enabled unit not declared and not enabled by a vendor preset: {u}")));
    report.extend(units.enable.iter().filter(|u| g.m.units.enable.contains(*u)).map(|u| format!("Declared unit not enabled: {u}")));

    home(&g, &evidence, &mut paths)?;
    if let Some(state) = crate::secure::audit(&g, &mut report)? {
        say!("• {state}");
    }
    drop(_stage);
    let mut notes = Vec::new();
    crate::report::audit(&mut notes);
    notes.iter().for_each(|n| say!("• {n}"));
    print(&report, &paths);
    Ok(())
}

/// Shows the number of items first. Then the paths with the evidence of a
/// former claim, the paths with no evidence, the paths whose name agrees with
/// a package, and the other items, under one heading for each kind.
fn print(report: &[String], paths: &[(u8, String)]) {
    if report.is_empty() && paths.is_empty() {
        say!("• The system agrees with the configuration.");
        return;
    }
    match report.len() + paths.len() {
        1 => say!("• 1 item that is not declared or that differs from the configuration:"),
        n => say!("• {n} items that are not declared or that differ from the configuration:"),
    }
    let blocks = [
        "Paths that a feature claimed in an earlier generation:",
        "Paths with no known origin:",
        "Paths with a name, rule or DKMS match. These matches give clues about the origin:",
    ];
    for (rank, title) in blocks.iter().enumerate() {
        let items: Vec<&str> = paths.iter().filter(|p| p.0 as usize == rank).map(|p| p.1.as_str()).collect();
        if !items.is_empty() {
            say!("• {title}");
            kinds(&items);
        }
    }
    if !report.is_empty() && !paths.is_empty() {
        say!("• Other items:");
    }
    kinds(&report.iter().map(String::as_str).collect::<Vec<_>>());
    say!("• Apply corrects the managed files, the declared units, the boot files and the TPM2 keys. It never removes a package that it did not install. Declare each other item, add it to claims or audit.ignore, or remove it.");
    crate::report::gap();
    say!("• Audit done: {}.", crate::count(report.len() + paths.len(), "item"));
}

/// Shows the items under one heading for each kind.
fn kinds(lines: &[&str]) {
    let mut kinds: Vec<(&str, Vec<&str>)> = Vec::new();
    for line in lines {
        let (kind, item) = line.split_once(": ").unwrap_or((line, ""));
        match kinds.iter_mut().find(|k| k.0 == kind) {
            Some(k) => k.1.push(item),
            None => kinds.push((kind, vec![item])),
        }
    }
    for (kind, items) in &kinds {
        say!("  {kind} ({}):", items.len());
        items.iter().for_each(|i| say!("    {}", i.replace('\n', "\n    ")));
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
fn home(g: &Generation, evidence: &Evidence, paths: &mut Vec<(u8, String)>) -> Result<()> {
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
                let (rank, note) = evidence.of(&abs, Some(&rel));
                paths.push((rank, format!("Unclaimed home path: {abs}{note}")));
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
        crate::report::progress(&format!("Deleting the data of removed feature {feature}"));
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
