use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Seek};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering::Relaxed};
use std::time::{Duration, Instant};

const PACMAN_LOG: &str = "/var/log/pacman.log";
/// The install notes of the packages: the last scriptlet output of each
/// package, and the notes of the last run that had notes, for the audit.
const NOTES: &str = "/var/lib/pacnix/notes.json";
/// The isolated steps that failed and that the next apply does again.
const RETRY: &str = "/var/lib/pacnix/retry.json";

/// The current step and its start, the size of the pacman log at the start of
/// the run, and the reasons to reboot at the start of the run.
static RUN: Mutex<Option<(String, Instant, u64, Vec<String>)>> = Mutex::new(None);
/// The number of planned changes of each kind, for the last line.
static PLANNED: Mutex<Vec<((&str, &str), Vec<String>)>> = Mutex::new(Vec::new());
/// The start of the run, in seconds since the epoch, less one.
static STARTED: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(i64::MAX);
/// The number of the stage that runs, 0 without a stage.
static STAGE: Mutex<u64> = Mutex::new(0);
/// Set before the first change of the system. Before it, an interrupt stops
/// pacnix at once, and nothing changed.
static CHANGING: AtomicBool = AtomicBool::new(false);
/// The time of the first interrupt after the first change, in seconds since the boot.
static INTERRUPT: AtomicI64 = AtomicI64::new(0);
/// The terminal settings at the start, which a password prompt changes.
static mut TERMINAL: std::mem::MaybeUninit<libc::termios> = std::mem::MaybeUninit::uninit();
static TERMINAL_SAVED: AtomicBool = AtomicBool::new(false);

/// The error of a run that the user interrupted.
#[derive(Debug)]
pub struct Interrupted;

impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("interrupted")
    }
}

impl std::error::Error for Interrupted {}

fn uptime() -> i64 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut t) };
    t.tv_sec
}

/// Ctrl-C. Before the first change, pacnix stops at once. After it, the step
/// that runs stops, because its commands get the interrupt too, and pacnix
/// stops after it with the last line. A second interrupt stops pacnix at once.
/// sudo relays the interrupt of the terminal, thus a second one in the same
/// two seconds is the same interrupt.
extern "C" fn interrupt(_: libc::c_int) {
    if CHANGING.load(Relaxed) {
        let (now, first) = (uptime(), INTERRUPT.load(Relaxed));
        if first == 0 {
            INTERRUPT.store(now.max(1), Relaxed);
            return;
        }
        if now - first < 2 {
            return;
        }
    }
    let text: &[u8] = match CHANGING.load(Relaxed) {
        false => b"\npacnix: interrupted before any change. Nothing changed. Exit status 130.\n",
        true => b"\npacnix: stopped at once by a second interrupt. The step that ran can be incomplete. Run pacnix again to complete it. Exit status 130.\n",
    };
    unsafe {
        if TERMINAL_SAVED.load(Relaxed) {
            libc::tcsetattr(0, libc::TCSANOW, (&raw const TERMINAL).cast());
        }
        libc::write(1, text.as_ptr().cast(), text.len());
        libc::_exit(130);
    }
}

/// Installs the handler of Ctrl-C and keeps the terminal settings.
pub fn signals() {
    unsafe {
        if libc::tcgetattr(0, (&raw mut TERMINAL).cast()) == 0 {
            TERMINAL_SAVED.store(true, Relaxed);
        }
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = interrupt as extern "C" fn(libc::c_int) as usize;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut());
    }
}

/// Marks the start of the changes to the system.
pub fn changing() {
    CHANGING.store(true, Relaxed);
}

/// True after an interrupt.
pub fn interrupted() -> bool {
    INTERRUPT.load(Relaxed) != 0
}

/// Fails after an interrupt, so that pacnix stops after the current command.
pub fn check() -> anyhow::Result<()> {
    match interrupted() {
        true => Err(Interrupted.into()),
        false => Ok(()),
    }
}
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
    STARTED.store(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64 - 1), Relaxed);
    let offset = std::fs::metadata(PACMAN_LOG).map_or(0, |m| m.len());
    *RUN.lock().unwrap() = Some((String::new(), Instant::now(), offset, reboot_reasons()));
}

