//! Settings files Lanes shares with you, like Gemini CLI's and VS Code's. A hook is plainly
//! Lanes' (its command says so), but a value like `context.fileName` isn't, so Lanes notes each
//! value it sets and what was there before. Taking agents back out puts back exactly that, and
//! only while the value is still the one Lanes set; if you've changed it since, it's yours.

use super::jsonc::Doc;
use super::{io_error, read_or_empty, write_atomic};
use crate::error::{Error, ErrorKind, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// What Lanes set, per settings file (by path).
#[derive(Debug, Default, Serialize, Deserialize)]
struct Record(BTreeMap<String, File>);

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
struct File {
    /// Lanes made the file, so it goes again once nothing else is in it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    made_file: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    values: Vec<Set>,
}

/// One value Lanes set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Set {
    /// Where it is: object keys, outermost first.
    pub key: Vec<String>,
    /// Its text before, exactly as written, or `None` when it wasn't there.
    pub was: Option<String>,
    /// What Lanes set it to.
    pub set: Value,
    /// Lanes made the object it's in, too.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub made_parent: bool,
}

fn load(path: &Path) -> Result<Record> {
    let text = read_or_empty(path)?;
    if text.trim().is_empty() {
        return Ok(Record::default());
    }
    serde_json::from_str(&text).map_err(|e| {
        Error::new(
            ErrorKind::Config,
            format!("{} isn't valid JSON ({e})", path.display()),
        )
        .hint("It's Lanes' note of what it set in agents' settings. Delete it, then run `lane setup` again.")
    })
}

fn save(path: &Path, record: &Record) -> Result<()> {
    if record.0.is_empty() {
        return match fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io_error("remove", path, e)),
            _ => Ok(()),
        };
    }
    let text = serde_json::to_string_pretty(record).expect("the record serializes") + "\n";
    write_atomic(path, &text)
}

/// What one edit of a settings file is to do: on, add Lanes' pieces (returning each value it set
/// that isn't plainly Lanes'); off, take Lanes' own pieces out. Returns whether the file changed.
pub(crate) fn edit(
    file: &Path,
    record: &Path,
    on: bool,
    add: impl FnOnce(&mut Doc) -> std::result::Result<Vec<Set>, String>,
    remove: impl FnOnce(&mut Doc) -> std::result::Result<(), String>,
) -> Result<bool> {
    let invalid = |e: String| {
        Error::new(
            ErrorKind::Config,
            format!("{} isn't valid JSON ({e})", file.display()),
        )
        .hint("Fix the file, then run `lane setup` again.")
    };
    let existed = file.exists();
    let text = read_or_empty(file)?;
    let mut doc = Doc::parse(&text).map_err(invalid)?;
    let before = doc.text().to_string();
    let mut notes = load(record)?;
    let key = file.display().to_string();
    let mut mine = notes.0.get(&key).cloned().unwrap_or_default();

    if on {
        let sets = add(&mut doc).map_err(invalid)?;
        if doc.text() == before {
            return Ok(false);
        }
        mine.made_file |= !existed;
        mine.values.extend(sets);
        // The note first: if writing the file fails, taking it back out finds nothing to undo.
        notes.0.insert(key, mine);
        save(record, &notes)?;
        let mut out = doc.text().to_string();
        if text.trim().is_empty() {
            out.push('\n');
        }
        write_atomic(file, &out)?;
        return Ok(true);
    }

    if !existed {
        if notes.0.remove(&key).is_some() {
            save(record, &notes)?;
        }
        return Ok(false);
    }
    remove(&mut doc).map_err(invalid)?;
    for set in mine.values.iter().rev() {
        take_back(&mut doc, set).map_err(invalid)?;
    }
    let changed = if mine.made_file && doc.value() == &json!({}) {
        fs::remove_file(file).map_err(|e| io_error("remove", file, e))?;
        true
    } else if doc.text() != before {
        write_atomic(file, doc.text())?;
        true
    } else {
        false
    };
    if notes.0.remove(&key).is_some() {
        save(record, &notes)?;
    }
    Ok(changed)
}

/// Put back what was there before Lanes set `set`, if it's still what Lanes set.
fn take_back(doc: &mut Doc, set: &Set) -> std::result::Result<(), String> {
    let key: Vec<&str> = set.key.iter().map(String::as_str).collect();
    if doc.get(&key) != Some(&set.set) {
        return Ok(());
    }
    match &set.was {
        Some(raw) => doc.replace(&key, raw),
        None => {
            doc.remove(&key)?;
            let parent = &key[..key.len().saturating_sub(1)];
            if set.made_parent && !parent.is_empty() && doc.get(parent) == Some(&json!({})) {
                doc.remove(parent)?;
            }
            Ok(())
        }
    }
}
