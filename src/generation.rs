use crate::{Ctx, ask, audit, cmd, failed, files, forward, home, packages, report, run_cmd, run_quiet, trust, units, users};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

pub const PROFILE: &str = "/nix/var/nix/profiles/pacnix";
pub const STATE: &str = "/var/lib/pacnix";
const PENDING: &str = "/var/lib/pacnix/pending";
const MODULES: &str = "/usr/share/pacnix";
const NIX: [&str; 3] = ["nix", "--extra-experimental-features", "nix-command flakes"];

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct Manifest {
    pub packages: Packages,
    pub files: BTreeMap<String, FileSpec>,
    pub units: Units,
    pub triggers: BTreeMap<String, Trigger>,
    pub claims: BTreeMap<String, Claim>,
    pub audit: AuditSpec,
    pub home: BTreeMap<String, Home>,
    pub users: Accounts,
    pub mounts: BTreeMap<String, String>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct Accounts {
    pub users: BTreeMap<String, User>,
    pub groups: BTreeMap<String, Group>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct User {
    pub uid: Option<u32>,
    pub group: Option<String>,
    #[serde(rename = "extraGroups")]
    pub extra_groups: Vec<String>,
    pub shell: Option<String>,
    pub home: String,
    pub description: Option<String>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct Group {
    pub gid: Option<u32>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct Packages {
    pub repo: Vec<String>,
    pub aur: Vec<String>,
    pub local: BTreeMap<String, String>,
    pub providers: BTreeMap<String, String>,
    pub versions: BTreeMap<String, String>,
    pub server: Option<String>,
    #[serde(rename = "serverKey")]
    pub server_key: Option<String>,
    pub serve: Serve,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct Serve {
    pub aur: Vec<String>,
    pub local: BTreeMap<String, String>,
}

#[derive(Deserialize, Clone, PartialEq)]
pub struct FileSpec {
    pub source: String,
    pub target: Option<String>,
    pub mode: Option<String>,
    pub user: String,
    pub group: String,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct Units {
    pub enable: Vec<String>,
    pub disable: Vec<String>,
    pub mask: Vec<String>,
}

#[derive(Deserialize, Clone)]
pub struct Trigger {
    pub paths: Vec<String>,
    pub command: String,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct Claim {
    pub system: Vec<String>,
    pub home: Vec<String>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct AuditSpec {
    pub ignore: Vec<String>,
}

#[derive(Deserialize, Clone)]
pub struct Home {
    pub directory: String,
    pub drv: String,
}

#[derive(Clone, Default)]
pub struct Generation {
    pub path: String,
    pub m: Manifest,
}

impl Generation {
    pub fn load(path: &str) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("read generation {path}"))?;
        let m = serde_json::from_str(&text).with_context(|| format!("parse generation {path}"))?;
        Ok(Generation { path: path.to_string(), m })
    }
}

/// Evaluates the flake and writes the generation to the Nix store. The notes
/// of Nix about a dirty Git tree and about the lock file are not shown: the
/// override of the pacnix input always changes the lock file, and pacnix never
/// writes it. Nix evaluates as the owner of the flake through the nix-daemon,
/// thus the Nix cache and the Git checks of the flake are those of the owner.
pub fn build(ctx: &Ctx) -> Result<Generation> {
    report::progress("evaluate the configuration");
    let attr = format!("{}#pacnixConfigurations.\"{}\".generation", ctx.flake, ctx.host);
    let dir = format!("/run/pacnix/eval-{}", std::process::id());
    let result = format!("{dir}/generation");
    std::fs::create_dir_all(&dir)?;
    let mut c = match owner(&ctx.flake) {
        Some((uid, gid, user, home)) => {
            std::os::unix::fs::chown(&dir, Some(uid), Some(gid))?;
            let mut c = cmd("runuser", &["-u", &user, "--", "env", "-i", &format!("HOME={home}"), "PATH=/usr/local/bin:/usr/bin"]);
            c.args(NIX);
            c
        }
        None => cmd(NIX[0], &NIX[1..]),
    };
    c.args(["eval", "--no-warn-dirty", "--write-to", &result, &attr]);
    if Path::new(MODULES).join("flake.nix").exists() {
        c.args(["--override-input", "pacnix", &format!("path:{MODULES}")]);
    }
    let mut lock_note = false;
    let (status, lines) = forward(&mut c, 20, true, |l| {
        lock_note = l.starts_with("warning: not writing modified lock file") || (lock_note && (l.starts_with("• ") || l.starts_with("    ")));
        !lock_note
    })?;
    let path = std::fs::read_to_string(&result);
    let _ = std::fs::remove_dir_all(&dir);
    if !status.success() {
        return Err(failed(&c, status, &lines)).context("evaluate the configuration");
    }
    Generation::load(path?.trim())
}

/// The user that owns the directory of the flake, when it is not root and the
/// nix-daemon accepts connections. Before Nix makes its store, root evaluates.
fn owner(flake: &str) -> Option<(u32, u32, String, String)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(flake).ok()?;
    if meta.uid() == 0 || !Path::new("/nix/var/nix/daemon-socket/socket").exists() || !Path::new("/nix/store").exists() {
        return None;
    }
    let entry = crate::output(cmd("getent", &["passwd", &meta.uid().to_string()])).ok()?;
    let fields: Vec<&str> = entry.trim().split(':').collect();
    Some((meta.uid(), fields.get(3)?.parse().ok()?, fields[0].to_string(), fields.get(5)?.to_string()))
}

/// The generation that the profile points to.
pub fn active() -> Result<Option<Generation>> {
    match std::fs::canonicalize(PROFILE) {
        Ok(p) => Ok(Some(Generation::load(&p.display().to_string())?)),
        Err(_) => Ok(None),
    }
}

/// A generation whose apply did not complete. Its files count as managed.
pub fn pending() -> Option<Generation> {
    let p = std::fs::canonicalize(PENDING).ok()?;
    Generation::load(&p.display().to_string()).ok()
}

/// All generations in the profile, oldest first, as (number, generation).
pub fn history() -> Result<Vec<(u64, Generation)>> {
    let mut list = Vec::new();
    let dir = Path::new(PROFILE).parent().unwrap();
    for e in std::fs::read_dir(dir)? {
        let name = e?.file_name().to_string_lossy().into_owned();
        if let Some(n) = name.strip_prefix("pacnix-").and_then(|s| s.strip_suffix("-link")).and_then(|s| s.parse().ok()) {
            if let Ok(p) = std::fs::canonicalize(dir.join(&name)) {
                list.push((n, Generation::load(&p.display().to_string())?));
            }
        }
    }
    list.sort_by_key(|(n, _)| *n);
    Ok(list)
}

pub struct Plan {
    pub target: Generation,
    pub packages: packages::Plan,
    pub users: users::Plan,
    pub files: files::Plan,
    pub units: units::Plan,
    pub home: home::Plan,
    /// The paths of the features that leave, by feature.
    pub data: Vec<(String, Vec<String>)>,
}

impl Plan {
    fn compute(ctx: &Ctx, target: Generation) -> Result<Plan> {
        let old = active()?.unwrap_or_default();
        let ledger: Vec<Generation> = std::iter::once(old.clone()).chain(pending()).collect();
        let packages = packages::plan(ctx, &target)?;
        let users = users::plan(&target, &old)?;
        let files = files::plan(ctx, &target, &ledger)?;
        let units = units::plan(&target, &old)?;
        let home = home::plan(&target)?;
        let data = audit::leftovers(&old, &target)?;
        Ok(Plan { target, packages, users, files, units, home, data })
    }

    fn print(&self) {
        let mut lines = Vec::new();
        self.packages.print(&mut lines);
        self.users.print(&mut lines);
        self.files.print(&mut lines);
        files::print_triggers(&self.files, &mut lines);
        self.units.print(&mut lines);
        self.home.print(&mut lines);
        lines.extend(self.data.iter().map(|(f, paths)| format!("delete the data of removed feature {f}: {}", paths.join(", "))));
        if lines.is_empty() {
            println!("No changes.");
        }
        for l in lines {
            println!("{l}");
        }
    }
}

pub fn plan(ctx: &Ctx) -> Result<()> {
    let target = build(ctx)?;
    println!("Generation {}", target.path);
    Plan::compute(ctx, target)?.print();
    Ok(())
}

/// Makes the system agree with the configuration: pacman -Syu, the rebuild
/// of changed AUR packages, then the plan. The one question of the run comes
/// before any change: the removals that the plan shows now, the changes of the
/// AUR packages that upgrade, and the Arch Linux news since the last upgrade.
/// Offline, the sync databases and the AUR packages stay as they are.
pub fn apply(ctx: &Ctx, offline: bool) -> Result<()> {
    let target = build(ctx)?;
    let mut plan = None;
    if !offline {
        let news = std::thread::spawn(trust::news);
        let before = Plan::compute(ctx, target.clone())?;
        let builds = packages::upgrade_builds(&target)?;
        let news = news.join().unwrap_or_default();
        ask(&[packages::removals(&before.packages), builds.asks.clone(), news].concat(), &audit::asks(&before.data), &ctx.opts)?;
        users::ask_passwords(&before.users, &ctx.opts)?;
        // When the upgrade changes no package and no sync database, the plan stays the same.
        let (state, built) = (packages::databases(), builds.queue.is_empty());
        packages::upgrade(&target, builds)?;
        plan = (built && packages::databases() == state).then_some(before);
    }
    activate(ctx, target.clone(), plan)?;
    commit(&target)?;
    trust::news_read(!offline);
    Ok(())
}

/// Makes the generation the active generation of the profile.
fn commit(target: &Generation) -> Result<()> {
    // The changes of the apply, for example deleted files, are on the disk
    // before the profile records the generation, also after a power loss.
    unsafe { libc::sync() };
    run_cmd(cmd("nix-env", &["--profile", PROFILE, "--set", &target.path]))?;
    let _ = std::fs::remove_file(PENDING);
    Ok(())
}

/// Brings the system to the target generation. The profile is not changed.
fn activate(ctx: &Ctx, target: Generation, plan: Option<Plan>) -> Result<()> {
    println!("Generation {}", target.path);
    packages::recover_lock()?;
    let plan = match plan {
        Some(plan) => plan,
        None => Plan::compute(ctx, target)?,
    };
    plan.print();
    packages::check(ctx, &plan.packages)?;
    files::check(ctx, &plan.target, &plan.files)?;
    let builds = packages::fetch_builds(&plan.target, &plan.packages)?;
    let delete = ask(&[packages::removals(&plan.packages), builds.asks.clone()].concat(), &audit::asks(&plan.data), &ctx.opts)?;
    users::ask_passwords(&plan.users, &ctx.opts)?;
    let current = std::fs::canonicalize(PENDING).ok();
    if current.as_deref() != Some(Path::new(&plan.target.path)) {
        std::fs::create_dir_all(STATE)?;
        let _ = std::fs::remove_file(PENDING);
        run_quiet(cmd("nix-store", &["--add-root", PENDING, "--realise", &plan.target.path]))?;
    }
    let leaving = units::leaving(&plan.units, &plan.files.deleted());
    packages::prepare(&plan.target, &plan.packages, builds)?;
    units::stop(&leaving)?;
    packages::apply(&plan.packages)?;
    users::apply(&plan.users)?;
    let changed = files::apply(ctx, &plan.target, &plan.files)?;
    files::run_triggers(&plan.files, &changed)?;
    units::apply(&plan.units, &changed)?;
    home::apply(&plan.target, &plan.home)?;
    keep_or_delete(&plan.target, &plan.data, delete)?;
    Ok(())
}

pub fn rollback(ctx: &Ctx) -> Result<()> {
    let current = std::fs::canonicalize(PROFILE).context("no active generation")?;
    let history = history()?;
    let index = history
        .iter()
        .position(|(_, g)| Path::new(&g.path) == current)
        .context("the active generation is not in the profile")?;
    if index == 0 {
        bail!("no generation is older than the active generation");
    }
    let (number, previous) = history[index - 1].clone();
    println!("Roll back to generation {number}");
    activate(ctx, previous, None)?;
    run_cmd(cmd("nix-env", &["--profile", PROFILE, "--switch-generation", &number.to_string()]))?;
    let _ = std::fs::remove_file(PENDING);
    Ok(())
}

fn keep_or_delete(target: &Generation, data: &[(String, Vec<String>)], delete: bool) -> Result<()> {
    if delete {
        return audit::delete(target, data);
    }
    if !data.is_empty() {
        report::last("pacnix: the data of the removed features stays. pacnix audit shows it.".to_string());
    }
    Ok(())
}

/// Removes pacnix and keeps the system as configured: the declared files,
/// units and packages stay, and the Home Manager files become copies outside
/// the Nix store. The packages pacnix, aurutils and nix leave with pacman.
/// Then the footprint of pacnix, the feature pacnix, goes as the data of a
/// removed feature, with the users and groups of pacnix and Nix and the key of
/// packages.server in the pacman keyring.
pub fn uninstall(ctx: &Ctx) -> Result<()> {
    let current = active()?.context("no active generation")?;
    let mut target = current.clone();
    target.m.claims.remove("pacnix");
    let data = audit::leftovers(&current, &target)?;
    let remove = packages::uninstall_plan()?;
    // The users of pacnix and of the Nix builds have the group pacnix or nixbld as primary group.
    let gid = |g: &str| crate::output_any(cmd("getent", &["group", g])).ok()?.split(':').nth(2)?.parse::<u32>().ok();
    let groups: Vec<&str> = ["pacnix", "nixbld"].into_iter().filter(|g| gid(g).is_some()).collect();
    let gids: Vec<u32> = groups.iter().filter_map(|g| gid(g)).collect();
    let passwd = std::fs::read_to_string("/etc/passwd")?;
    let users: Vec<&str> = passwd.lines().map(|l| l.split(':').collect::<Vec<_>>()).filter(|f| f.get(3).and_then(|g| g.parse().ok()).is_some_and(|g| gids.contains(&g))).map(|f| f[0]).collect();
    let key = current.m.packages.server_key.clone().map(|k| k.replace(' ', "").to_uppercase()).filter(|k| run_quiet(cmd("pacman-key", &["--list-keys", k])).is_ok());
    let mut asks = audit::asks(&data);
    if !groups.is_empty() {
        asks.push(format!("Delete the users {} and the groups {}.", users.join(" "), groups.join(" ")));
    }
    asks.extend(key.iter().map(|k| format!("Delete the key {k} of packages.server in the pacman keyring.")));
    let removals = if remove.is_empty() { vec![] } else { vec![format!("Remove {} packages: {}.", remove.len(), remove.join(" "))] };
    let delete = ask(&removals, &asks, &ctx.opts)?;
    home::detach(&current)?;
    packages::uninstall(&remove)?;
    keep_or_delete(&target, &data, delete)?;
    if delete {
        for u in &users {
            run_cmd(cmd("userdel", &[u]))?;
        }
        for g in groups {
            if gid(g).is_some() {
                run_cmd(cmd("groupdel", &[g]))?;
            }
        }
        if let Some(k) = key {
            run_quiet(cmd("pacman-key", &["--delete", &k]))?;
        }
    }
    Ok(())
}
