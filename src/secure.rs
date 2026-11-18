//! Secure Boot with the keys of the host, through sbctl, and the TPM2 keys of
//! the LUKS devices of the initramfs, through systemd-cryptenroll.
use crate::generation::Generation;
use crate::{cmd, isolated, output, output_any, report, run_quiet};
use anyhow::{Result, bail};
use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;

const GLOBAL: &str = "8be4df61-93ca-11d2-aa0d-00e098032b8c";
const KEYS: &str = "/var/lib/sbctl/keys";
const TPM: &str = "/sys/class/tpm/tpm0";

/// The passphrase and the new PIN of each LUKS device, which the user types at the run.
static SECRETS: Mutex<BTreeMap<String, (String, String)>> = Mutex::new(BTreeMap::new());

/// The Secure Boot state of the firmware and the keys of sbctl.
struct Firmware {
    /// The firmware checks the signatures.
    enforced: bool,
    setup: bool,
    keys: bool,
    /// The platform key of the firmware is the one of sbctl.
    ours: bool,
}

/// A byte of a global EFI variable, from efivarfs: the value after the 4 bytes of attributes.
fn variable(name: &str) -> Option<u8> {
    std::fs::read(format!("/sys/firmware/efi/efivars/{name}-{GLOBAL}")).ok()?.get(4).copied()
}

fn firmware() -> Firmware {
    let setup = variable("SetupMode") == Some(1);
    let keys = Path::new(KEYS).join("db/db.key").exists();
    let pem = std::fs::read_to_string(format!("{KEYS}/PK/PK.pem")).unwrap_or_default();
    let ours = keys && !setup && {
        let enrolled: serde_json::Value = serde_json::from_str(&output_any(cmd("sbctl", &["list-enrolled-keys", "--json"])).unwrap_or_default()).unwrap_or_default();
        enrolled["PK"][0]["Raw"].as_str().is_some_and(|raw| pem.lines().filter(|l| !l.starts_with("-----")).collect::<String>() == raw)
    };
    Firmware { enforced: variable("SecureBoot") == Some(1) && !setup, setup, keys, ours }
}

/// The declared boot files and the files that the loader entries on the EFI
/// system partition boot, with the signed output of each.
fn boot_files(g: &Generation) -> Vec<(String, String)> {
    let sb = &g.m.boot.secure_boot;
    let mut list: Vec<(String, String)> = sb.files.iter().map(|f| (f.path.clone(), f.output.clone())).collect();
    let mut entries: Vec<_> = std::fs::read_dir(format!("{}/loader/entries", sb.esp)).into_iter().flatten().flatten().map(|e| e.path()).collect();
    entries.sort();
    for e in entries.iter().filter(|e| e.extension().is_some_and(|x| x == "conf")) {
        for line in std::fs::read_to_string(e).unwrap_or_default().lines() {
            if let Some(("linux" | "efi", file)) = line.trim().split_once(char::is_whitespace) {
                let path = format!("{}/{}", sb.esp, file.trim().trim_start_matches('/'));
                if !list.iter().any(|(p, _)| *p == path) {
                    list.push((path.clone(), path));
                }
            }
        }
    }
    list
}

/// The files in the database of sbctl, with their output and whether they are signed.
fn database() -> BTreeMap<String, (String, bool)> {
    let list: serde_json::Value = serde_json::from_str(&output_any(cmd("sbctl", &["list-files", "--json"])).unwrap_or_default()).unwrap_or_default();
    let files = list.as_array().into_iter().flatten();
    files.filter_map(|f| Some((f["file"].as_str()?.to_string(), (f["output_file"].as_str()?.to_string(), f["is_signed"].as_bool() == Some(true))))).collect()
}

/// The boot files that exist and that sbctl did not sign to their output.
fn unsigned(g: &Generation) -> Vec<(String, String)> {
    let done = database();
    boot_files(g).into_iter().filter(|(p, o)| Path::new(p).exists() && done.get(p) != Some(&(o.clone(), true))).collect()
}