/// Writes the step that starts to the output.
pub fn progress(step: &str) {
    say!("pacnix: {step}");
    if let Some((s, start, _, _)) = RUN.lock().unwrap().as_mut() {
        *s = step.to_string();
        *start = Instant::now();
    }
}

/// The text with each line longer than 80 columns broken between words. The
/// next parts of a broken line have two more spaces of indent.
pub fn wrap(text: &str) -> String {
    wrapped(text).replace(crate::BIND, " ")
}

fn wrapped(text: &str) -> String {
    let lines = text.split('\n').map(|line| {
        if line.chars().count() <= 80 {
            return line.to_string();
        }
        // A top-level line continues under the text after "pacnix: ", deeper than the items under it.
        let indent = line.len() - line.trim_start().len();
        let next = if line.starts_with("pacnix: ") { 8 } else { indent + 2 };
        let (mut out, mut col) = (" ".repeat(indent), indent);
        for (i, word) in line.split_whitespace().enumerate() {
            let len = word.chars().count();
            if i > 0 && col + 1 + len > 80 {
                // A short word, for example "in", moves to the next line with the word after it.
                let last = out.rsplit_once(' ').filter(|(head, w)| w.chars().count() <= 3 && !head.rsplit('\n').next().unwrap_or_default().trim().is_empty()).map(|(head, w)| (head.len(), w.to_string()));
                let carry = last.map(|(at, w)| {
                    out.truncate(at);
                    format!("{w} ")
                });
                out.push('\n');
                out.push_str(&" ".repeat(next));
                out.push_str(carry.as_deref().unwrap_or_default());
                col = next + carry.map_or(0, |c| c.chars().count());
            } else if i > 0 {
                out.push(' ');
                col += 1;
            }
            out.push_str(word);
            col += len;
        }
        out
    });
    lines.collect::<Vec<_>>().join("\n")
}

/// Ends the stage when it goes out of scope.
pub struct Stage(u64);

impl Drop for Stage {
    fn drop(&mut self) {
        let mut s = STAGE.lock().unwrap();
        if *s == self.0 {
            *s = 0;
        }
    }
}

/// A stage of the work that changes nothing, for example the evaluation. It
/// shows only when it takes 2 s or more, and an interrupt names it.
pub fn stage(step: &str) -> Stage {
    let id = {
        let mut s = STAGE.lock().unwrap();
        *s += 1;
        *s
    };
    if let Some((s, start, _, _)) = RUN.lock().unwrap().as_mut() {
        *s = step.to_string();
        *start = Instant::now();
    }
    let step = step.to_string();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(2));
        if *STAGE.lock().unwrap() == id {
            say!("pacnix: {step}");
        }
    });
    Stage(id)
}

/// The number of planned changes of each kind, for the last line.
pub fn planned(kinds: Vec<((&'static str, &'static str), Vec<String>)>) {
    *PLANNED.lock().unwrap() = kinds;
}

/// The end of the pacman log, as the start of a later read.
pub fn log_end() -> u64 {
    std::fs::metadata(PACMAN_LOG).map_or(0, |m| m.len())
}

/// The packages that pacman changed after the offset in the pacman log, by verb.
pub fn changed_packages(offset: u64) -> Vec<(&'static str, Vec<String>)> {
    let mut text = String::new();
    if let Ok(mut f) = std::fs::File::open(PACMAN_LOG) {
        let _ = f.seek(std::io::SeekFrom::Start(offset)).and_then(|_| f.read_to_string(&mut text));
    }
    ["installed", "upgraded", "downgraded", "reinstalled", "removed"]
        .into_iter()
        .map(|verb| (verb, text.lines().filter_map(|l| l.split_once(&format!("[ALPM] {verb} "))).map(|(_, r)| r.split(' ').next().unwrap_or(r).to_string()).collect::<Vec<_>>()))
        .filter(|(_, names)| !names.is_empty())
        .collect()
}

