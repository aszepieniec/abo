//! typst: a template and its data made a PDF. Reports and invoices are laid
//! out by their templates; this only writes the data where a template reads
//! it, and runs typst.

use anyhow::{bail, Context as _, Result};
use serde::Serialize;
use std::path::Path;
use std::process::Command;

/// Write `data` where the template reads it and compile the template to `out`,
/// seeding the template into the books first if it is not there yet. Returns
/// whether it was seeded, for the caller to commit. A failure takes back what
/// this call created — the seeded template, the folders made for it and for
/// `out` — so a missing typst leaves the books as they were.
pub fn compile(
    root: &Path,
    template_rel: &str,
    template_src: &str,
    data_rel: &str,
    data: &impl Serialize,
    out: &Path,
) -> Result<bool> {
    let template = root.join(template_rel);
    let seeded = !template.exists();
    let mut made = vec![];
    let mut dir = |d: &Path| -> Result<()> {
        if !d.exists() {
            std::fs::create_dir_all(d)?;
            made.push(d.to_path_buf());
        }
        Ok(())
    };
    dir(template.parent().unwrap())?;
    if let Some(parent) = out.parent() {
        dir(parent)?;
    }
    let result = (|| {
        if seeded {
            std::fs::write(&template, template_src)?;
        }
        let json = root.join(data_rel);
        std::fs::create_dir_all(json.parent().unwrap())?;
        std::fs::write(&json, serde_json::to_vec_pretty(data)?)?;
        typst(root, &template, out)
    })();
    if result.is_err() {
        if seeded {
            let _ = std::fs::remove_file(&template);
        }
        for d in made.iter().rev() {
            let _ = std::fs::remove_dir(d); // only if empty
        }
    }
    result.map(|()| seeded)
}

/// Compile a template to PDF. The one place abo runs typst.
pub fn typst(root: &Path, template: &Path, out: &Path) -> Result<()> {
    let mut typst = Command::new("typst");
    // A diagnostic on a line of its own, file:line:col: error: ..., not
    // drawn in boxes: the CLI shows only ASCII.
    typst.arg("compile").arg("--diagnostic-format").arg("short").arg("--root").arg(root);
    // Dated by the operation's stamp, as its commit is.
    if let Ok(s) = crate::repo::Stamp::now(root) {
        typst.arg("--creation-timestamp").arg(s.time.to_string());
    }
    let result = typst.arg(template).arg(out).output();
    let result = match result {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("typst is not installed; see https://github.com/typst/typst#installation")
        }
        other => other.context("running typst")?,
    };
    if !result.status.success() {
        bail!("typst: {}", String::from_utf8_lossy(&result.stderr).trim());
    }
    Ok(())
}

