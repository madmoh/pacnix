use crate::generation::Generation;
use crate::{cmd, isolated, output, report, run_cmd};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;

/// The preset file of pacnix. It has priority over the vendor presets of the
/// packages. pacnix runs systemctl preset only for the declared units and the
/// units whose state it changed: when a declaration goes, the vendor preset
/// decides such a unit again, and the other units stay as they are.
pub const PRESET: &str = "/etc/systemd/system-preset/00-pacnix.preset";
/// The units whose state pacnix changed for a declaration, and the units that it masked.
const MADE: &str = "/var/lib/pacnix/units.json";

#[derive(Serialize, Deserialize, Default, Clone)]
struct Made {
    preset: BTreeSet<String>,
    mask: BTreeSet<String>,
}

impl Plan {
    /// Whether pacnix changed the state of the unit.
    pub fn made(&self, unit: &str) -> bool {
        self.made.preset.contains(unit) || self.made.mask.contains(unit)
    }
}

/// The record of the units that pacnix changed. Without the record, the units
/// of the earlier generation, which pacnix 0.10 changed.
fn made(old: &Generation) -> Made {
    let u = &old.m.units;
    std::fs::read(MADE).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_else(|| Made {
        preset: u.enable.iter().chain(&u.disable).cloned().collect(),
        mask: u.mask.iter().cloned().collect(),
    })
}
const SCRATCH: &str = "/run/pacnix/preset-root";

#[derive(Default)]
pub struct Plan {
    preset: String,
    /// The units that systemctl preset gets: the declared units and the units that pacnix changed.
    touched: Vec<String>,
    /// The declared units of units.enable and units.disable.
    listed: BTreeSet<String>,
    made: Made,
    now: BTreeSet<String>,
    /// The units of units.enable. Only these start at the apply; a unit that a
    /// vendor preset enables starts at the next boot, in its own target.
    declared: BTreeSet<String>,
    pub enable: Vec<String>,
    pub disable: Vec<String>,
    mask: Vec<String>,
    unmask: Vec<String>,
    /// The systemctl commands that failed at an earlier apply.
    retry: Vec<String>,
}

impl Plan {
    pub fn print(&self, l: &mut crate::generation::Lines) {
        let mut add = |u: &String, line: String| l.push((u.clone(), line));
        for u in &self.enable {
            add(u, match self.declared.contains(u) {
                true => format!("enable and start unit {u}"),
                false => format!("enable unit {u} (a vendor preset, it starts at the next boot)"),
            });
        }
        self.disable.iter().for_each(|u| add(u, format!("disable and stop unit {u}")));
        self.mask.iter().for_each(|u| add(u, format!("mask unit {u}")));
        self.unmask.iter().for_each(|u| add(u, format!("unmask unit {u}")));
        let retry = self.retry.iter().filter(|c| !c.starts_with("systemctl try-restart "));
        retry.for_each(|c| add(&c.rsplit(' ').next().unwrap_or(c).to_string(), format!("run {c} (the last apply did not complete it)")));
    }
}

/// The state of each unit file, from one systemctl list-unit-files in a root
/// directory. One call costs about one second on a desktop host, so callers
/// list once and filter.
fn unit_states(root: Option<&str>) -> Result<BTreeMap<String, String>> {
    let mut c = cmd("systemctl", &["list-unit-files", "--no-legend", "--no-pager"]);
    if let Some(r) = root {
        c.args(["--root", r]);
    }
    let text = output(c)?;
    Ok(text
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            Some((f.next()?.to_string(), f.next()?.to_string()))
        })
        .collect())
}

fn in_state(states: &BTreeMap<String, String>, state: &str) -> BTreeSet<String> {
    states.iter().filter(|(_, s)| *s == state).map(|(u, _)| u.clone()).collect()
}

/// The unit file of an instance: getty@tty1.service is getty@.service.
pub fn template(unit: &str) -> String {
    match (unit.split_once('@'), unit.rsplit_once('.')) {
        (Some((head, _)), Some((_, kind))) => format!("{head}@.{kind}"),
        _ => unit.to_string(),
    }
}

