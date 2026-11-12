use crate::{Ctx, cmd, files, home, output, packages, run_cmd, units};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

pub const PROFILE: &str = "/nix/var/nix/profiles/pacnix";
pub const STATE: &str = "/var/lib/pacnix";
const PENDING: &str = "/var/lib/pacnix/pending";
const FIRSTBOOT: &str = "/var/lib/pacnix/firstboot";
const MODULES: &str = "/usr/share/pacnix";

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
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct Packages {
    pub repo: Vec<String>,
    pub aur: Vec<String>,
    pub local: BTreeMap<String, String>,
    pub providers: BTreeMap<String, String>,
    pub server: Option<String>,
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
    pub mode: String,
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

/// Evaluates the flake and writes the generation to the Nix store.
pub fn build(ctx: &Ctx) -> Result<Generation> {
    let attr = format!("{}#pacnixConfigurations.\"{}\".generation", ctx.flake, ctx.host);
    let mut c = nix(&["eval", "--raw", &attr]);
    if Path::new(MODULES).join("flake.nix").exists() {
        c.args(["--override-input", "pacnix", &format!("path:{MODULES}")]);
    }
    let path = output(&mut c).context("evaluate the configuration")?;
    Generation::load(path.trim())
}

pub fn nix(args: &[&str]) -> std::process::Command {
    let mut c = cmd("nix", &["--extra-experimental-features", "nix-command flakes"]);
    c.args(args);
    c
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
    pub files: files::Plan,
    pub units: units::Plan,
    pub home: home::Plan,
}

impl Plan {
    fn compute(ctx: &Ctx, target: Generation) -> Result<Plan> {
        let old = active()?.unwrap_or_default();
        let ledger: Vec<Generation> = std::iter::once(old.clone()).chain(pending()).collect();
        let packages = packages::plan(ctx, &target)?;
        let files = files::plan(ctx, &target, &ledger)?;
        let units = units::plan(&target, &old)?;
        let home = home::plan(&target)?;
        Ok(Plan { target, packages, files, units, home })
    }

    fn print(&self) {
        let mut lines = Vec::new();
        self.packages.print(&mut lines);
        self.files.print(&mut lines);
        files::print_triggers(&self.files, &mut lines);
        self.units.print(&mut lines);
        self.home.print(&mut lines);
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

pub fn apply(ctx: &Ctx) -> Result<()> {
    let target = build(ctx)?;
    activate(ctx, target.clone())?;
    commit(&target)
}

/// Makes the generation the active generation of the profile.
fn commit(target: &Generation) -> Result<()> {
    run_cmd(cmd("nix-env", &["--profile", PROFILE, "--set", &target.path]))?;
    let _ = std::fs::remove_file(PENDING);
    Ok(())
}

/// Brings the system to the target generation. The profile is not changed.
fn activate(ctx: &Ctx, target: Generation) -> Result<()> {
    println!("Generation {}", target.path);
    packages::recover_lock()?;
    let mut plan = Plan::compute(ctx, target)?;
    if packages::needs_bootstrap(&plan.target) {
        // The checks that do not need aurutils run before it is built.
        packages::check(ctx, &plan.packages)?;
        files::check(ctx, &plan.files)?;
        packages::bootstrap()?;
        plan = Plan::compute(ctx, plan.target)?;
    }
    plan.print();
    packages::check(ctx, &plan.packages)?;
    files::check(ctx, &plan.files)?;
    let current = std::fs::canonicalize(PENDING).ok();
    if current.as_deref() != Some(Path::new(&plan.target.path)) {
        std::fs::create_dir_all(STATE)?;
        let _ = std::fs::remove_file(PENDING);
        run_cmd(cmd("nix-store", &["--add-root", PENDING, "--realise", &plan.target.path]).stdout(std::process::Stdio::null()))?;
    }
    let leaving = units::leaving(&plan.units, &plan.files.deleted());
    packages::prepare(&plan.target, &plan.packages, !leaving.is_empty())?;
    units::stop(&leaving)?;
    packages::apply(&plan.packages)?;
    let changed = files::apply(ctx, &plan.target, &plan.files)?;
    files::run_triggers(&plan.files, &changed)?;
    units::apply(&plan.units, &changed)?;
    home::apply(&plan.target, &plan.home)?;
    // In a chroot, pacnix-firstboot.service applies again on the first boot
    // to do the deferred work. An apply on the running system completes it.
    if packages::booted() {
        let _ = std::fs::remove_file(FIRSTBOOT);
    } else {
        std::fs::write(FIRSTBOOT, format!("ARGS=--flake {} --host {}\n", ctx.flake, ctx.host))?;
        println!("pacnix: note: pacnix-firstboot.service completes the configuration on the first boot");
    }
    Ok(())
}

pub fn upgrade(ctx: &Ctx) -> Result<()> {
    let target = build(ctx)?;
    packages::upgrade(ctx, &target)?;
    activate(ctx, target.clone())?;
    commit(&target)
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
    activate(ctx, previous)?;
    run_cmd(cmd("nix-env", &["--profile", PROFILE, "--switch-generation", &number.to_string()]))?;
    let _ = std::fs::remove_file(PENDING);
    Ok(())
}
