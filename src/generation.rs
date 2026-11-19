use std::collections::BTreeSet;
use crate::{Ctx, ask, audit, cmd, files, forward, home, packages, report, run_cmd, secure, trust, units, users};
use anyhow::{Context, Result};
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
    pub boot: BootSpec,
    #[serde(rename = "installNotes")]
    pub install_notes: BTreeMap<String, InstallNote>,
}

/// A manual step that apply shows at its end.
#[derive(Deserialize, Clone)]
pub struct InstallNote {
    pub message: String,
    pub check: Option<NoteCheck>,
}

/// A command that tells if the step of an install note is done.
#[derive(Deserialize, Clone)]
pub struct NoteCheck {
    pub user: String,
    pub command: Vec<String>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct BootSpec {
    #[serde(rename = "secureBoot")]
    pub secure_boot: SecureBoot,
    pub luks: BTreeMap<String, Luks>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct SecureBoot {
    pub enable: bool,
    #[serde(rename = "dbAdditions")]
    pub db_additions: Vec<String>,
    pub files: Vec<SignedFile>,
    pub esp: String,
}

#[derive(Deserialize, Default, Clone)]
pub struct SignedFile {
    pub path: String,
    pub output: String,
    pub required: bool,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct Luks {
    pub device: String,
    pub tpm2: Tpm2,
    #[serde(rename = "recoveryKey")]
    pub recovery_key: bool,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct Tpm2 {
    pub enable: bool,
    pub pcrs: Vec<u32>,
    pub pin: bool,
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
    /// The derivation of home.path, the packages of the home configuration.
    pub path: String,
    /// The home files of an empty home configuration of the user, relative to the home directory.
    #[serde(default)]
    pub support: Vec<String>,
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

/// Evaluates the flake and writes the generation to the Nix store. Root
/// evaluates with direct access to the store, without the nix-daemon. The
/// output of Nix shows only at an error: pacnix never writes the lock file.
pub fn build(ctx: &Ctx) -> Result<Generation> {
    let _stage = report::stage("evaluate the configuration");
    let attr = format!("{}#pacnixConfigurations.\"{}\".generation", ctx.flake, ctx.host);
    let dir = format!("/run/pacnix/eval-{}", std::process::id());
    let result = format!("{dir}/generation");
    std::fs::create_dir_all(&dir)?;
    let mut c = cmd(NIX[0], &NIX[1..]);
    c.args(["eval", "--no-warn-dirty", "--write-to", &result, &attr]);
    if Path::new(MODULES).join("flake.nix").exists() {
        c.args(["--override-input", "pacnix", &format!("path:{MODULES}")]);
    }
    let (status, lines) = forward(&mut c, usize::MAX, true, |_| false)?;
    let path = std::fs::read_to_string(&result);
    let _ = std::fs::remove_dir_all(&dir);
    if !status.success() {
        return Err(crate::failed_with(eval_error(ctx, &lines), &lines));
    }
    Generation::load(path?.trim())
}

/// The error of Nix without its trace and its command: the last error with
/// its location and the suggestion of Nix, with the paths of the flake and of
/// the pacnix modules instead of their copies in the Nix store.
fn eval_error(ctx: &Ctx, lines: &[String]) -> String {
    let start = match lines.iter().rposition(|l| l.trim_start().starts_with("error:")) {
        Some(i) if lines[i].trim() == "error:" => i + 1,
        Some(i) => i,
        None => lines.len().saturating_sub(6),
    };
    let indent = lines.get(start).map_or(0, |l| l.len() - l.trim_start().len());
    let mut text: Vec<String> = lines[start..].iter().map(|l| l[indent.min(l.len() - l.trim_start().len())..].to_string()).collect();
    if let Some(first) = text.first_mut() {
        *first = first.trim_start_matches("error: ").to_string();
    }
    let text = text.join("\n");
    if text.contains("does not provide attribute") && text.contains(&format!("pacnixConfigurations.\"{}\"", ctx.host)) {
        return format!("the flake {} has no host {} in pacnixConfigurations. Add the host to the flake, or give --host with a host of the flake", ctx.flake, ctx.host);
    }
    let mut out = String::new();
    let mut rest = text.as_str();
    while let Some(i) = rest.find("/nix/store/") {
        out.push_str(&rest[..i]);
        let path = &rest[i..];
        let end = path.find(['\'', '`', ':', ' ', '"', '\n']).unwrap_or(path.len());
        let (path, after) = path.split_at(end);
        let local = path.split_once("-source/").map(|(_, rel)| rel).and_then(|rel| [ctx.flake.as_str(), MODULES].iter().map(|d| format!("{d}/{rel}")).find(|p| Path::new(p).exists()));
        out.push_str(local.as_deref().unwrap_or(path));
        rest = after;
    }
    out.push_str(rest);
    // Nix names only the file of an option that does not exist: the line comes from the first line with the last part of the name.
    // The full name first, then its last part, for a name inside a nested set.
    let option = out.split_once("The option `").and_then(|(_, r)| r.split_once("' does not exist")).map(|(o, _)| o.to_string());
    let file = out.split_once("- In `").and_then(|(_, r)| r.split_once('\'')).map(|(f, _)| f.to_string());
    if let (Some(option), Some(file)) = (option, file) {
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let find = |o: &str| text.lines().position(|l| l.contains(o));
        if let Some(n) = find(&option).or_else(|| find(option.rsplit('.').next().unwrap_or(&option))) {
            out = out.replacen(&format!("- In `{file}'"), &format!("- In `{file}:{}'", n + 1), 1);
        }
    }
    // The suggestion of Nix for an option comes first, before the definitions.
    let first_hint = out.lines().find(|l| l.trim_start().starts_with("Did you mean")).and_then(|l| l.split_once('`')).and_then(|(_, r)| r.split_once('\'')).map(|(o, _)| format!("Did you mean `{o}'?"));
    let mut lines: Vec<&str> = out.lines().collect();
    if let Some(i) = lines.iter().position(|l| l.trim_start().starts_with("Did you mean")) {
        // Nix sorts the suggestions by their distance: the first is the closest.
        let full = lines.remove(i).trim_start();
        let hint = first_hint.as_deref().unwrap_or(full);
        let first = lines[0].split_once(" Definition values:").map(|(head, _)| head).unwrap_or(lines[0]);
        // Each location shows with its line of the file, not with the value of Nix.
        let at: Vec<String> = lines[1..]
            .iter()
            .filter_map(|l| l.trim_start().strip_prefix("- In `")?.split_once('\'').map(|(at, _)| at.to_string()))
            .map(|at| {
                let source = at.rsplit_once(':').and_then(|(f, n)| std::fs::read_to_string(f).ok()?.lines().nth(n.parse::<usize>().ok()? - 1).map(|l| l.trim().to_string()));
                format!("- in {at}{}", source.map(|s| format!(": {s}")).unwrap_or_default())
            })
            .collect();
        return format!("the configuration has an error:\n  {first}\n  {hint}{}", at.iter().map(|a| format!("\n  {a}")).collect::<String>());
    }
    format!("the configuration has an error:\n  {}", lines.join("\n").replace('\n', "\n  "))
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
    pub secure: secure::Plan,
    /// The paths of the features that leave, by feature.
    pub data: Vec<(String, Vec<String>)>,
}

impl Plan {
    fn compute(ctx: &Ctx, target: Generation) -> Result<Plan> {
        let _stage = report::stage("compute the changes");
        let old = active()?.unwrap_or_default();
        let ledger: Vec<Generation> = std::iter::once(old.clone()).chain(pending()).collect();
        // Before any change, also before the upgrade: Secure Boot must keep the system bootable.
        secure::check(&target, &old)?;
        let packages = packages::plan(ctx, &target, &old)?;
        packages::check(ctx, &packages)?;
        let users = users::plan(&target, &old)?;
        let files = files::plan(ctx, &target, &ledger)?;
        let units = units::plan(&target, &old, false)?;
        let home = home::plan(&target)?;
        let secure = secure::plan(&target, &old)?;
        let data = audit::leftovers(&old, &target)?;
        Ok(Plan { target, packages, users, files, units, home, secure, data })
    }

    /// The items and lines of the plan, by kind of change: the noun and the verb of the last line of the run.
    fn sections(&self) -> Vec<((&'static str, &'static str), Lines)> {
        let kinds = [
            ("package", ""),
            ("install reason", "set"),
            ("user", "changed"),
            ("file", "created"),
            ("file", "changed"),
            ("file", "deleted"),
            ("file", "restored"),
            ("trigger", "run"),
            ("unit", "changed"),
            ("home configuration", "activated"),
            ("home file", "created"),
            ("home file", "changed"),
            ("home file", "deleted"),
            ("home file", "restored"),
            ("Secure Boot step", ""),
            ("removed feature", ""),
        ];
        let mut s: Vec<((&'static str, &'static str), Lines)> = kinds.map(|k| (k, Vec::new())).into();
        let [packages, marks, users, created, changed, deleted, restored, triggers, units, activate, home_created, home_changed, home_deleted, home_restored, secure, data] = &mut s[..] else { unreachable!() };
        self.packages.print(&mut packages.1, &mut marks.1);
        self.users.print(&mut users.1);
        self.files.print([&mut created.1, &mut changed.1, &mut deleted.1, &mut restored.1]);
        files::print_triggers(&self.files, &mut triggers.1, &mut units.1);
        self.units.print(&mut units.1);
        self.home.print([&mut activate.1, &mut home_created.1, &mut home_changed.1, &mut home_deleted.1, &mut home_restored.1]);
        self.secure.print(&mut secure.1);
        data.1.extend(self.data.iter().map(|(f, paths)| (f.clone(), format!("delete the data of removed feature {f} (apply asks first): {}", paths.join(", ")))));
        s
    }

    /// The upgrade, the changes, the declared items that need no change, and a last line.
    fn print(&self) -> Result<()> {
        let newer = packages::newer()?;
        // The local sync databases tell what is newer since their last refresh. Apply refreshes them.
        match newer.len() {
            0 => {}
            1 => say!("pacnix: system upgrade: {} is newer", newer[0]),
            2 | 3 => say!("pacnix: system upgrade: {} are newer", newer.join(", ")),
            n => say!("pacnix: system upgrade: {n} packages are newer"),
        }
        let sections = self.sections();
        let lines: Vec<&(String, String)> = sections.iter().flat_map(|s| &s.1).collect();
        if !lines.is_empty() {
            say!("pacnix: changes ({}):", lines.len());
        }
        lines.iter().for_each(|l| say!("{}", report::plan_line(l)));
        self.secure.notes().iter().for_each(|n| say!("pacnix: note: {n}"));
        self.same();
        let upgrade = "Apply refreshes the sync databases first, thus it can also upgrade packages that this plan does not show.";
        match lines.len() {
            0 => say!("pacnix: plan done: no change. {upgrade}"),
            n => say!("pacnix: plan done: {}. The system is unchanged. {upgrade}", crate::count(n, "change")),
        }
        Ok(())
    }

    /// The declared items that need no change, with the state of each unit.
    fn same(&self) {
        let sections = self.sections();
        let changed: BTreeSet<&String> = sections.iter().flat_map(|s| &s.1).map(|l| &l.0).collect();
        let m = &self.target.m;
        // What the removal of the declaration does, from the records of pacnix.
        let units = |list: &Vec<String>, state: &str| {
            let by = |u: &String| if self.units.made(u) { "by pacnix; vendor preset if you remove the declaration" } else { "before pacnix; stays if you remove the declaration" };
            list.iter().map(|u| (u.clone(), format!("{u} ({state} {})", by(u)))).collect::<Vec<_>>()
        };
        let plain = |list: Vec<&String>| list.into_iter().map(|i| (i.clone(), i.clone())).collect::<Vec<_>>();
        let packages = m.packages.repo.iter().chain(&m.packages.aur).chain(m.packages.local.keys());
        let declared = [
            ("package", packages.map(|p| (p.clone(), format!("{p} (installed {})", if self.packages.made(p) { "by pacnix; removed if you remove the declaration" } else { "before pacnix; stays if you remove the declaration" }))).collect()),
            ("user", plain(m.users.users.keys().collect())),
            ("file", plain(m.files.keys().collect())),
            ("unit", [units(&m.units.enable, "enabled"), units(&m.units.disable, "disabled"), units(&m.units.mask, "masked")].concat()),
        ];
        let same: Vec<String> = declared
            .into_iter()
            .map(|(noun, items)| (noun, items.into_iter().filter(|i| !changed.contains(&i.0)).map(|i| i.1).collect::<Vec<_>>()))
            .filter(|(_, items)| !items.is_empty())
            .map(|(noun, items)| if items.len() > 3 { crate::count(items.len(), noun) } else { items.join(", ") })
            .collect();
        if !same.is_empty() {
            say!("pacnix: already as declared: {}", same.join(", "));
        }
    }
}

/// The lines of a kind of change, each with its item: a package, a path, a unit or a name.
pub type Lines = Vec<(String, String)>;

/// The file and line of the first line in the *.nix files of a local flake that contains the text.
pub fn locate(ctx: &Ctx, text: &str) -> Option<String> {
    if ctx.flake.contains(':') {
        return None;
    }
    let files = walkdir::WalkDir::new(&ctx.flake).into_iter().filter_entry(|e| e.file_name() != ".git").filter_map(|e| e.ok());
    files.filter(|e| e.path().extension().is_some_and(|x| x == "nix")).find_map(|e| {
        let n = std::fs::read_to_string(e.path()).ok()?.lines().position(|l| l.contains(text))?;
        Some(format!("{}:{}", e.path().display(), n + 1))
    })
}

pub fn plan(ctx: &Ctx) -> Result<()> {
    Plan::compute(ctx, build(ctx)?)?.print()
}

/// Makes the system agree with the configuration: pacman -Syu, the rebuild
/// of changed AUR packages, then the plan. The questions of the run come
/// before any long work, see `preflight`: the removals and the AUR builds that
/// the plan shows now, the changes of the AUR packages that upgrade, and the
/// Arch Linux news since the last upgrade. Offline, the sync databases and the
/// AUR packages stay as they are.
pub fn apply(ctx: &Ctx, offline: bool) -> Result<()> {
    let target = build(ctx)?;
    let (mut plan, mut read) = (None, false);
    if !offline {
        let news = std::thread::spawn(trust::news);
        let mut before = Plan::compute(ctx, target.clone())?;
        files::check(ctx, &before.target, &before.files)?;
        let builds = packages::upgrade_builds(&target)?;
        let fetched = packages::fetch_builds(&before.target, &before.packages)?;
        let news = news.join().unwrap_or_default();
        read = news.is_some();
        let asks = [packages::removals(&before.packages), builds.asks.clone(), fetched.asks.clone(), news.unwrap_or_default()].concat();
        preflight(ctx, &mut before, &asks)?;
        // When the upgrade changes no package and no sync database, the plan and its builds stay the same.
        let (state, built) = (packages::databases(), builds.queue.is_empty());
        report::changing();
        packages::upgrade(&target, builds)?;
        plan = (built && packages::databases() == state).then_some((before, fetched));
    }
    if offline {
        say!("pacnix: system upgrade: skipped, because of --offline");
    }
    activate(ctx, target.clone(), plan)?;
    commit(&target)?;
    trust::news_read(read);
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

/// The questions of the run, one time before the long work, also for the
/// retries: the one question, the passwords of the new users, the passphrases
/// and PINs of the LUKS devices, then the recovery keys, which pacnix enrolls
/// and shows at once. After them, a new item of the question stops pacnix,
/// and the late steps do not ask: they show the next command.
fn preflight(ctx: &Ctx, plan: &mut Plan, asks: &[String]) -> Result<bool> {
    let delete = ask(asks, &audit::asks(&plan.data), &ctx.opts)?;
    if !crate::closed() {
        users::ask_passwords(&plan.users, &ctx.opts)?;
        secure::ask_secrets(&plan.target, &plan.secure)?;
        secure::recover(&mut plan.secure)?;
        crate::close();
    }
    Ok(delete)
}

/// Brings the system to the target generation. The profile is not changed.
fn activate(ctx: &Ctx, target: Generation, plan: Option<(Plan, packages::Builds)>) -> Result<()> {
    packages::recover_lock()?;
    let (mut plan, builds) = match plan {
        Some(plan) => plan,
        None => {
            let plan = Plan::compute(ctx, target)?;
            files::check(ctx, &plan.target, &plan.files)?;
            let builds = packages::fetch_builds(&plan.target, &plan.packages)?;
            (plan, builds)
        }
    };
    let asks = [packages::removals(&plan.packages), builds.asks.clone()].concat();
    let delete = preflight(ctx, &mut plan, &asks)?;
    // The last line names the target of a declared link.
    let link = |p: String| match plan.target.m.files.get(&p).and_then(|f| f.target.clone()) {
        Some(t) => format!("{p} (link to {t})"),
        None => p,
    };
    let p = &plan.target.m.packages;
    report::declared(p.repo.iter().chain(&p.aur).chain(p.local.keys()).cloned().collect());
    report::install_notes(plan.target.m.install_notes.clone());
    let mut kinds: Vec<_> = plan.sections().into_iter().skip(1).map(|(kind, lines)| (kind, lines.into_iter().map(|l| link(l.0)).collect())).collect();
    // The recovery keys of the start of the run left the plan.
    kinds.push((("recovery key", "enrolled"), secure::recovered()));
    report::planned(kinds);
    report::changing();
    let current = std::fs::canonicalize(PENDING).ok();
    if current.as_deref() != Some(Path::new(&plan.target.path)) {
        std::fs::create_dir_all(STATE)?;
        let _ = std::fs::remove_file(PENDING);
        run_cmd(cmd("nix-store", &["--add-root", PENDING, "--realise", &plan.target.path]))?;
    }
    let leaving = units::leaving(&plan.units, &plan.files.deleted());
    let skip = packages::prepare(&plan.target, &plan.packages, builds)?;
    units::stop(&leaving)?;
    packages::apply(&plan.packages, &skip)?;
    users::apply(&plan.users)?;
    let changed = files::apply(ctx, &plan.target, &plan.files)?;
    files::run_triggers(&plan.files, &changed)?;
    secure::apply(&plan.target, &plan.secure)?;
    units::apply(&plan.units, &changed)?;
    home::apply(&plan.target, &plan.home)?;
    keep_or_delete(&plan.target, &plan.data, delete)?;
    report::save_retries();
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
    let key = current.m.packages.server_key.clone().map(|k| k.replace(' ', "").to_uppercase()).filter(|k| run_cmd(cmd("pacman-key", &["--list-keys", k])).is_ok());
    let mut asks = audit::asks(&data);
    if !groups.is_empty() {
        asks.push(format!("Delete the users {} and the groups {}.", users.join(" "), groups.join(" ")));
    }
    asks.extend(key.iter().map(|k| format!("Delete the key {k} of packages.server in the pacman keyring.")));
    let removals = if remove.is_empty() { vec![] } else { vec![format!("Remove {}: {}.", crate::count(remove.len(), "package"), remove.join(" "))] };
    let delete = ask(&removals, &asks, &ctx.opts)?;
    report::changing();
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
            run_cmd(cmd("pacman-key", &["--delete", &k]))?;
        }
    }
    Ok(())
}