/// The files in the database of sbctl that are not boot files of the
/// configuration, also files that do not exist, which sbctl list-files does not show.
fn forgotten(g: &Generation) -> Vec<String> {
    let files = boot_files(g);
    let saved: BTreeMap<String, serde_json::Value> = std::fs::read("/var/lib/sbctl/files.json").ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    saved.into_keys().filter(|f| !files.iter().any(|(p, _)| p == f)).collect()
}

fn executable(path: &str) -> bool {
    std::fs::read(path).is_ok_and(|b| b.starts_with(b"MZ"))
}

/// The path of a crypttab device: UUID=, PARTUUID=, LABEL= or PARTLABEL=, or a path.
fn device(spec: &str) -> String {
    match spec.split_once('=') {
        Some((kind, value)) if !spec.starts_with('/') => format!("/dev/disk/by-{}/{value}", kind.to_lowercase()),
        _ => spec.to_string(),
    }
}

/// The TPM2 policy hash that systemd-cryptenroll records for PCRs of the
/// SHA-256 bank and an optional PIN: PolicyPCR with the current values, then
/// PolicyAuthValue. A different hash means that the key does not unseal.
fn policy(pcrs: &[u32], pin: bool) -> Option<String> {
    use sha2::{Digest, Sha256};
    let mut sorted = pcrs.to_vec();
    sorted.sort();
    let (mut select, mut values) = ([0u8; 3], Vec::new());
    for p in &sorted {
        *select.get_mut(*p as usize / 8)? |= 1 << (p % 8);
        let hex = std::fs::read_to_string(format!("{TPM}/pcr-sha256/{p}")).ok()?;
        values.extend((0..64).step_by(2).map(|i| u8::from_str_radix(hex.trim().get(i..i + 2)?, 16).ok()).collect::<Option<Vec<u8>>>()?);
    }
    let mut digest = Sha256::new().chain_update([0u8; 32]).chain_update(0x17Fu32.to_be_bytes()).chain_update(1u32.to_be_bytes());
    digest.update([0x00, 0x0B, 3]);
    digest.update(select);
    digest.update(Sha256::digest(&values));
    let mut hash = digest.finalize().to_vec();
    if pin {
        hash = Sha256::new().chain_update(&hash).chain_update(0x16Bu32.to_be_bytes()).finalize().to_vec();
    }
    Some(hash.iter().map(|b| format!("{b:02x}")).collect())
}

enum Action {
    Enroll(String),
    Wipe,
    Recovery,
    Wait(String),
}

#[derive(Default)]
pub struct Plan {
    keys: bool,
    sign: Vec<(String, String)>,
    forget: Vec<String>,
    enroll: Option<Vec<String>>,
    tpm: Vec<(String, String, Action)>,
}

impl Plan {
    pub fn print(&self, l: &mut Vec<String>) {
        if self.keys {
            l.push("create the Secure Boot keys of this host with sbctl".to_string());
        }
        l.extend(self.sign.iter().map(|(p, o)| if p == o { format!("sign boot file {p}") } else { format!("sign boot file {p} as {o}") }));
        l.extend(self.forget.iter().map(|p| format!("stop signing file {p}, which is not a boot file of the configuration")));
        if let Some(additions) = &self.enroll {
            let with = if additions.is_empty() { "without db additions".to_string() } else { format!("with {}", additions.join(", ")) };
            l.push(format!("enroll the Secure Boot keys of this host in the firmware, {with}"));
        }
        for (name, _, action) in &self.tpm {
            l.push(match action {
                Action::Enroll(why) => format!("enroll the TPM2 key of LUKS device {name}: {why}"),
                Action::Wipe => format!("remove the TPM2 key of LUKS device {name}"),
                Action::Recovery => format!("enroll a recovery key for LUKS device {name}"),
                Action::Wait(why) => format!("note: the TPM2 key of LUKS device {name} waits: {why}"),
            });
        }
    }
}