/// The preset file of the declared units, empty without units.
fn preset_text(g: &Generation) -> String {
    if g.m.units.enable.is_empty() && g.m.units.disable.is_empty() {
        return String::new();
    }
    let mut lines: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for u in &g.m.units.enable {
        let instance = u.split_once('@').and_then(|(_, rest)| rest.rsplit_once('.')).map(|(i, _)| i.to_string());
        lines.entry(template(u)).or_default().extend(instance.filter(|i| !i.is_empty()));
    }
    let mut text = String::from("# pacnix writes this file from the declared units.\n");
    for (unit, instances) in lines {
        text.push_str(format!("enable {unit} {}", instances.join(" ")).trim_end());
        text.push('\n');
    }
    for u in &g.m.units.disable {
        text.push_str(&format!("disable {u}\n"));
    }
    text
}

/// Runs systemctl preset for the units, or preset-all without units, in a
/// copy of the unit directories with the target preset file and the target
/// unit files. systemd computes the result, so Also=, Alias= and
/// DefaultInstance= work as they do on the live system.
fn simulate(g: &Generation, old: &Generation, preset: &str, units: Option<&[String]>) -> Result<BTreeSet<String>> {
    let root = Path::new(SCRATCH);
    let _ = std::fs::remove_dir_all(root);
    for dir in ["etc/systemd/system", "etc/systemd/system-preset", "usr/lib/systemd/system", "usr/lib/systemd/system-preset"] {
        let (src, dst) = (Path::new("/").join(dir), root.join(dir));
        std::fs::create_dir_all(dst.parent().unwrap())?;
        if src.exists() {
            run_cmd(cmd("cp", &["-a"]).arg(&src).arg(&dst))?;
        } else {
            std::fs::create_dir_all(&dst)?;
        }
    }
    for path in old.m.files.keys().filter(|p| p.starts_with("/etc/systemd/system/")) {
        let _ = std::fs::remove_file(root.join(path.trim_start_matches('/')));
    }
    for (path, spec) in g.m.files.iter().filter(|(p, _)| p.starts_with("/etc/systemd/system/")) {
        let dst = root.join(path.trim_start_matches('/'));
        std::fs::create_dir_all(dst.parent().unwrap())?;
        match &spec.target {
            Some(link) => std::os::unix::fs::symlink(link, &dst)?,
            None => std::fs::copy(&spec.source, &dst).map(|_| ())?,
        }
    }
    std::fs::write(root.join(PRESET.trim_start_matches('/')), preset)?;
    match units {
        Some(units) => run_cmd(cmd("systemctl", &["--root", SCRATCH, "preset"]).args(units))?,
        None => run_cmd(cmd("systemctl", &["--root", SCRATCH, "preset-all"]))?,
    }
    let enabled = unit_states(Some(SCRATCH)).map(|s| in_state(&s, "enabled"));
    std::fs::remove_dir_all(root)?;
    enabled
}

/// The declared units and the units that pacnix changed, whose unit file
/// exists and stays: a unit whose declared unit file goes stops with it.
fn touched(g: &Generation, old: &Generation, made: &Made, states: &BTreeMap<String, String>) -> Vec<String> {
    let gone = |u: &str| [u.to_string(), template(u)].iter().any(|f| {
        let path = format!("/etc/systemd/system/{f}");
        old.m.files.contains_key(&path) && !g.m.files.contains_key(&path)
    });
    let all = g.m.units.enable.iter().chain(&g.m.units.disable).chain(&made.preset);
    let mut units: Vec<String> = all.filter(|u| (states.contains_key(*u) || states.contains_key(&template(u))) && !gone(u)).cloned().collect();
    units.sort();
    units.dedup();
    units
}

