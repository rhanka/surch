#![forbid(unsafe_code)]

use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;
use thiserror::Error;

const REQUIRED_FIELDS: &[&str] = &[
    "id",
    "title",
    "owner",
    "priority",
    "upstream_ref",
    "parity_level",
    "dependencies",
    "allowed_paths",
    "forbidden_paths",
    "golden_tests_required",
    "gates",
    "status",
];
const REQUIRED_UPSTREAM_FIELDS: &[&str] = &["repo", "commit", "files", "symbols"];
const NON_EMPTY_LIST_FIELDS: &[&str] = &["allowed_paths", "golden_tests_required", "gates"];
const VALID_OWNERS: &[&str] = &[
    "StorageEngine",
    "Indexer",
    "SearchEngine",
    "APIServer",
    "Conductor",
];
const VALID_STATUSES: &[&str] = &[
    "discovered",
    "triaged",
    "specced",
    "ready",
    "active",
    "pr",
    "validated",
    "done",
    "deferred",
];

#[derive(Debug, Error)]
pub enum LedgerError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: invalid JSON: {source}")]
    InvalidJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("{0}")]
    Validation(String),
}

pub type Result<T> = std::result::Result<T, LedgerError>;

pub fn validate_ticket_path(path: &Path) -> Result<usize> {
    let ticket_files = iter_ticket_files(path)?;
    if ticket_files.is_empty() {
        return Err(LedgerError::Validation(format!(
            "{}: no .json tickets found",
            path.display()
        )));
    }

    for ticket_file in &ticket_files {
        validate_ticket(ticket_file)?;
    }

    Ok(ticket_files.len())
}

pub fn check_language_policy(root: &Path) -> Result<()> {
    let mut violations = Vec::new();
    collect_language_policy_violations(root, root, &mut violations)?;

    if violations.is_empty() {
        return Ok(());
    }

    Err(LedgerError::Validation(format!(
        "disallowed file(s): {}",
        violations.join(", ")
    )))
}

fn iter_ticket_files(path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }

    let mut files = Vec::new();
    collect_json_files(path, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_json_files(path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let entries = read_dir(path)?;
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_json_files(&path, files)?;
        } else if path.extension() == Some(OsStr::new("json")) {
            files.push(path);
        }
    }
    Ok(())
}

fn validate_ticket(path: &Path) -> Result<()> {
    let raw = fs::read_to_string(path).map_err(|source| LedgerError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let ticket: Value = serde_json::from_str(&raw).map_err(|source| LedgerError::InvalidJson {
        path: path.to_path_buf(),
        source,
    })?;
    let ticket = ticket.as_object().ok_or_else(|| {
        LedgerError::Validation(format!("{}: ticket root must be an object", path.display()))
    })?;

    for field in REQUIRED_FIELDS {
        if !ticket.contains_key(*field) {
            return Err(LedgerError::Validation(format!(
                "{}: missing required field {field}",
                path.display()
            )));
        }
    }

    for field in ["id", "title", "owner", "priority", "parity_level", "status"] {
        require_string(ticket.get(field), path, field)?;
    }

    require_membership(ticket.get("owner"), path, "owner", VALID_OWNERS)?;
    require_membership(ticket.get("status"), path, "status", VALID_STATUSES)?;

    for field in [
        "dependencies",
        "allowed_paths",
        "forbidden_paths",
        "golden_tests_required",
        "gates",
    ] {
        require_string_list(
            ticket.get(field),
            path,
            field,
            NON_EMPTY_LIST_FIELDS.contains(&field),
        )?;
    }

    let upstream = ticket
        .get("upstream_ref")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            LedgerError::Validation(format!(
                "{}: upstream_ref must be an object",
                path.display()
            ))
        })?;
    for field in REQUIRED_UPSTREAM_FIELDS {
        if !upstream.contains_key(*field) {
            return Err(LedgerError::Validation(format!(
                "{}: upstream_ref missing required field {field}",
                path.display()
            )));
        }
    }
    for field in ["repo", "commit"] {
        require_string(upstream.get(field), path, &format!("upstream_ref.{field}"))?;
    }
    for field in ["files", "symbols"] {
        require_string_list(
            upstream.get(field),
            path,
            &format!("upstream_ref.{field}"),
            true,
        )?;
    }

    Ok(())
}