/// Refuses, before any change, a configuration that can make the system not
/// boot: Secure Boot that the firmware checks with keys that are not the keys
/// of the host, a declared file that sbctl cannot sign, or the removal of
/// Secure Boot while the firmware checks the signatures.
pub fn check(target: &Generation, old: &Generation) -> Result<()> {
    let sb = &target.m.boot.secure_boot;
    let fw = firmware();
    if sb.enable && fw.enforced && !fw.ours {
        bail!("the firmware checks Secure Boot signatures with keys that are not the keys of this host, thus a boot file that pacnix signs does not boot. Reset Secure Boot to Setup Mode in the firmware, or turn off Secure Boot, then run pacnix again");
    }
    if !sb.enable && old.m.boot.secure_boot.enable && fw.enforced && fw.ours {
        bail!("the firmware checks Secure Boot signatures, and without boot.loader.systemd-boot.secureBoot the boot files of the next update are not signed. Turn off Secure Boot in the firmware, then run pacnix again");
    }
    if let Some(f) = sb.files.iter().find(|f| f.required && !executable(&f.path)) {
        bail!("{} is not an EFI binary, thus sbctl cannot sign it. Remove it from boot.loader.systemd-boot.secureBoot.extraFiles", f.path);
    }
    Ok(())
}

pub fn plan(g: &Generation) -> Result<Plan> {
    let sb = &g.m.boot.secure_boot;
    let fw = firmware();
    let mut plan = Plan::default();
    if sb.enable {
        plan.keys = !fw.keys;
        plan.sign = if fw.keys { unsigned(g) } else { boot_files(g).into_iter().filter(|(p, _)| Path::new(p).exists()).collect() };
        plan.forget = forgotten(g);
        plan.enroll = fw.setup.then(|| sb.db_additions.clone());
    }
    for (name, l) in &g.m.boot.luks {
        let dev = device(&l.device);
        if !Path::new(&dev).exists() {
            continue;
        }
        let meta: serde_json::Value = serde_json::from_str(&output(cmd("cryptsetup", &["luksDump", "--dump-json-metadata", &dev]))?)?;
        let tokens: Vec<&serde_json::Value> = meta["tokens"].as_object().into_iter().flat_map(|t| t.values()).collect();
        let tpm: Vec<&&serde_json::Value> = tokens.iter().filter(|t| t["type"] == "systemd-tpm2").collect();
        let mut add = |a| plan.tpm.push((name.clone(), dev.clone(), a));
        if !l.tpm2.enable {
            if !tpm.is_empty() {
                add(Action::Wipe);
            }
        } else {
            let mut pcrs = l.tpm2.pcrs.clone();
            pcrs.sort();
            let token_pcrs = |t: &serde_json::Value| -> Vec<u32> { t["tpm2-pcrs"].as_array().into_iter().flatten().filter_map(|p| p.as_u64().map(|p| p as u32)).collect() };
            let why = match tpm.as_slice() {
                [] => Some("the device has none"),
                [t] if token_pcrs(t) != pcrs || t["tpm2-pin"].as_bool().unwrap_or(false) != l.tpm2.pin => Some("the PCRs or the PIN changed"),
                [t] if t["tpm2-pcr-bank"] != "sha256" || t.get("tpm2_pubkey").is_some() || t["tpm2-policy-hash"].as_str() != policy(&pcrs, l.tpm2.pin).as_deref() => Some("the values of the PCRs changed"),
                [_] => None,
                _ => Some("the device has more than one"),
            };
            // PCR 7 has its final value only in a boot that checks the signatures with the keys of the host.
            let ready = !pcrs.contains(&7) || (fw.enforced && (!sb.enable || fw.ours));
            match why {
                None => {}
                Some(_) if !Path::new(TPM).exists() => add(Action::Wait("the system has no TPM2 chip".to_string())),
                Some(_) if !ready => add(Action::Wait("the firmware does not check Secure Boot signatures with the keys of this host yet".to_string())),
                Some(why) => add(Action::Enroll(why.to_string())),
            }
        }
        if l.recovery_key && !tokens.iter().any(|t| t["type"] == "systemd-recovery") {
            add(Action::Recovery);
        }
    }
    Ok(plan)
}

