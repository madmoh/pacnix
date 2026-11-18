//! What the configuration cannot show and the user must accept: a new
//! maintainer or a new AUR dependency of an AUR package that was built before,
//! and the Arch Linux news since the last upgrade.
use crate::packages::dep_name;
use crate::{cmd, run_cmd};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The maintainers and the AUR dependencies of each AUR package at its last
/// build, and the Git revision that was built.
const RECORDS: &str = "/var/lib/pacnix/aur.json";
/// The time of the last upgrade, for the news.
const NEWS: &str = "/var/lib/pacnix/news";
const NEWS_FEED: &str = "https://archlinux.org/feeds/news/";

#[derive(Serialize, Deserialize, Default, Clone, PartialEq)]
#[serde(default)]
pub struct Record {
    maintainer: String,
    comaintainers: Vec<String>,
    depends: Vec<String>,
    pub head: String,
}

pub fn records() -> BTreeMap<String, Record> {
    std::fs::read(RECORDS).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Saves the record of a package after its build.
pub fn save(name: &str, record: Record) -> Result<()> {
    let mut all = records();
    all.insert(name.to_string(), record);
    Ok(std::fs::write(RECORDS, serde_json::to_vec_pretty(&all)?)?)
}

/// Asks the AUR RPC for the maintainers and dependencies of the packages that
/// will build, and compares them with the records. Returns the changes that
/// need the consent of the user and the new records. A package without a
/// record gets one without a question.
pub fn changes(names: &[String]) -> Result<(Vec<String>, BTreeMap<String, Record>)> {
    let old = records();
    let mut fresh = BTreeMap::new();
    if names.is_empty() {
        return Ok((vec![], fresh));
    }
    let file = format!("/run/pacnix/aur-info-{}", std::process::id());
    std::fs::create_dir_all("/run/pacnix")?;
    let query: Vec<String> = names.iter().map(|n| format!("arg[]={}", n.replace('+', "%2B"))).collect();
    run_cmd(cmd("curl", &["-fsS", "--max-time", "60", "-o", &file]).arg(format!("https://aur.archlinux.org/rpc/v5/info?{}", query.join("&"))))?;
    let info: serde_json::Value = serde_json::from_slice(&std::fs::read(&file)?)?;
    let _ = std::fs::remove_file(&file);
    let known: BTreeSet<&str> = names.iter().map(String::as_str).chain(old.keys().map(String::as_str)).collect();
    let strings = |v: &serde_json::Value| -> Vec<String> { v.as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect() };
    let mut asks = Vec::new();
    for r in info["results"].as_array().into_iter().flatten() {
        let name = r["Name"].as_str().unwrap_or_default().to_string();
        let mut depends: Vec<String> = ["Depends", "MakeDepends", "CheckDepends"]
            .iter()
            .flat_map(|k| strings(&r[*k]))
            .map(|d| dep_name(&d).to_string())
            .filter(|d| known.contains(d.as_str()))
            .collect();
        depends.sort();
        depends.dedup();
        let mut rec = Record { maintainer: r["Maintainer"].as_str().unwrap_or("none").to_string(), comaintainers: strings(&r["CoMaintainers"]), depends, head: String::new() };
        rec.comaintainers.sort();
        if let Some(o) = old.get(&name) {
            if o.maintainer != rec.maintainer || o.comaintainers != rec.comaintainers {
                asks.push(format!(
                    "The AUR package {name} has new maintainers: {} (co-maintainers: {}), before: {} (co-maintainers: {}).",
                    rec.maintainer,
                    list(&rec.comaintainers),
                    o.maintainer,
                    list(&o.comaintainers)
                ));
            }
            let new: Vec<&String> = rec.depends.iter().filter(|d| !o.depends.contains(d)).collect();
            if !new.is_empty() {
                asks.push(format!("The AUR package {name} has new AUR dependencies: {}.", new.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
            }
            rec.head = o.head.clone();
        }
        fresh.insert(name, rec);
    }
    Ok((asks, fresh))
}

fn list(v: &[String]) -> String {
    if v.is_empty() { "none".to_string() } else { v.join(", ") }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// The Arch Linux news since the last read of the feed. Without a record,
/// there is no news: the first run records the time. When the feed is not
/// available, None: the time stays, and the next apply that reads the feed
/// shows the news since the last read.
pub fn news() -> Option<Vec<String>> {
    let Some(since) = std::fs::read_to_string(NEWS).ok().and_then(|t| t.trim().parse::<i64>().ok()) else { return Some(vec![]) };
    let out = std::process::Command::new("curl").args(["-fsSL", "--max-time", "20", NEWS_FEED]).output().ok().filter(|o| o.status.success())?;
    let xml = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut items = Vec::new();
    let mut reader = quick_xml::Reader::from_str(&xml);
    let (mut tag, mut item): (String, BTreeMap<String, String>) = Default::default();
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(e)) => {
                tag = e.name().as_ref().to_string();
                if tag == "item" {
                    item.clear();
                }
            }
            Ok(quick_xml::events::Event::Text(t)) => {
                item.entry(tag.clone()).or_default().push_str(t.as_ref());
            }
            Ok(quick_xml::events::Event::GeneralRef(r)) => {
                if let Ok(Some(c)) = r.resolve_char_ref() {
                    item.entry(tag.clone()).or_default().push(c);
                } else {
                    let c = match r.as_ref() { "amp" => "&", "lt" => "<", "gt" => ">", "quot" => "\"", "apos" => "'", _ => "" };
                    item.entry(tag.clone()).or_default().push_str(c);
                }
            }
            Ok(quick_xml::events::Event::End(e)) if e.name().as_ref() == "item" => {
                let date = item.get("pubDate").cloned().unwrap_or_default();
                if rfc2822(&date).is_some_and(|t| t > since) {
                    items.push(format!("Arch Linux news of {date}: {}\n  {}", item.get("title").map_or("", |s| s.trim()), item.get("link").map_or("", |s| s.trim())));
                }
            }
            Ok(quick_xml::events::Event::End(_)) => tag.clear(),
            Ok(quick_xml::events::Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    Some(items)
}

/// Records the time of a successful upgrade whose news were read, or of the first successful run.
pub fn news_read(read: bool) {
    if read || !std::path::Path::new(NEWS).exists() {
        let _ = std::fs::write(NEWS, format!("{}\n", now()));
    }
}

/// Seconds since 1970 of an RFC 2822 date, for example "Sat, 01 Nov 2025 12:00:00 +0000".
fn rfc2822(s: &str) -> Option<i64> {
    let f: Vec<&str> = s.split_whitespace().collect();
    let [_, day, month, year, time, zone] = f[..] else { return None };
    let m = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"].iter().position(|x| *x == month)? as i64 + 1;
    let (d, y): (i64, i64) = (day.parse().ok()?, year.parse().ok()?);
    let hms: Vec<i64> = time.split(':').map(|x| x.parse().unwrap_or(0)).collect();
    let z: i64 = zone.parse().unwrap_or(0);
    // Days from civil date, from H. Hinnant's chrono algorithms.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468;
    let offset = z.signum() * ((z.abs() / 100) * 3600 + (z.abs() % 100) * 60);
    Some(days * 86400 + hms.first()? * 3600 + hms.get(1)? * 60 + hms.get(2)? - offset)
}