/// The units that systemctl preset enables and disables. Declared units of
/// packages that are not installed yet are listed as well. With `all`, for the
/// audit, the result of preset-all: the state that the vendor presets give.
pub fn plan(g: &Generation, old: &Generation, all: bool) -> Result<Plan> {
    let mut plan = Plan { preset: preset_text(g), declared: g.m.units.enable.iter().cloned().collect(), ..Default::default() };
    let states = unit_states(None)?;
    let now = in_state(&states, "enabled");
    plan.made = made(old);
    plan.listed = g.m.units.enable.iter().chain(&g.m.units.disable).cloned().collect();
    plan.touched = touched(g, old, &plan.made, &states);
    let after = match (all, plan.touched.is_empty()) {
        (true, _) => simulate(g, old, &plan.preset, None)?,
        (false, true) => now.clone(),
        (false, false) => simulate(g, old, &plan.preset, Some(&plan.touched))?,
    };
    let known = in_state(&states, "disabled");
    plan.enable = after.difference(&now).cloned().collect();
    plan.enable.extend(
        g.m.units.enable.iter().filter(|u| !after.contains(*u) && !after.contains(&template(u)) && !known.contains(&template(u))).cloned(),
    );
    plan.disable = now.difference(&after).cloned().collect();
    let masked = in_state(&states, "masked");
    plan.mask = g.m.units.mask.iter().filter(|u| !masked.contains(*u)).cloned().collect();
    let masks: BTreeSet<&String> = g.m.units.mask.iter().collect();
    plan.unmask = plan.made.mask.iter().filter(|u| !masks.contains(u) && masked.contains(*u)).cloned().collect();
    // A command runs again only when the configuration still wants its result.
    let wanted = |c: &String| {
        let words: Vec<&str> = c.split(' ').collect();
        let u = words[words.len() - 1].to_string();
        let enabled = after.contains(&u) || after.contains(&template(&u));
        let exists = states.contains_key(&u) || states.contains_key(&template(&u));
        exists && match words[1] {
            "start" => enabled,
            "restart" | "try-restart" => !masks.contains(&u) && !g.m.units.disable.contains(&u),
            "stop" | "disable" => !enabled,
            "mask" => masks.contains(&u),
            "unmask" => !masks.contains(&u),
            _ => false,
        }
    };
    plan.retry = report::retries().into_iter().filter(|c| c.starts_with("systemctl ") && wanted(c)).collect();
    plan.now = now;
    Ok(plan)
}

/// A systemctl command as an isolated step: a unit that does not start or stop
/// does not stop the apply, as with switch-to-configuration of NixOS. `step`
/// is the command that the next apply runs again.
fn systemctl(step: &str, args: &[&str]) -> Result<bool> {
    isolated(step, || run_cmd(cmd("systemctl", &["--quiet"]).args(args)))
}

