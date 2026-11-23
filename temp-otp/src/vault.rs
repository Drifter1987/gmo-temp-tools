//! The small local authenticator vault. The same file is used by the desktop
//! OTP tab and the web app's OTP API.
//!
//! On Windows the serialized vault is encrypted with DPAPI for the current
//! user. A per-file lock serializes the desktop tab and the web server, which
//! otherwise both read and rewrite the vault.

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

const MAGIC: &[u8] = b"GMOOTP1\0";
const ENTROPY: &[u8] = b"GMO TempTools OTP vault v1";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredSecret {
    pub label: String,
    pub secret: String,
    pub digits: u32,
    pub period: u32,
    pub algorithm: String,
}

impl StoredSecret {
    pub fn from_input(raw: &str, label: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        if trimmed.to_lowercase().starts_with("otpauth://") {
            let params = crate::parse_otpauth(trimmed)?;
            let name = if !label.trim().is_empty() {
                label.trim().to_string()
            } else if !params.issuer.is_empty() {
                format!("{} · {}", params.issuer, params.account)
            } else {
                params.account.clone()
            };
            return Some(Self {
                label: name,
                secret: params.secret,
                digits: params.digits,
                period: params.period,
                algorithm: params.algorithm,
            });
        }
        let compact: String = trimmed
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-')
            .collect::<String>()
            .to_uppercase();
        if compact.len() < 8 {
            return None;
        }
        Some(Self {
            label: if label.trim().is_empty() {
                "Secret".to_string()
            } else {
                label.trim().to_string()
            },
            secret: compact,
            digits: 6,
            period: 30,
            algorithm: "SHA1".to_string(),
        })
    }

    pub fn code(&self, now: f64) -> String {
        crate::totp(&self.secret, self.digits, self.period, &self.algorithm, now)
    }

    pub fn masked(&self) -> String {
        "••••••••".to_string()
    }
}

pub fn default_path() -> PathBuf {
    PathBuf::from("state").join("vault.json")
}

fn lock_path(path: &Path) -> PathBuf {
    path.with_extension("json.lock")
}

fn lock(path: &Path) -> Result<File, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create vault directory: {error}"))?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(lock_path(path))
        .map_err(|error| format!("cannot open vault lock: {error}"))?;
    file.lock_exclusive()
        .map_err(|error| format!("cannot lock the vault: {error}"))?;
    Ok(file)
}

fn read_unlocked(path: &Path) -> Result<Vec<StoredSecret>, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot read the vault: {error}")),
    };
    let legacy = !bytes.starts_with(MAGIC);
    let secrets: Vec<StoredSecret> = if legacy {
        // A vault written before encryption was enabled is still readable. It
        // is encrypted immediately below rather than waiting for an edit.
        serde_json::from_slice(&bytes).map_err(|error| format!("cannot parse the vault: {error}"))?
    } else {
        let plain = unseal(&bytes[MAGIC.len()..])?;
        serde_json::from_slice(&plain).map_err(|error| format!("cannot parse the vault: {error}"))?
    };
    if legacy {
        write_unlocked(path, &secrets)?;
    }
    Ok(secrets)
}

fn write_unlocked(path: &Path, secrets: &[StoredSecret]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create vault directory: {error}"))?;
    }
    let plain = serde_json::to_vec_pretty(secrets)
        .map_err(|error| format!("cannot serialize the vault: {error}"))?;
    let mut document = MAGIC.to_vec();
    document.extend_from_slice(&seal(&plain)?);

    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, &document)
        .map_err(|error| format!("cannot write the vault: {error}"))?;
    if fs::rename(&temporary, path).is_err() {
        // Windows can refuse a replace rename. The lock still prevents a
        // concurrent reader from seeing a partial document.
        fs::write(path, &document).map_err(|error| format!("cannot replace the vault: {error}"))?;
        let _ = fs::remove_file(&temporary);
    }
    Ok(())
}

/// Read the vault, keeping the shared lock for the duration of the read.
pub fn load_result(path: &Path) -> Result<Vec<StoredSecret>, String> {
    let _guard = lock(path)?;
    read_unlocked(path)
}

/// Atomically read, change and write the vault while holding its lock. Every
/// mutation goes through this function so two windows cannot lose an update.
pub fn update<F, R>(path: &Path, change: F) -> Result<R, String>
where
    F: FnOnce(&mut Vec<StoredSecret>) -> R,
{
    let _guard = lock(path)?;
    let mut secrets = read_unlocked(path)?;
    let result = change(&mut secrets);
    write_unlocked(path, &secrets)?;
    Ok(result)
}

