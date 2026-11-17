use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Seek};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

const PACMAN_LOG: &str = "/var/log/pacman.log";
/// The install notes of the packages: the last scriptlet output of each
/// package, and the notes of the last run that had notes, for the audit.
const NOTES: &str = "/var/lib/pacnix/notes.json";
/// The isolated steps that failed and that the next apply does again.
const RETRY: &str = "/var/lib/pacnix/retry.json";

/// The current step and its start, and the size of the pacman log at the
/// start of the run.
static RUN: Mutex<Option<(String, Instant, u64)>> = Mutex::new(None);
/// Lines for the end of the summary, for example the next command after an install.
static LAST: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// The isolated steps that failed in this attempt.
static FAILED: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Notes {
    scriptlets: BTreeMap<String, String>,
    last: BTreeMap<String, String>,
}

/// Starts a run that changes the system: progress lines and a summary at the end.
pub fn begin() {
    let offset = std::fs::metadata(PACMAN_LOG).map_or(0, |m| m.len());
    *RUN.lock().unwrap() = Some((String::new(), Instant::now(), offset));
}

/// Writes a step of the run to the output.
pub fn progress(step: &str) {
    let mut guard = RUN.lock().unwrap();
    let Some((s, start, _)) = guard.as_mut() else { return };
    println!("pacnix: {step}");
    *s = step.to_string();
    *start = Instant::now();
}

/// Adds a line to the end of the summary.
pub fn last(line: String) {
    LAST.lock().unwrap().push(line);
}

