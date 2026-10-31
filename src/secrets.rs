//! Channel secrets live in the OS keyring, with a private file if that is unavailable.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use rand::RngCore;

pub const CHANNEL_NAME: &str = "METIS-INFO";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretStoreKind {
    Keyring,
    File,
}

impl SecretStoreKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Keyring => "keyring",
            Self::File => "file",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Keyring => "Operating system keyring",
            Self::File => "Private file under data/",
        }
    }
}

#[derive(Clone)]
pub struct Secrets {
    dir: PathBuf,
    prefer_file: bool,
}

impl Secrets {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            prefer_file: false,
        }
    }

    /// Test helper. Production uses the keyring and falls back to a file on its own.
    #[cfg(test)]
    pub fn file_only(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            prefer_file: true,
        }
    }

    pub fn generate() -> [u8; 16] {
        let mut secret = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut secret);
        secret
    }

    pub fn store(&self, name: &str, secret: &[u8; 16]) -> Result<SecretStoreKind, String> {
        if !self.prefer_file {
            if let Ok(kind) = self.store_keyring(name, secret) {
                return Ok(kind);
            }
        }
        self.store_file(name, secret)?;
        Ok(SecretStoreKind::File)
    }

    pub fn load(&self, name: &str) -> Result<Option<[u8; 16]>, String> {
        if !self.prefer_file {
            if let Some(secret) = self.load_keyring(name)? {
                return Ok(Some(secret));
            }
        }
        self.load_file(name)
    }

    fn store_keyring(&self, name: &str, secret: &[u8; 16]) -> Result<SecretStoreKind, String> {
        let entry = keyring::Entry::new("metis-mesh-hq", name).map_err(|err| err.to_string())?;
        entry
            .set_password(&hex::encode(secret))
            .map_err(|err| err.to_string())?;
        Ok(SecretStoreKind::Keyring)
    }

    fn load_keyring(&self, name: &str) -> Result<Option<[u8; 16]>, String> {
        let entry = match keyring::Entry::new("metis-mesh-hq", name) {
            Ok(entry) => entry,
            Err(_) => return Ok(None),
        };
        match entry.get_password() {
            Ok(value) => decode_hex_secret(&value).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Ok(None),
        }
    }

    fn store_file(&self, name: &str, secret: &[u8; 16]) -> Result<(), String> {
        fs::create_dir_all(&self.dir).map_err(|err| err.to_string())?;
        let path = self.file_path();
        let mut lines = self.read_lines();
        lines.retain(|line| line.split_whitespace().next() != Some(name));
        lines.push(format!("{name} {}", hex::encode(secret)));
        write_private(&path, lines.join("\n") + "\n")
    }

    fn load_file(&self, name: &str) -> Result<Option<[u8; 16]>, String> {
        for line in self.read_lines() {
            let mut parts = line.split_whitespace();
            if parts.next() == Some(name) {
                if let Some(value) = parts.next() {
                    return decode_hex_secret(value).map(Some);
                }
            }
        }
        Ok(None)
    }

    fn file_path(&self) -> PathBuf {
        self.dir.join("channel-secrets")
    }

    fn read_lines(&self) -> Vec<String> {
        fs::read_to_string(self.file_path())
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()
    }
}

fn decode_hex_secret(value: &str) -> Result<[u8; 16], String> {
    let bytes = hex::decode(value.trim()).map_err(|err| err.to_string())?;
    let secret: [u8; 16] = bytes
        .try_into()
        .map_err(|_| "channel secret must be 16 bytes".to_string())?;
    Ok(secret)
}

fn write_private(path: &Path, contents: String) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|err| err.to_string())?;
    file.write_all(contents.as_bytes())
        .map_err(|err| err.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_store_round_trip() {
        let dir = std::env::temp_dir().join(format!(
            "metis-secrets-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let secrets = Secrets::file_only(&dir);
        let value = Secrets::generate();
        assert_eq!(
            secrets.store("METIS-INFO", &value).unwrap(),
            SecretStoreKind::File
        );
        assert_eq!(secrets.load("METIS-INFO").unwrap(), Some(value));
        let _ = fs::remove_dir_all(&dir);
    }
}
