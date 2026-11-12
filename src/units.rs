use crate::generation::Generation;
use crate::{cmd, output, run_cmd};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;

/// The preset file of pacnix. It has priority over the vendor presets of the
/// packages, so the enabled units are the declared units plus the units that
/// vendor presets enable, less the declared disabled units. The Arch preset
/// 99-default.preset disables the rest.
pub const PRESET: &str = "/etc/systemd/system-preset/00-pacnix.preset";
const SCRATCH: &str = "/run/pacnix/preset-root";

#[derive(Default)]
pub struct Plan {
    preset: String,
    now: BTreeSet<String>,
    pub enable: Vec<String>,
    pub disable: Vec<String>,
    mask: Vec<String>,
    unmask: Vec<String>,
}

impl Plan {
    pub fn print(&self, l: &mut Vec<String>) {
        l.extend(self.enable.iter().map(|u| format!("enable and start unit {u}")));
        l.extend(self.disable.iter().map(|u| format!("disable and stop unit {u}")));
        l.extend(self.mask.iter().map(|u| format!("mask unit {u}")));
        l.extend(self.unmask.iter().map(|u| format!("unmask unit {u}")));
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

fn preset_text(g: &Generation) -> String {
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

/// Runs preset-all in a copy of the unit directories with the target preset
/// file and the target unit files. systemd computes the result, so Also=,
/// Alias= and DefaultInstance= work as they do on the live system.
fn simulate(g: &Generation, old: &Generation, preset: &str) -> Result<BTreeSet<String>> {
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
    let mut c = cmd("systemctl", &["--root", SCRATCH, "preset-all"]);
    run_cmd(c.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()))?;
    let enabled = unit_states(Some(SCRATCH)).map(|s| in_state(&s, "enabled"));
    std::fs::remove_dir_all(root)?;
    enabled
}

/// The units that preset-all enables and disables. Declared units of packages
/// that are not installed yet are listed as well.
pub fn plan(g: &Generation, old: &Generation) -> Result<Plan> {
    let mut plan = Plan { preset: preset_text(g), ..Default::default() };
    let states = unit_states(None)?;
    let now = in_state(&states, "enabled");
    let after = simulate(g, old, &plan.preset)?;
    let known = in_state(&states, "disabled");
    plan.enable = after.difference(&now).cloned().collect();
    plan.enable.extend(
        g.m.units.enable.iter().filter(|u| !after.contains(*u) && !after.contains(&template(u)) && !known.contains(&template(u))).cloned(),
    );
    plan.disable = now.difference(&after).cloned().collect();
    let masked = in_state(&states, "masked");
    plan.mask = g.m.units.mask.iter().filter(|u| !masked.contains(*u)).cloned().collect();
    let masks: BTreeSet<&String> = g.m.units.mask.iter().collect();
    plan.unmask = old.m.units.mask.iter().filter(|u| !masks.contains(u) && masked.contains(*u)).cloned().collect();
    plan.now = now;
    Ok(plan)
}

/// Writes the preset file and runs systemctl preset-all, then starts the units
/// that it enabled and stops the units that it disabled.
pub fn apply(plan: &Plan, changed: &[String]) -> Result<()> {
    if std::fs::read_to_string(PRESET).unwrap_or_default() != plan.preset {
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
        run_cmd(cmd("systemctl", &["unmask", u]))?;
    }
    for u in &plan.mask {
        run_cmd(cmd("systemctl", &["mask", "--now", u]))?;
    }
    let before = &plan.now;
    run_cmd(cmd("systemctl", &["preset-all"]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()))?;
    let states = unit_states(None)?;
    let after = in_state(&states, "enabled");
    // The list from the plan is older than the package transaction: skip the
    // units that stop_leaving stopped and the units whose package left.
    for u in before.difference(&after).filter(|u| states.contains_key(*u) && !plan.disable.contains(*u)) {
        run_cmd(cmd("systemctl", &["stop", u]))?;
    }
    let instances = plan.enable.iter().filter(|u| u.contains('@') && !u.contains("@."));
    let started: BTreeSet<&String> = after.difference(before).chain(instances).collect();
    for u in &started {
        if !u.ends_with(".target") && !u.contains("@.") {
            run_cmd(cmd("systemctl", &["start", u]))?;
        }
    }
    for u in changed_units(changed).iter().filter(|u| !started.contains(u)) {
        println!("pacnix: restart unit {u} if it runs");
        run_cmd(cmd("systemctl", &["try-restart", u]))?;
    }
    Ok(())
}

/// The units that leave, which stop before packages and files change: the
/// units that preset-all will disable and the units whose unit file goes away.
/// Without their unit file, systemd can no longer stop them.
pub fn leaving(plan: &Plan, deleted: &[String]) -> Vec<String> {
    let gone: Vec<String> = deleted.iter().filter(|p| p.starts_with("/etc/systemd/system/") && p.matches('/').count() == 4).cloned().collect();
    changed_units(&gone).into_iter().chain(plan.disable.iter().cloned()).collect()
}

/// Disables and stops units.
pub fn stop(units: &[String]) -> Result<()> {
    for u in units {
        run_cmd(cmd("systemctl", &["disable", "--now", u]))?;
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
