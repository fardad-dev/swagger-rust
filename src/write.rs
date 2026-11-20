//! Formatting and writing the generated files.

use crate::emit::{File, GENERATED_MARKER};
use anyhow::{Context as _, Result};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Default)]
pub struct Written {
    /// Files created or changed (or, with `check`, that would be).
    pub changed: Vec<PathBuf>,
    pub unchanged: usize,
    /// Generated files of an earlier run that are no longer produced.
    pub removed: Vec<PathBuf>,
}

/// Writes `files` into `dir`, formatting them with rustfmt when asked and
/// available. Unchanged files are not touched (so builds stay incremental);
/// `config.rs` is only written when missing. With `check`, nothing is
/// written: the result lists what would change.
pub fn write(dir: &Path, files: Vec<File>, rustfmt: bool, check: bool) -> Result<Written> {
    let mut written = Written::default();
    if !check {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let formatter = if rustfmt { find_rustfmt() } else { None };

    let mut produced = Vec::new();
    for file in files {
        let path = dir.join(file.name);
        produced.push(file.name);
        let existing = std::fs::read_to_string(&path).ok();
        if !file.regenerate && existing.is_some() {
            written.unchanged += 1;
            continue;
        }
        let content = match &formatter {
            Some(rustfmt) => format(rustfmt, dir, &file.content).unwrap_or(file.content),
            None => file.content,
        };
        if existing.as_deref() == Some(content.as_str()) {
            written.unchanged += 1;
            continue;
        }
        if !check {
            std::fs::write(&path, &content)
                .with_context(|| format!("cannot write {}", path.display()))?;
        }
        written.changed.push(path);
    }

    // Generated files this configuration no longer produces (`queries.rs`
    // after turning `gpui` off).
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".rs") || produced.contains(&name.as_str()) {
                continue;
            }
            let generated =
                std::fs::read_to_string(&path).is_ok_and(|text| text.starts_with(GENERATED_MARKER));
            if generated {
                if !check {
                    std::fs::remove_file(&path)
                        .with_context(|| format!("cannot remove {}", path.display()))?;
                }
                written.removed.push(path);
            }
        }
    }
    Ok(written)
}

fn find_rustfmt() -> Option<PathBuf> {
    let rustfmt = std::env::var_os("RUSTFMT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("rustfmt"));
    let available = Command::new(&rustfmt)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    available.then_some(rustfmt)
}

/// Formats `source` with rustfmt, run from `dir` so the project's
/// `rustfmt.toml` applies. `None` if rustfmt fails (the file is then
/// written unformatted, which still compiles).
fn format(rustfmt: &Path, dir: &Path, source: &str) -> Option<String> {
    let cwd = if dir.exists() {
        dir.to_path_buf()
    } else {
        std::env::current_dir().ok()?
    };
    let mut child = Command::new(rustfmt)
        .args(["--edition", "2021", "--emit", "stdout"])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let input = source.to_owned();
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let output = child.wait_with_output().ok()?;
    writer.join().ok()?.ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}
