use std::fmt::{self, Write};
use std::path::{Path, PathBuf};

use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce, Tag};
use base64::Engine;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use zeroize::Zeroizing;

use crate::json::Value;
use crate::text::Text;
use crate::{Error, config, paths};

mod platform;

pub const MAX_SECRET_BYTES: usize = 65_536;
pub const MAX_ENVELOPE_BYTES: usize = MAX_SECRET_BYTES.div_ceil(3) * 4 + 256;
pub const MAX_DIRECTORY_ENTRIES: usize = 4_096;
pub const MAX_PATH_COMPONENTS: usize = 256;
pub const MAX_PATH_UNITS: usize = 32_766;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultError {
    Name,
    AppCredential,
    Value,
    PrivateKey,
    Directory,
    Containment,
    Linked,
    Ownership,
    Permissions,
    Changed,
    MissingKey,
    KeySize,
    Envelope,
    InvalidEntry,
    ForbiddenEntry,
    Limit,
    Random,
    Io,
    Read,
    Write,
    List,
    Cleanup,
    Unsupported,
}

impl VaultError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Name => "ROSTER_VAULT_NAME",
            Self::AppCredential => "ROSTER_VAULT_APP_CREDENTIAL",
            Self::Value => "ROSTER_VAULT_VALUE",
            Self::PrivateKey => "ROSTER_VAULT_PRIVATE_KEY",
            Self::Directory => "ROSTER_VAULT_DIRECTORY",
            Self::Containment => "ROSTER_VAULT_CONTAINMENT",
            Self::Linked => "ROSTER_VAULT_LINKED",
            Self::Ownership => "ROSTER_VAULT_OWNERSHIP",
            Self::Permissions => "ROSTER_VAULT_PERMISSIONS",
            Self::Changed => "ROSTER_VAULT_CHANGED",
            Self::MissingKey => "ROSTER_VAULT_MISSING_KEY",
            Self::KeySize => "ROSTER_VAULT_KEY_SIZE",
            Self::Envelope => "ROSTER_VAULT_AUTHENTICATION",
            Self::InvalidEntry => "ROSTER_VAULT_ENTRY",
            Self::ForbiddenEntry => "ROSTER_VAULT_FORBIDDEN_ENTRY",
            Self::Limit => "ROSTER_VAULT_LIMIT",
            Self::Random => "ROSTER_VAULT_RANDOM",
            Self::Io => "ROSTER_VAULT_IO",
            Self::Read => "ROSTER_VAULT_READ",
            Self::Write => "ROSTER_VAULT_WRITE",
            Self::List => "ROSTER_VAULT_LIST",
            Self::Cleanup => "ROSTER_VAULT_CLEANUP",
            Self::Unsupported => "ROSTER_VAULT_UNSUPPORTED",
        }
    }
}

impl fmt::Display for VaultError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(match self {
            Self::Name => "Secret names must be 1-64 ASCII letters, digits or underscores, starting with a letter or underscore.",
            Self::AppCredential => "GitHub App credentials cannot be stored in the LLM vault.",
            Self::Value => "A secret must be a non-empty string.",
            Self::PrivateKey => "PEM private keys must not be stored in the LLM vault.",
            Self::Directory => "The vault directory must be a non-empty, bounded filesystem path.",
            Self::Containment => "The file vault must be outside a Git worktree, with no symbolic links or reparse points in its path.",
            Self::Linked => "Vault files must be regular files with one link, not symbolic links or reparse points. The vault location must be a directory, not a symbolic link.",
            Self::Ownership => "The file vault contains an object not owned by the effective OS user; preserve it and choose an owned vault.",
            Self::Permissions => "Unable to secure or verify owner-only vault permissions; preserve the vault and check filesystem ACL support and access.",
            Self::Changed => "The file vault path or file identity changed during the operation; preserve it and retry only after resolving the path change.",
            Self::MissingKey => "The vault key is missing; restore it before reading or writing existing entries.",
            Self::KeySize => "The vault key must contain 32 bytes; preserve it and restore the original complete key.",
            Self::Envelope => "The vault entry is invalid or could not be authenticated; preserve the entry and its original key.",
            Self::InvalidEntry => "The vault contains an invalid entry; preserve it for explicit recovery.",
            Self::ForbiddenEntry => "The vault contains a forbidden GitHub App entry.",
            Self::Limit => "The file vault exceeds a bounded input limit; preserve it and consult the documented vault limits.",
            Self::Random => "The operating system could not provide cryptographic randomness; this operation did not publish a secret entry. Preserve any existing vault key.",
            Self::Io => "Unable to read, list or write the file vault. Check its location and permissions.",
            Self::Read => "Unable to read the file vault. Check its location and permissions.",
            Self::Write => "Unable to write the file vault. Check its location and permissions.",
            Self::List => "Unable to list the file vault. Check its location and permissions.",
            Self::Cleanup => "Unable to remove an owned incomplete vault write; preserve the vault for explicit recovery.",
            Self::Unsupported => "Native secure vault operations are unavailable on this platform or filesystem; no weaker provider was substituted.",
        })
    }
}