/// The failed steps of earlier applies that the next apply does again.
pub fn retries() -> std::collections::BTreeSet<String> {
    std::fs::read(RETRY).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn store(steps: &std::collections::BTreeSet<String>) {
    if let Ok(b) = serde_json::to_vec_pretty(steps) {
        let _ = std::fs::create_dir_all(crate::generation::STATE).and_then(|_| std::fs::write(RETRY, b));
    }
}

/// True when the step failed at an earlier apply and did not succeed after it.
pub fn retry(step: &str) -> bool {
    retries().contains(step)
}

/// Records a step that succeeded. Returns true.
pub fn passed(step: &str) -> bool {
    let mut steps = retries();
    if steps.remove(step) {
        store(&steps);
    }
    true
}

/// Records and shows a step that failed, so that the next apply does it again,
/// also when this run stops later. Returns false.
pub fn failure(step: &str, reason: &str) -> bool {
    println!("pacnix: error: {step}: {reason}");
    println!("pacnix: the apply continues without this step");
    FAILED.lock().unwrap().push(step.to_string());
    let mut steps = retries();
    if steps.insert(step.to_string()) {
        store(&steps);
    }
    false
}

/// Starts a new attempt without failed steps.
pub fn reset() {
    FAILED.lock().unwrap().clear();
}

/// True when a step of this run failed.
pub fn failed() -> bool {
    !FAILED.lock().unwrap().is_empty()
}

/// After a complete activation, the steps to do again are the failed steps of this run.
pub fn save_retries() {
    store(&FAILED.lock().unwrap().iter().cloned().collect());
}

/// A line for a command that writes no output for a long time, so that the
/// output shows that the step continues, with the last state of a progress meter.
pub fn heartbeat(command: &str, minutes: u64, meter: &str) {
    let guard = RUN.lock().unwrap();
    let (step, minutes) = match guard.as_ref() {
        Some((s, start, _)) if !s.is_empty() => (s.clone(), start.elapsed().as_secs() / 60),
        _ => (command.to_string(), minutes),
    };
    match meter.trim() {
        "" => println!("pacnix: the step continues after {minutes} min without output: {step}"),
        meter => println!("pacnix: the step continues after {minutes} min: {step}: {meter}"),
    }
}

/// The install notes and the new .pacnew files in the part of the pacman log
/// that the run wrote. pacman logs the output of a scriptlet after the line of
/// its package, and the output of a hook after the line that runs the hook.
/// Hook output, for example of DKMS and mkinitcpio, is not a note.
fn log_segment(offset: u64) -> (BTreeMap<String, String>, Vec<String>) {
    let mut text = String::new();
    if let Ok(mut f) = std::fs::File::open(PACMAN_LOG) {
        let _ = f.seek(std::io::SeekFrom::Start(offset)).and_then(|_| f.read_to_string(&mut text));
    }
    let (mut notes, mut pacnew, mut current): (BTreeMap<String, String>, Vec<String>, Option<String>) = Default::default();
    for line in text.lines() {
        let Some((_, rest)) = line.split_once("] [") else { continue };
        if let Some(msg) = rest.strip_prefix("ALPM-SCRIPTLET] ") {
            if let Some(pkg) = &current {
                let note = notes.entry(pkg.clone()).or_default();
                note.push_str(msg.trim_end());
                note.push('\n');
            }
        } else if let Some(msg) = rest.strip_prefix("ALPM] ") {
            current = ["installed ", "upgraded ", "reinstalled ", "downgraded "]
                .iter()
                .find_map(|p| msg.strip_prefix(p))
                .and_then(|m| m.split_whitespace().next())
                .map(String::from);
            if let Some(file) = msg.strip_prefix("warning: ").and_then(|m| m.split_once(" installed as ")).map(|(_, f)| f) {
                pacnew.push(file.to_string());
            }
        }
    }
    (notes, pacnew)
}

/// Prints what the user must know after the run: a necessary reboot, new or
/// changed install notes and new .pacnew files. The notes stay for the audit.
pub fn summary() {
    let Some((_, _, offset)) = RUN.lock().unwrap().clone() else { return };
    let (notes, pacnew) = log_segment(offset);
    let mut saved: Notes = std::fs::read(NOTES).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let fresh: BTreeMap<String, String> = notes.into_iter().filter(|(p, n)| saved.scriptlets.get(p) != Some(n)).collect();
    for r in reboot_reasons() {
        println!("pacnix: reboot to use the changes: {r}");
    }
    for (pkg, note) in &fresh {
        println!("pacnix: install note of {pkg}:\n  {}", note.trim_end().replace('\n', "\n  "));
    }
    for f in &pacnew {
        println!("pacnix: new file {f}: merge it with {}", f.trim_end_matches(".pacnew"));
    }
    for l in LAST.lock().unwrap().iter() {
        println!("{l}");
    }
    let failed = FAILED.lock().unwrap();
    if !failed.is_empty() {
        println!("pacnix: the apply is done, but {} steps failed. The next apply does them again:", failed.len());
        for f in failed.iter() {
            println!("  {f}");
        }
    }
    if !fresh.is_empty() {
        saved.scriptlets.extend(fresh.clone());
        saved.last = fresh;
        if let Ok(b) = serde_json::to_vec(&saved) {
            let _ = std::fs::write(NOTES, b);
        }
    }
}

/// The items of the summary that stay true after the run, for the audit.
pub fn audit(report: &mut Vec<String>) {
    report.extend(reboot_reasons().into_iter().map(|r| format!("reboot necessary: {r}")));
    let saved: Notes = std::fs::read(NOTES).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    report.extend(saved.last.iter().map(|(p, n)| format!("install note of {p} from the last change:\n  {}", n.trim_end().replace('\n', "\n  "))));
}

fn boot_time() -> i64 {
    let stat = std::fs::read_to_string("/proc/stat").unwrap_or_default();
    stat.lines().find_map(|l| l.strip_prefix("btime ")?.trim().parse().ok()).unwrap_or(i64::MAX)
}

/// The reasons to reboot: the running kernel left the system, the boot images
/// changed after the boot, or a DKMS module was built after the boot, for a
/// kernel that does not run or that did not load it.
fn reboot_reasons() -> Vec<String> {
    let release = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default().trim().to_string();
    let mut reasons = Vec::new();
    if !Path::new(&format!("/usr/lib/modules/{release}/vmlinuz")).exists() {
        reasons.push(format!("the running kernel {release} is not installed"));
    }
    let boot = boot_time();
    let newer = |p: &Path| std::fs::metadata(p).is_ok_and(|m| m.mtime() > boot);
    for dir in ["/boot", "/efi"] {
        for e in walkdir::WalkDir::new(dir).max_depth(4).into_iter().flatten() {
            let name = e.file_name().to_string_lossy();
            let image = name.starts_with("initramfs-") || name.starts_with("vmlinuz-") || name.starts_with("booster-") || (name.ends_with(".efi") && e.path().parent().is_some_and(|d| d.ends_with("EFI/Linux")));
            if image && e.file_type().is_file() && newer(e.path()) {
                reasons.push(format!("{} changed after the boot", e.path().display()));
            }
        }
    }
    for e in walkdir::WalkDir::new("/usr/lib/modules").min_depth(3).into_iter().flatten() {
        let kernel = e.path().strip_prefix("/usr/lib/modules").ok().and_then(|p| p.iter().next()).map(|k| k.to_string_lossy().into_owned()).unwrap_or_default();
        let name = e.file_name().to_string_lossy();
        let Some(module) = name.split_once(".ko").map(|(m, _)| m.replace('-', "_")) else { continue };
        if !e.path().starts_with(format!("/usr/lib/modules/{kernel}/updates")) || !newer(e.path()) {
            continue;
        }
        if kernel != release {
            reasons.push(format!("the module {module} was built for the kernel {kernel}, which does not run"));
        } else if !Path::new("/sys/module").join(&module).exists() {
            reasons.push(format!("the module {module} was built after the boot and is not loaded"));
        }
    }
    reasons.sort();
    reasons.dedup();
    reasons
}