/// Writes the preset file and runs systemctl preset for the declared units and
/// the units of the earlier generation, then starts the declared units that it
/// enabled and stops the units that it disabled.
pub fn apply(plan: &Plan, changed: &[String]) -> Result<()> {
    for (verb, units) in [("enable", &plan.enable), ("disable", &plan.disable), ("mask", &plan.mask), ("unmask", &plan.unmask)] {
        if !units.is_empty() {
            report::progress(&format!("{verb} {}: {}", crate::count(units.len(), "unit"), report::items(&units)));
        }
    }
    // The starts that follow the presets are recorded first: after an
    // interrupt, the units are enabled and the next plan shows the starts.
    let starts: Vec<String> = plan.enable.iter().filter(|u| plan.declared.contains(*u)).map(|u| format!("systemctl start {u}")).collect();
    report::pending(starts.iter().cloned());
    if plan.preset.is_empty() {
        let _ = std::fs::remove_file(PRESET);
    } else if std::fs::read_to_string(PRESET).unwrap_or_default() != plan.preset {
        std::fs::create_dir_all(Path::new(PRESET).parent().unwrap())?;
        let tmp = format!("{PRESET}.pacnix-new");
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(plan.preset.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, PRESET)?;
    }
    if changed.iter().any(|p| p.starts_with("/etc/systemd/")) {
        run_cmd(cmd("systemctl", &["daemon-reload"]))?;
    }
    for u in &plan.unmask {
        systemctl(&format!("systemctl unmask {u}"), &["unmask", u])?;
    }
    for u in &plan.mask {
        systemctl(&format!("systemctl mask --now {u}"), &["mask", "--now", u])?;
    }
    let before = &plan.now;
    // The package transaction can add the unit files of declared units after the plan.
    let units = touched_now(plan)?;
    if !units.is_empty() {
        run_cmd(cmd("systemctl", &["preset"]).args(&units))?;
    }
    let states = unit_states(None)?;
    let after = in_state(&states, "enabled");
    // The record: the units that this apply changed, less the units that went back to their vendor preset.
    let mut made = plan.made.clone();
    made.preset.retain(|u| plan.listed.contains(u));
    made.preset.extend(before.symmetric_difference(&after).cloned());
    made.mask.extend(plan.mask.iter().cloned());
    made.mask.retain(|u| !plan.unmask.contains(u));
    std::fs::write(MADE, serde_json::to_vec(&made)?)?;
    // The list from the plan is older than the package transaction: skip the
    // units that stop_leaving stopped and the units whose package left.
    for u in before.difference(&after).filter(|u| states.contains_key(*u) && !plan.disable.contains(*u)) {
        systemctl(&format!("systemctl stop {u}"), &["stop", u])?;
    }
    let instances = plan.enable.iter().filter(|u| u.contains('@') && !u.contains("@."));
    let started: BTreeSet<&String> = after.difference(before).filter(|u| plan.declared.contains(*u)).chain(instances).collect();
    let mut done = Vec::new();
    for u in &started {
        if u.ends_with(".target") || u.contains("@.") {
            continue;
        }
        // systemd refuses to start a socket while its service runs, for example systemd-journald-audit.socket.
        if u.ends_with(".socket") && output(cmd("systemctl", &["show", "-P", "Triggers", u]))?.split_whitespace().any(|s| cmd("systemctl", &["is-active", "--quiet", s]).status().is_ok_and(|x| x.success())) {
            say!("pacnix: unit {u} starts at the next boot, because its service runs");
            continue;
        }
        report::progress(&format!("start unit {u}"));
        systemctl(&format!("systemctl start {u}"), &["start", u])?;
        done.push(format!("systemctl start {u}"));
    }
    // The recorded starts of the units that this apply did not start, for example a target, leave the record.
    for s in starts.iter().filter(|s| !done.contains(s)) {
        report::passed(s);
    }
    for u in changed_units(changed) {
        if !started.contains(&u) {
            report::progress(&format!("restart unit {u} if it runs"));
            // A unit that fails at the restart does not run, thus the next apply restarts it without the condition.
            systemctl(&format!("systemctl restart {u}"), &["try-restart", &u])?;
        }
        report::passed(&format!("systemctl try-restart {u}"));
    }
    for c in &plan.retry {
        report::progress(&format!("run {c}"));
        let args: Vec<&str> = c.split(' ').skip(1).collect();
        systemctl(c, &args)?;
    }
    Ok(())
}

fn touched_now(plan: &Plan) -> Result<Vec<String>> {
    let states = unit_states(None)?;
    let declared = plan.declared.iter().chain(&plan.touched).chain(&plan.enable).filter(|u| states.contains_key(*u) || states.contains_key(&template(u)));
    let mut units: Vec<String> = declared.cloned().collect();
    units.sort();
    units.dedup();
    Ok(units)
}

/// The units that leave, which stop before packages and files change: the
/// units that systemctl preset will disable and the units whose unit file goes away.
/// Without their unit file, systemd can no longer stop them.
pub fn leaving(plan: &Plan, deleted: &[String]) -> Vec<String> {
    let gone: Vec<String> = deleted.iter().filter(|p| p.starts_with("/etc/systemd/system/") && p.matches('/').count() == 4).cloned().collect();
    changed_units(&gone).into_iter().chain(plan.disable.iter().cloned()).collect()
}

/// Disables and stops units.
pub fn stop(units: &[String]) -> Result<()> {
    for u in units {
        systemctl(&format!("systemctl disable --now {u}"), &["disable", "--now", u])?;
    }
    Ok(())
}

/// Units whose unit file or drop-in changed. Running units restart, so that a
/// change takes effect without a reboot. A deleted unit file gives no unit.
pub fn changed_units(paths: &[String]) -> BTreeSet<String> {
    paths
        .iter()
        .filter_map(|p| p.strip_prefix("/etc/systemd/system/"))
        .filter_map(|rel| match rel.split_once('/') {
            Some((dir, _)) => dir.strip_suffix(".d").map(String::from),
            None => Path::new("/etc/systemd/system").join(rel).exists().then(|| rel.to_string()),
        })
        .filter(|u| !u.contains("@.") && !u.ends_with(".target") && !u.ends_with(".wants") && u.contains('.'))
        .collect()
}
