//! Atomic, versioned state-file persistence.
//!
//! A write goes to a sibling temporary file, is flushed to disk, and then
//! replaces the target in one rename/replace operation. Before replacing a
//! valid target, its bytes are also written to `<path>.bak`, which is used as a
//! known-good fallback when a later load finds a damaged primary file.

use serde::de::DeserializeOwned;
use std::fs::{self, OpenOptions};
#[cfg(unix)]
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn backup_path(path: &Path) -> PathBuf {
    let mut backup = path.as_os_str().to_os_string();
    backup.push(".bak");
    PathBuf::from(backup)
}

fn temp_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state");
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(".{name}.{}.{counter}.tmp", std::process::id()))
}

/// Write bytes to a sibling temporary file, sync it, then atomically replace
/// the target. The target is never truncated in place.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = temp_path(path);
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        replace_file(&temporary, path)?;
        sync_parent(path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Write a new state file and retain the previous target as a known-good
/// backup. If the backup cannot be written, the new target is not replaced.
pub fn write_with_backup(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if path.exists() {
        let previous = fs::read(path)?;
        write_atomic(&backup_path(path), &previous)?;
    }
    write_atomic(path, bytes)
}

/// Load a versioned JSON state file, falling back to its `.bak` copy when the
/// primary file is missing or invalid. A successful fallback repairs the
/// primary file atomically and returns a warning describing the recovery.
pub fn load_json<T: DeserializeOwned>(
    path: &Path,
    current_version: u32,
    version: impl Fn(&T) -> u32,
) -> Result<(Option<T>, Option<String>), String> {
    let primary = read_optional(path)?;
    if let Some(raw) = primary {
        match parse_versioned::<T>(&raw, current_version, &version) {
            Ok(value) => return Ok((Some(value), None)),
            Err(primary_error) => {
                if let Some(future) = future_version(&raw, current_version) {
                    return Err(format!(
                        "{}: {future}; refusing to fall back to an older backup",
                        path.display()
                    ));
                }
                let backup = backup_path(path);
                let Some(backup_raw) = read_optional(&backup)? else {
                    return Err(format!(
                        "{} is invalid and no known-good backup exists: {primary_error}",
                        path.display()
                    ));
                };
                match parse_versioned::<T>(&backup_raw, current_version, &version) {
                    Ok(value) => {
                        let warning = format!(
                            "{} was invalid ({primary_error}); recovered {}",
                            path.display(),
                            backup.display()
                        );
                        if let Err(err) = write_atomic(path, backup_raw.as_bytes()) {
                            return Err(format!(
                                "{warning}; could not repair the primary file: {err}. The backup was left untouched."
                            ));
                        }
                        return Ok((Some(value), Some(warning)));
                    }
                    Err(backup_error) => {
                        return Err(format!(
                            "{} is invalid ({primary_error}) and {} is invalid ({backup_error})",
                            path.display(),
                            backup.display()
                        ));
                    }
                }
            }
        }
    }

    let backup = backup_path(path);
    let Some(backup_raw) = read_optional(&backup)? else {
        return Ok((None, None));
    };
    match parse_versioned::<T>(&backup_raw, current_version, &version) {
        Ok(value) => {
            let warning = format!(
                "{} was missing; recovered {}",
                path.display(),
                backup.display()
            );
            if let Err(err) = write_atomic(path, backup_raw.as_bytes()) {
                return Err(format!(
                    "{warning}; could not restore the primary file: {err}. The backup was left untouched."
                ));
            }
            Ok((Some(value), Some(warning)))
        }
        Err(backup_error) => Err(format!(
            "{} is missing and {} is invalid ({backup_error})",
            path.display(),
            backup.display()
        )),
    }
}

fn parse_versioned<T: DeserializeOwned>(
    raw: &str,
    current_version: u32,
    version: &impl Fn(&T) -> u32,
) -> Result<T, String> {
    let value: T = serde_json::from_str(raw).map_err(|err| err.to_string())?;
    let found = version(&value);
    if found > current_version {
        return Err(format!(
            "version {found} is newer than the supported version {current_version}"
        ));
    }
    Ok(value)
}

fn future_version(raw: &str, current_version: u32) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let found = value.get("version")?.as_u64()?;
    (found > u64::from(current_version)).then(|| {
        format!("version {found} is newer than the supported version {current_version}")
    })
}

fn read_optional(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(raw) => Ok(Some(raw)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("could not read {}: {err}", path.display())),
    }
}

#[cfg(windows)]
fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    let result = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_keeps_previous_target_as_backup() {
        let directory = std::env::temp_dir().join(format!(
            "gmo-persist-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let path = directory.join("state.json");
        fs::create_dir_all(&directory).unwrap();
        fs::write(&path, b"old").unwrap();

        write_with_backup(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(fs::read(backup_path(&path)).unwrap(), b"old");
        let temporary = fs::read_dir(&directory)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"));
        assert!(!temporary, "temporary file should be consumed");

        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn invalid_primary_recovers_the_backup_and_repairs_the_primary() {
        #[derive(Debug, serde::Deserialize)]
        struct Versioned {
            #[serde(default)]
            version: u32,
            value: String,
        }
        fn version(value: &Versioned) -> u32 {
            value.version
        }

        let directory = std::env::temp_dir().join(format!(
            "gmo-persist-recovery-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let path = directory.join("state.json");
        fs::create_dir_all(&directory).unwrap();
        fs::write(&path, br#"{"version":1,"value":"old"}"#).unwrap();
        write_with_backup(&path, br#"{"version":1,"value":"new"}"#).unwrap();
        fs::write(&path, b"not json").unwrap();

        let (value, warning) = load_json::<Versioned>(&path, 1, version).unwrap();
        assert_eq!(value.unwrap().value, "old");
        assert!(warning.unwrap().contains("recovered"));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            r#"{"version":1,"value":"old"}"#
        );

        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn future_versions_are_rejected_instead_of_downgraded() {
        #[derive(Debug, serde::Deserialize)]
        struct Versioned {
            #[serde(default)]
            version: u32,
        }
        fn version(value: &Versioned) -> u32 {
            value.version
        }

        let directory = std::env::temp_dir().join(format!(
            "gmo-persist-future-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let path = directory.join("state.json");
        fs::create_dir_all(&directory).unwrap();
        fs::write(&path, br#"{"version":2}"#).unwrap();
        fs::write(backup_path(&path), br#"{"version":1}"#).unwrap();

        let error = load_json::<Versioned>(&path, 1, version).unwrap_err();
        assert!(error.contains("refusing to fall back"), "{error}");

        let _ = fs::remove_dir_all(directory);
    }
}