impl std::error::Error for VaultError {}

impl From<VaultError> for Error {
    fn from(error: VaultError) -> Self {
        let name = if matches!(
            error,
            VaultError::Name
                | VaultError::AppCredential
                | VaultError::Value
                | VaultError::PrivateKey
                | VaultError::Directory
        ) {
            "TypeError"
        } else {
            "VaultError"
        };
        Self::new(name, error.to_string()).coded(error.code())
    }
}

pub fn validate_secret_name(name: &Text) -> Result<(), VaultError> {
    if !crate::llm::redact::valid_secret_identifier(name) {
        return Err(VaultError::Name);
    }
    if !crate::llm::redact::valid_secret_name(name) {
        return Err(VaultError::AppCredential);
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), VaultError> {
    if name.len() > 64 {
        return Err(VaultError::Name);
    }
    validate_secret_name(&Text::from(name))
}

fn validate_value(value: &str) -> Result<(), VaultError> {
    if value.is_empty() {
        return Err(VaultError::Value);
    }
    if value.len() > MAX_SECRET_BYTES {
        return Err(VaultError::Limit);
    }
    let private_key = regex_lite::Regex::new(r"(?i)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----")
        .map_err(|_| VaultError::Unsupported)?;
    if private_key.is_match(value) {
        return Err(VaultError::PrivateKey);
    }
    Ok(())
}

fn random<const N: usize>() -> Result<[u8; N], VaultError> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(|_| VaultError::Random)?;
    Ok(bytes)
}

fn hexadecimal(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        let _ = write!(&mut encoded, "{byte:02x}");
    }
    encoded
}

fn entry_name(name: &str) -> String {
    format!("{}.json", hexadecimal(name.as_bytes()))
}

fn entry_secret_name(filename: &str) -> Result<Option<String>, VaultError> {
    let Some(encoded) = filename.strip_suffix(".json") else {
        return Ok(None);
    };
    if !(2..=128).contains(&encoded.len())
        || encoded.len() % 2 != 0
        || !encoded
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(VaultError::InvalidEntry);
    }
    let bytes = encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|text| u8::from_str_radix(text, 16).ok())
                .ok_or(VaultError::InvalidEntry)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let name = String::from_utf8(bytes).map_err(|_| VaultError::InvalidEntry)?;
    validate_secret_name(&Text::from(name.as_str())).map_err(|error| {
        if error == VaultError::AppCredential {
            VaultError::ForbiddenEntry
        } else {
            VaultError::InvalidEntry
        }
    })?;
    Ok(Some(name))
}

fn base64() -> GeneralPurpose {
    GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
    )
}

fn encrypt(name: &str, value: &str, key: &[u8], iv: &[u8; 12]) -> Result<Vec<u8>, VaultError> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| VaultError::KeySize)?;
    let mut ciphertext = Zeroizing::new(value.as_bytes().to_vec());
    let tag = cipher
        .encrypt_in_place_detached(Nonce::from_slice(iv), name.as_bytes(), &mut ciphertext)
        .map_err(|_| VaultError::Envelope)?;
    Ok(format!(
        "{{\"version\":1,\"iv\":\"{}\",\"tag\":\"{}\",\"ciphertext\":\"{}\"}}",
        base64().encode(iv),
        base64().encode(tag),
        base64().encode(&*ciphertext),
    )
    .into_bytes())
}

fn decrypt(name: &str, stored: &[u8], key: &[u8]) -> Result<String, VaultError> {
    let entry: serde_json::Value =
        serde_json::from_slice(stored).map_err(|_| VaultError::Envelope)?;
    if entry.get("version").and_then(serde_json::Value::as_f64) != Some(1.0) {
        return Err(VaultError::Envelope);
    }
    let decode = |field| {
        let value = entry
            .get(field)
            .and_then(serde_json::Value::as_str)
            .ok_or(VaultError::Envelope)?;
        base64().decode(value).map_err(|_| VaultError::Envelope)
    };
    let iv = decode("iv")?;
    let tag = decode("tag")?;
    let mut ciphertext = Zeroizing::new(decode("ciphertext")?);
    if iv.len() != 12 || tag.len() != 16 {
        return Err(VaultError::Envelope);
    }
    if ciphertext.len() > MAX_SECRET_BYTES {
        return Err(VaultError::Limit);
    }
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| VaultError::KeySize)?;
    cipher
        .decrypt_in_place_detached(
            Nonce::from_slice(&iv),
            name.as_bytes(),
            &mut ciphertext,
            Tag::from_slice(&tag),
        )
        .map_err(|_| VaultError::Envelope)?;
    let value = std::str::from_utf8(&ciphertext).map_err(|_| VaultError::Envelope)?;
    Ok(value.to_owned())
}

