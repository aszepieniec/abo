//! Download links: a URL that is its own credential, for a script to fetch
//! the books whole (`/export/<token>`), where the gateway has no credential to give
//! one (DESIGN §1, §8). Kept in `.abo/export-links.json`, outside git: not
//! exported, not replayed, not carried by an import. The token is kept, so
//! Backups can show every link's address to an admin; whoever reads the
//! file reads working addresses, as whoever reads the books reads them.

use crate::model::Date;
use crate::store;
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

const FILE: &str = ".abo/export-links.json";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Link {
    /// The first 12 hex digits of the hash: how the page names it.
    pub id: String,
    pub label: String,
    /// Who made it, as the commits name them.
    pub by: String,
    /// When it was made, ISO 8601.
    pub made: String,
    /// Its last day, if it has one; it answers through that day.
    pub until: Option<Date>,
    /// When it was last fetched through, ISO 8601.
    pub used: Option<String>,
    sha256: String,
    /// The key itself, for the address Backups shows. Blank for a link
    /// made when only its hash was kept.
    #[serde(default)]
    pub token: String,
}

impl Link {
    pub fn expired(&self, today: Date) -> bool {
        self.until.is_some_and(|u| u < today)
    }
}

/// The links, oldest first.
pub fn list(root: &Path) -> Vec<Link> {
    std::fs::read(root.join(FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(root: &Path, links: &[Link]) -> Result<()> {
    std::fs::create_dir_all(root.join(".abo"))?;
    let tmp = root.join(".abo/export-links.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(links)?)?;
    std::fs::rename(&tmp, root.join(FILE)).context("saving the download links")
}

fn now() -> String {
    jiff::Zoned::now().strftime("%Y-%m-%dT%H:%M:%S%:z").to_string()
}

/// A new link.
pub fn make(root: &Path, label: &str, by: &str, until: Option<Date>, today: Date) -> Result<Link> {
    let label = label.trim();
    if label.is_empty() || label.chars().any(char::is_control) {
        bail!("a label is needed, on one line, to tell the link from others: what fetches through it, such as \"nightly backup\"");
    }
    if until.is_some_and(|u| u < today) {
        bail!("its last day is before today, so it would never answer");
    }
    let mut bytes = [0u8; 32];
    std::io::Read::read_exact(&mut std::fs::File::open("/dev/urandom")?, &mut bytes)?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let sha256 = store::sha256(token.as_bytes());
    let link = Link { id: sha256[..12].to_string(), label: label.into(), by: by.into(), made: now(), until, used: None, sha256, token };
    let mut links = list(root);
    links.push(link.clone());
    save(root, &links)?;
    Ok(link)
}

/// Take the link `id` away: from now on it answers as a wrong one does.
pub fn revoke(root: &Path, id: &str) -> Result<()> {
    let mut links = list(root);
    let before = links.len();
    links.retain(|l| l.id != id);
    if links.len() == before {
        bail!("there is no download link {id}");
    }
    save(root, &links)
}

/// The link `token` is, if it answers today, its use recorded. A wrong
/// token, a revoked one and an expired one are alike None.
pub fn check(root: &Path, token: &str, today: Date) -> Option<Link> {
    // Only what a token can be: 64 hex digits.
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let sha256 = store::sha256(token.to_ascii_lowercase().as_bytes());
    let mut links = list(root);
    let l = links.iter_mut().find(|l| l.sha256 == sha256 && !l.expired(today))?;
    l.used = Some(now());
    let found = l.clone();
    let _ = save(root, &links);
    Some(found)
}

#[test]
fn a_link_answers_until_revoked_or_past_its_day() {
    let dir = std::env::temp_dir().join(format!("abo-links-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let day = |d: &str| crate::model::ymd(d).unwrap();
    let today = day("2026-11-05");
    let a = make(&dir, "nightly", "Ann", Some(day("2026-11-30")), today).unwrap();
    let token = a.token.clone();
    assert_eq!(list(&dir)[0].token, token, "kept, to show");
    assert_eq!(check(&dir, &token, today).map(|l| l.id), Some(a.id.clone()));
    assert!(list(&dir)[0].used.is_some());
    assert!(check(&dir, &token.to_uppercase(), day("2026-11-30")).is_some());
    assert!(check(&dir, &token, day("2026-12-01")).is_none(), "past its last day");
    assert!(check(&dir, &"0".repeat(64), today).is_none());
    assert!(check(&dir, "../../x", today).is_none());
    let forever = make(&dir, "backup", "Ann", None, today).unwrap().token;
    revoke(&dir, &a.id).unwrap();
    assert!(check(&dir, &token, today).is_none(), "revoked");
    assert!(check(&dir, &forever, day("2099-01-01")).is_some());
    assert!(make(&dir, " ", "Ann", None, today).is_err());
    assert!(make(&dir, "old", "Ann", Some(day("2026-11-04")), today).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