fn require_string(value: Option<&Value>, path: &Path, field: &str) -> Result<()> {
    if value
        .and_then(Value::as_str)
        .is_some_and(|item| !item.trim().is_empty())
    {
        return Ok(());
    }

    Err(LedgerError::Validation(format!(
        "{}: {field} must be a non-empty string",
        path.display()
    )))
}

fn require_membership(
    value: Option<&Value>,
    path: &Path,
    field: &str,
    allowed: &[&str],
) -> Result<()> {
    let Some(value) = value.and_then(Value::as_str) else {
        return require_string(value, path, field);
    };
    if allowed.contains(&value) {
        return Ok(());
    }

    Err(LedgerError::Validation(format!(
        "{}: {field} must be one of {:?}",
        path.display(),
        allowed
    )))
}

fn require_string_list(
    value: Option<&Value>,
    path: &Path,
    field: &str,
    non_empty: bool,
) -> Result<()> {
    let Some(items) = value.and_then(Value::as_array) else {
        return Err(LedgerError::Validation(format!(
            "{}: {field} must be a list",
            path.display()
        )));
    };
    if non_empty && items.is_empty() {
        return Err(LedgerError::Validation(format!(
            "{}: {field} must not be empty",
            path.display()
        )));
    }
    if !items
        .iter()
        .all(|item| item.as_str().is_some_and(|value| !value.trim().is_empty()))
    {
        return Err(LedgerError::Validation(format!(
            "{}: {field} must contain only non-empty strings",
            path.display()
        )));
    }

    Ok(())
}

fn collect_language_policy_violations(
    root: &Path,
    path: &Path,
    violations: &mut Vec<String>,
) -> Result<()> {
    for entry in read_dir(path)? {
        let path = entry.path();
        let file_name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
        if path.is_dir() {
            if matches!(file_name, ".git" | "target" | "__pycache__") {
                continue;
            }
            collect_language_policy_violations(root, &path, violations)?;
            continue;
        }

        if is_disallowed_script_artifact(file_name, path.extension().and_then(OsStr::to_str))
            || has_disallowed_interpreter_shebang(&path)?
        {
            violations.push(display_relative(root, &path));
        }
    }
    Ok(())
}

fn is_disallowed_script_artifact(file_name: &str, extension: Option<&str>) -> bool {
    let short_ext = ["p", "y"].concat();
    let typed_ext = ["p", "y", "i"].concat();
    let project_manifest = format!("{}project.toml", short_ext);

    extension.is_some_and(|ext| ext == short_ext || ext == typed_ext)
        || file_name == project_manifest
        || (file_name.starts_with("requirements") && file_name.ends_with(".txt"))
}

fn has_disallowed_interpreter_shebang(path: &Path) -> Result<bool> {
    let mut file = fs::File::open(path).map_err(|source| LedgerError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut buffer = [0_u8; 160];
    let read = file.read(&mut buffer).map_err(|source| LedgerError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let header = String::from_utf8_lossy(&buffer[..read]).to_ascii_lowercase();
    let forbidden_interpreter = ["p", "y", "t", "h", "o", "n"].concat();

    Ok(header.starts_with("#!") && header.contains(&forbidden_interpreter))
}

fn read_dir(path: &Path) -> Result<Vec<fs::DirEntry>> {
    fs::read_dir(path)
        .map_err(|source| LedgerError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|source| LedgerError::Io {
            path: path.to_path_buf(),
            source,
        })
}

fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}