/// The packages that the pacman transactions of the run changed, from the pacman log.
fn packages() -> Vec<((&'static str, &'static str), Vec<String>)> {
    let Some(offset) = RUN.lock().unwrap().as_ref().map(|r| r.2) else { return vec![] };
    changed_packages(offset).into_iter().map(|(verb, names)| (("package", verb), names)).collect()
}

/// The last line of a run: the result, the changes and the exit status.
/// Returns the exit status.
pub fn close(verb: &str, ok: bool) -> i32 {
    let failed = FAILED.lock().unwrap().len();
    let code = if interrupted() { 130 } else if !ok { 1 } else if failed > 0 { 4 } else { 0 };
    if !CHANGING.load(Relaxed) {
        if code != 0 {
            say!("pacnix: {verb} stopped before any change. Exit status {code}.");
        }
        return code;
    }
    let mut groups = packages();
    if code == 0 || code == 4 {
        groups.extend(PLANNED.lock().unwrap().iter().filter(|(_, items)| !items.is_empty()).cloned());
    }
    // The items show by name only when there are three or fewer in total.
    let few = groups.iter().map(|g| g.1.len()).sum::<usize>() <= 3;
    let changes: Vec<String> = groups
        .iter()
        .map(|((noun, verb), items)| if few { crate::named(noun, verb, items) } else { format!("{} {verb}", crate::count(items.len(), noun)).trim_end().to_string() })
        .collect();
    let changes = match changes.is_empty() {
        true if code == 0 || code == 4 => "No changes.".to_string(),
        true => String::new(),
        false => format!("Changes: {}.", changes.join("; ")),
    };
    let step = RUN.lock().unwrap().as_ref().map(|r| r.0.clone()).unwrap_or_default();
    match code {
        0 => say!("pacnix: {verb} done. {changes} Exit status 0."),
        4 => say!("pacnix: {verb} done, but {} failed. {changes} The next apply does the failed steps again. Exit status 4.", crate::count(failed, "step")),
        _ => say!(
            "pacnix: {verb} {} at the step: {step}. The steps before it are done.{}{changes} Run pacnix {verb} again to complete it. Exit status {code}.",
            if code == 130 { "interrupted" } else { "stopped" },
            if changes.is_empty() { "" } else { " " }
        ),
    }
    code
}

/// A download whose progress shows every 30 s: the package files with their
/// sizes, the cache directory, the label, the bytes at the last change, the
/// time of the last change and the time of the last line.
struct Download {
    files: Vec<(String, u64)>,
    cache: String,
    label: String,
    done: u64,
    changed: Instant,
    shown: Instant,
}

static DOWNLOAD: Mutex<Option<Download>> = Mutex::new(None);

/// Starts or ends the progress of a download of package files with their sizes into the cache directory.
pub fn download(label: &str, cache: &str, files: Option<Vec<(String, u64)>>) {
    *DOWNLOAD.lock().unwrap() = files.map(|files| Download { files, cache: cache.to_string(), label: label.to_string(), done: 0, changed: Instant::now(), shown: Instant::now() });
}

fn mib(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1048576.0)
}

/// Shows the progress of a download every 30 s, and tells when no data
/// arrived for a minute: the complete files in the cache directory and the
/// partial files in its download directories of pacman. Returns false
/// without a download.
pub fn meter() -> bool {
    let mut guard = DOWNLOAD.lock().unwrap();
    let Some(d) = guard.as_mut() else { return false };
    if d.shown.elapsed() < Duration::from_secs(30) {
        return true;
    }
    d.shown = Instant::now();
    let complete: Vec<u64> = d.files.iter().filter(|(f, _)| Path::new(&d.cache).join(f).exists()).map(|(_, s)| *s).collect();
    let partial: u64 = std::fs::read_dir(&d.cache)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("download-"))
        .flat_map(|e| std::fs::read_dir(e.path()).into_iter().flatten().flatten())
        .filter_map(|e| e.metadata().ok().map(|m| m.len()))
        .sum();
    let done = complete.iter().sum::<u64>() + partial;
    let total: u64 = d.files.iter().map(|f| f.1).sum();
    let state = format!("{} of {}, {} of {} MiB", complete.len(), crate::count(d.files.len(), "package"), mib(done), mib(total));
    if done != d.done {
        (d.done, d.changed) = (done, Instant::now());
        say!("pacnix: {}: {state}", d.label);
    } else if d.changed.elapsed() >= Duration::from_secs(60) {
        say!("pacnix: {}: no data arrived for {} min: {state}", d.label, d.changed.elapsed().as_secs() / 60);
    }
    true
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
    say!("pacnix: error: {step}: {reason}");
    say!("pacnix: the apply continues without this step");
    FAILED.lock().unwrap().push(step.to_string());
    let mut steps = retries();
    if steps.insert(step.to_string()) {
        store(&steps);
    }
    false
}