pub struct FileVault {
    directory: PathBuf,
}

impl FileVault {
    pub fn new(directory: impl AsRef<Path>) -> Result<Self, VaultError> {
        let path = directory.as_ref();
        let text = path.to_str().ok_or(VaultError::Directory)?;
        if text.encode_utf16().take(MAX_PATH_UNITS + 1).count() > MAX_PATH_UNITS
            || text.encode_utf16().all(crate::text::is_space)
        {
            return Err(VaultError::Directory);
        }
        let directory = paths::absolute(path).map_err(|_| VaultError::Directory)?;
        if directory.file_name().is_none()
            || directory.components().count() > MAX_PATH_COMPONENTS
            || directory
                .to_str()
                .ok_or(VaultError::Directory)?
                .encode_utf16()
                .count()
                > MAX_PATH_UNITS
        {
            return Err(VaultError::Directory);
        }
        Ok(Self { directory })
    }

    pub fn from_environment(environment: &Value) -> Result<Self, Error> {
        let home = config::environment_value(
            environment,
            if cfg!(windows) { "USERPROFILE" } else { "HOME" },
        )?
        .ok_or_else(|| Error::from(VaultError::Directory))?
        .to_utf8()
        .map_err(|_| Error::from(VaultError::Directory))?;
        Self::new(Path::new(&home).join(".roster").join("vault")).map_err(Error::from)
    }

    fn names(directory: &platform::Directory) -> Result<Vec<String>, VaultError> {
        let mut names = Vec::new();
        for filename in directory.entries()? {
            if let Some(name) = entry_secret_name(&filename)? {
                directory
                    .inspect(&filename)
                    .map_err(|error| {
                        if error == VaultError::Linked {
                            VaultError::InvalidEntry
                        } else {
                            error
                        }
                    })?
                    .ok_or(VaultError::Changed)?;
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }

    fn key(
        directory: &platform::Directory,
        create: bool,
    ) -> Result<Zeroizing<Vec<u8>>, VaultError> {
        let mut key = directory.read(".key", 33)?;
        if key.is_none() && create {
            if !Self::names(directory)?.is_empty() {
                key = directory.read(".key", 33)?;
                if key.is_none() {
                    return Err(VaultError::MissingKey);
                }
            } else {
                let mut generated = Zeroizing::new(vec![0; 32]);
                getrandom::fill(&mut generated).map_err(|_| VaultError::Random)?;
                directory.atomic(".key", &generated, true)?;
                key = directory.read(".key", 33)?;
            }
        }
        let key = key.ok_or(VaultError::MissingKey)?;
        if key.len() != 32 {
            return Err(VaultError::KeySize);
        }
        Ok(key)
    }

    pub fn get(&self, name: &str) -> Result<Option<String>, VaultError> {
        self.read_entry(name).map_err(|error| {
            if error == VaultError::Io {
                VaultError::Read
            } else {
                error
            }
        })
    }

    fn read_entry(&self, name: &str) -> Result<Option<String>, VaultError> {
        validate_name(name)?;
        let Some(directory) = platform::Directory::prepare(&self.directory, false)? else {
            return Ok(None);
        };
        let Some(stored) = directory.read(&entry_name(name), MAX_ENVELOPE_BYTES)? else {
            return Ok(None);
        };
        let key = Self::key(&directory, false)?;
        let value = decrypt(name, &stored, &key)?;
        directory.current()?;
        Ok(Some(value))
    }

    pub fn set(&self, name: &str, value: &str) -> Result<(), VaultError> {
        self.write_entry(name, value).map_err(|error| {
            if error == VaultError::Io {
                VaultError::Write
            } else {
                error
            }
        })
    }

    fn write_entry(&self, name: &str, value: &str) -> Result<(), VaultError> {
        validate_name(name)?;
        validate_value(value)?;
        let directory =
            platform::Directory::prepare(&self.directory, true)?.ok_or(VaultError::Directory)?;
        let key = Self::key(&directory, true)?;
        let stored = encrypt(name, value, &key, &random::<12>()?)?;
        directory.atomic(&entry_name(name), &stored, false)?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<String>, VaultError> {
        self.list_entries().map_err(|error| {
            if error == VaultError::Io {
                VaultError::List
            } else {
                error
            }
        })
    }

    fn list_entries(&self) -> Result<Vec<String>, VaultError> {
        let Some(directory) = platform::Directory::prepare(&self.directory, false)? else {
            return Ok(Vec::new());
        };
        let names = Self::names(&directory)?;
        directory.current()?;
        Ok(names)
    }
}

#[cfg(test)]
mod tests;