#[cfg(windows)]
fn seal(plain: &[u8]) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let mut input = CRYPT_INTEGER_BLOB {
        cbData: plain.len() as u32,
        pbData: plain.as_ptr() as *mut u8,
    };
    let mut entropy = CRYPT_INTEGER_BLOB {
        cbData: ENTROPY.len() as u32,
        pbData: ENTROPY.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        CryptProtectData(
            &mut input,
            std::ptr::null(),
            &mut entropy,
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(format!(
            "DPAPI could not encrypt the vault: {}",
            std::io::Error::last_os_error()
        ));
    }
    let encrypted = unsafe {
        std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
    };
    unsafe {
        LocalFree(output.pbData as *mut _);
    }
    Ok(encrypted)
}

#[cfg(windows)]
fn unseal(encrypted: &[u8]) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let mut input = CRYPT_INTEGER_BLOB {
        cbData: encrypted.len() as u32,
        pbData: encrypted.as_ptr() as *mut u8,
    };
    let mut entropy = CRYPT_INTEGER_BLOB {
        cbData: ENTROPY.len() as u32,
        pbData: ENTROPY.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        CryptUnprotectData(
            &mut input,
            std::ptr::null_mut(),
            &mut entropy,
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(format!(
            "DPAPI could not decrypt the vault: {}",
            std::io::Error::last_os_error()
        ));
    }
    let plain = unsafe {
        std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
    };
    unsafe {
        LocalFree(output.pbData as *mut _);
    }
    Ok(plain)
}

#[cfg(not(windows))]
fn seal(_plain: &[u8]) -> Result<Vec<u8>, String> {
    Err("the OTP vault requires Windows DPAPI".to_string())
}

#[cfg(not(windows))]
fn unseal(_encrypted: &[u8]) -> Result<Vec<u8>, String> {
    Err("the OTP vault requires Windows DPAPI".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_path(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "gmo-vault-{name}-{}-{stamp}.json",
            std::process::id()
        ))
    }

    fn secret(label: &str) -> StoredSecret {
        StoredSecret::from_input("JBSWY3DPEHPK3PXP", label).expect("valid test secret")
    }

    #[test]
    fn encrypted_vault_round_trips_without_plaintext_on_disk() {
        let path = test_path("roundtrip");
        update(&path, |secrets| secrets.push(secret("Work"))).expect("update vault");
        let bytes = fs::read(&path).expect("read vault");
        assert!(bytes.starts_with(MAGIC));
        assert!(!String::from_utf8_lossy(&bytes).contains("JBSWY3DPEHPK3PXP"));
        assert_eq!(load_result(&path).expect("load vault"), vec![secret("Work")]);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(lock_path(&path));
    }

    #[test]
    fn display_mask_does_not_reveal_secret_prefix() {
        let secret = secret("Work");
        assert_eq!(secret.masked(), "••••••••");
        assert!(!secret.masked().contains(&secret.secret[..4]));
    }

    #[test]
    fn legacy_plaintext_vault_is_migrated_on_the_next_write() {
        let path = test_path("legacy");
        fs::write(&path, serde_json::to_vec(&vec![secret("Old")]).unwrap()).unwrap();
        assert_eq!(load_result(&path).expect("load legacy vault"), vec![secret("Old")]);
        assert!(fs::read(&path).expect("read vault").starts_with(MAGIC));
        update(&path, |secrets| secrets.push(secret("New"))).expect("update vault");
        let bytes = fs::read(&path).expect("read vault");
        assert!(bytes.starts_with(MAGIC));
        assert_eq!(load_result(&path).expect("load vault").len(), 2);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(lock_path(&path));
    }

    #[test]
    fn concurrent_updates_are_serialized() {
        let path = Arc::new(test_path("concurrent"));
        let mut workers = Vec::new();
        for index in 0..8 {
            let path = path.clone();
            workers.push(thread::spawn(move || {
                update(&path, |secrets| {
                    secrets.push(secret(&format!("Account {index}")));
                })
                .expect("update vault");
            }));
        }
        for worker in workers {
            worker.join().expect("vault writer");
        }

        assert_eq!(load_result(&path).expect("load vault").len(), 8);
        let _ = fs::remove_file(path.as_ref());
        let _ = fs::remove_file(lock_path(path.as_ref()));
    }
}