/// Records steps that must follow a change, before the change, so that the
/// next apply does them also after an interrupt or a power loss. Each step
/// leaves the record when it succeeds.
pub fn pending(steps: impl IntoIterator<Item = String>) {
    let mut all = retries();
    let before = all.len();
    all.extend(steps);
    if all.len() != before {
        store(&all);
    }
}

/// Records a step that the next attempt or apply does again, and returns its error.
pub fn remember(step: &str, e: anyhow::Error) -> anyhow::Error {
    let mut steps = retries();
    if steps.insert(step.to_string()) {
        store(&steps);
    }
    e
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
        Some((s, start, _, _)) if !s.is_empty() => (s.clone(), start.elapsed().as_secs() / 60),
        _ => (command.to_string(), minutes),
    };
    match meter.trim() {
        "" => say!("pacnix: the step continues after {minutes} min without output: {step}"),
        meter => say!("pacnix: the step continues after {minutes} min: {step}: {meter}"),
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
    let Some((_, _, offset, before)) = RUN.lock().unwrap().clone() else { return };
    let (notes, pacnew) = log_segment(offset);
    let mut saved: Notes = std::fs::read(NOTES).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let fresh: BTreeMap<String, String> = notes.into_iter().filter(|(p, n)| saved.scriptlets.get(p) != Some(n)).collect();
    let reboot: Vec<String> = reboot_reasons().into_iter().filter(|r| !before.contains(r)).collect();
    if !reboot.is_empty() {
        say!("pacnix: reboot to use the changes: {}", reboot.join("; "));
    } else if reboot_reasons().is_empty() && !reasons(STARTED.load(Relaxed)).is_empty() {
        say!("pacnix: no reboot is necessary");
    }
    for (pkg, note) in &fresh {
        say!("pacnix: install note of {pkg}:\n  {}", note.trim_end().replace('\n', "\n  "));
    }
    for f in &pacnew {
        say!("pacnix: new file {f}: merge it with {}", f.trim_end_matches(".pacnew"));
    }
    for l in LAST.lock().unwrap().iter() {
        say!("{l}");
    }
    for f in FAILED.lock().unwrap().iter() {
        say!("pacnix: failed step: {f}");
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
    report.extend(saved.last.iter().map(|(p, n)| format!("install note of the last change: {p}\n  {}", n.trim_end().replace('\n', "\n  "))));
}

fn boot_time() -> i64 {
    let stat = std::fs::read_to_string("/proc/stat").unwrap_or_default();
    stat.lines().find_map(|l| l.strip_prefix("btime ")?.trim().parse().ok()).unwrap_or(i64::MAX)
}

/// The reasons to reboot: the running kernel left the system, the boot images
/// changed after the boot, or a DKMS module was built after the boot, for a
/// kernel that does not run or that did not load it.
fn reboot_reasons() -> Vec<String> {
    reasons(boot_time())
}

/// The reasons to reboot from the changes after a time: the kernel, the boot
/// images and the DKMS modules.
fn reasons(since: i64) -> Vec<String> {
    let release = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default().trim().to_string();
    let mut reasons = Vec::new();
    if !Path::new(&format!("/usr/lib/modules/{release}/vmlinuz")).exists() {
        reasons.push(format!("the running kernel {release} is not installed"));
    }
    let newer = |p: &Path| std::fs::metadata(p).is_ok_and(|m| m.mtime() > since);
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