/// Asks for the passphrase and the new PIN of each LUKS device that gets a
/// TPM2 key or a recovery key, one time in a run, before any change. Without a
/// terminal, the enrollment waits for an apply in a terminal.
pub fn ask_secrets(g: &Generation, plan: &Plan) -> Result<()> {
    if !std::io::stdin().is_terminal() {
        return Ok(());
    }
    let mut secrets = SECRETS.lock().unwrap();
    for (name, dev, action) in &plan.tpm {
        if !matches!(action, Action::Enroll(_) | Action::Recovery) || secrets.contains_key(name) {
            continue;
        }
        let passphrase = loop {
            let p = rpassword::prompt_password(format!("Passphrase of the LUKS device {name}: "))?;
            let mut c = cmd("cryptsetup", &["open", "--test-passphrase", "--key-file=-", dev]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
            c.stdin.take().unwrap().write_all(p.as_bytes())?;
            if c.wait()?.success() {
                break p;
            }
            println!("The passphrase is wrong. Type it again.");
        };
        let mut pin = String::new();
        while g.m.boot.luks[name].tpm2.pin && plan.tpm.iter().any(|(n, _, a)| n == name && matches!(a, Action::Enroll(_))) && pin.is_empty() {
            let first = rpassword::prompt_password(format!("New TPM2 PIN of the LUKS device {name}: "))?;
            if first == rpassword::prompt_password(format!("New TPM2 PIN of the LUKS device {name} again: "))? && !first.is_empty() {
                pin = first;
            } else {
                println!("The PINs are empty or not equal. Type them again.");
            }
        }
        secrets.insert(name.clone(), (passphrase, pin));
    }
    Ok(())
}

fn sbctl(args: &[&str]) -> Command {
    cmd("sbctl", args)
}

/// Creates the keys, signs the boot files and makes sure that each is signed,
/// then enrolls the keys when the firmware is in Setup Mode and no step of the
/// run failed. Then enrolls or removes the TPM2 keys and enrolls the recovery
/// keys. The summary tells the next step in the firmware.
pub fn apply(g: &Generation, plan: &Plan) -> Result<()> {
    let sb = &g.m.boot.secure_boot;
    if sb.enable {
        if !Path::new(KEYS).join("db/db.key").exists() {
            report::progress("create the Secure Boot keys of this host");
            run_quiet(sbctl(&["create-keys"]))?;
        }
        for (path, out) in unsigned(g) {
            report::progress(&format!("sign boot file {path}"));
            let mut c = sbctl(&["sign", "--save"]);
            if path != out {
                c.args(["--output", &out]);
            }
            run_quiet(c.arg(&path))?;
        }
        for path in forgotten(g) {
            report::progress(&format!("stop signing file {path}"));
            run_quiet(sbctl(&["remove-file", &path]))?;
        }
        let left: Vec<String> = unsigned(g).into_iter().map(|(p, _)| p).collect();
        if !left.is_empty() {
            bail!("sbctl did not sign the boot files {}", left.join(", "));
        }
        let fw = firmware();
        if fw.setup && report::failed() {
            report::last("pacnix: the Secure Boot keys wait, because a step failed. Run pacnix apply again after the correction".to_string());
        } else if fw.setup {
            let step = "enroll the Secure Boot keys of this host in the firmware";
            report::progress(step);
            let flags: Vec<String> = sb.db_additions.iter().map(|a| format!("--{a}")).collect();
            // sbctl refuses keys without the hashes of the option ROMs that the firmware loads, before it changes the firmware.
            if isolated(step, || run_quiet(sbctl(&["enroll-keys"]).args(&flags)))? {
                report::last("pacnix: the firmware has the Secure Boot keys of this host. Reboot. If sbctl status then shows that Secure Boot is disabled, turn on Secure Boot in the firmware".to_string());
            } else {
                report::last("pacnix: the firmware stays in Setup Mode. Read the message of sbctl. When it tells about option ROMs, add \"tpm-eventlog\" or \"microsoft\" to boot.loader.systemd-boot.secureBoot.dbAdditions, then run pacnix apply".to_string());
            }
        } else if !fw.ours {
            report::last("pacnix: Secure Boot: reset Secure Boot to Setup Mode in the firmware, then run pacnix apply. The firmware then gets the keys of this host".to_string());
        } else if !fw.enforced {
            report::last("pacnix: Secure Boot: turn on Secure Boot in the firmware, then reboot".to_string());
        }
    }
    for (name, dev, action) in &plan.tpm {
        let secret = SECRETS.lock().unwrap().get(name).cloned();
        let needs = || format!("pacnix: the TPM2 key and the recovery key of LUKS device {name} wait for the passphrase. Run pacnix apply in a terminal");
        match (action, secret) {
            (Action::Wait(why), _) => report::last(format!("pacnix: the TPM2 key of LUKS device {name} waits: {why}")),
            (Action::Wipe, _) => {
                let step = format!("remove the TPM2 key of LUKS device {name}");
                report::progress(&step);
                isolated(&step, || run_quiet(cmd("systemd-cryptenroll", &["--wipe-slot=tpm2", dev])))?;
            }
            (_, None) => report::last(needs()),
            (Action::Enroll(_), Some((passphrase, pin))) => {
                let step = format!("enroll the TPM2 key of LUKS device {name}");
                report::progress(&step);
                let l = &g.m.boot.luks[name];
                let pcrs: Vec<String> = l.tpm2.pcrs.iter().map(u32::to_string).collect();
                let mut c = cmd("systemd-cryptenroll", &["--wipe-slot=tpm2", "--tpm2-device=auto", &format!("--tpm2-pcrs={}", pcrs.join("+"))]);
                c.arg(format!("--tpm2-with-pin={}", if l.tpm2.pin { "yes" } else { "no" })).arg(dev).env("PASSWORD", passphrase).env("NEWPIN", pin);
                isolated(&step, || run_quiet(c.stdin(Stdio::null())))?;
            }
            (Action::Recovery, Some((passphrase, _))) => {
                let step = format!("enroll a recovery key for LUKS device {name}");
                report::progress(&step);
                let mut key = String::new();
                isolated(&step, || {
                    key = output(cmd("systemd-cryptenroll", &["--recovery-key", dev]).env("PASSWORD", passphrase).stdin(Stdio::null()))?.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default().to_string();
                    Ok(())
                })?;
                if !key.is_empty() {
                    report::last(format!("pacnix: the recovery key of LUKS device {name}: {key}\n  Write it down and keep it away from this computer. pacnix does not show it again"));
                }
            }
        }
    }
    Ok(())
}

/// The Secure Boot state, the boot files that are not signed and the state of
/// the TPM2 keys, for the audit.
/// Adds the boot files without a signature and the TPM2 keys that are not
/// current to the report, and returns the Secure Boot state.
pub fn audit(g: &Generation, report: &mut Vec<String>) -> Result<Option<String>> {
    if !g.m.boot.secure_boot.enable && g.m.boot.luks.is_empty() {
        return Ok(None);
    }
    let fw = firmware();
    let state = match (fw.setup, fw.enforced, fw.ours) {
        (true, _, _) => "Secure Boot: off, the firmware is in Setup Mode".to_string(),
        (_, true, true) => "Secure Boot: on, with the keys of this host".to_string(),
        (_, true, false) => "Secure Boot: on, with keys that are not the keys of this host".to_string(),
        (_, false, true) => "Secure Boot: off, the firmware has the keys of this host".to_string(),
        (_, false, false) => "Secure Boot: off".to_string(),
    };
    if fw.keys {
        report.extend(unsigned(g).into_iter().map(|(f, _)| format!("boot file without a signature of this host: {f}")));
    }
    for (name, action) in plan(g)?.tpm.into_iter().map(|(n, _, a)| (n, a)) {
        report.push(match action {
            Action::Enroll(why) => format!("TPM2 key of LUKS device {name}: not current, {why}"),
            Action::Wait(why) => format!("TPM2 key of LUKS device {name}: waits, {why}"),
            Action::Wipe => format!("TPM2 key of LUKS device {name}: not declared"),
            Action::Recovery => format!("LUKS device {name}: no recovery key"),
        });
    }
    Ok(Some(state))
}
