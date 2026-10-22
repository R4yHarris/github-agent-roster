use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use super::*;

const SECRET: &str = "test-only-vault-secret \u{2603} ";
const WIRE: &str = "{\"version\":1,\"iv\":\"AAECAwQFBgcICQoL\",\"tag\":\"WmVegJos6NhxhOr3A93eIA==\",\"ciphertext\":\"M2elb+iKrHf0bOHqxIUMQPCz5EaVD3+eoOTF\"}";

struct Fixture {
    root: PathBuf,
    directory: PathBuf,
}

fn io(_: std::io::Error) -> Error {
    Error::new(
        "FixtureError",
        "Owned vault fixture filesystem operation failed",
    )
}

impl Fixture {
    fn new() -> Result<Self, Error> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .ok_or_else(|| Error::type_error("Vault fixture needs an outside-Git parent"))?;
        let root = parent.join(format!(
            ".native-vault-{}-{}",
            std::process::id(),
            hexadecimal(&random::<16>()?)
        ));
        fs::create_dir(&root).map_err(io)?;
        eprintln!("Owned vault fixture: {}", root.display());
        Ok(Self {
            directory: root.join("vault"),
            root,
        })
    }

    fn vault(&self) -> Result<FileVault, VaultError> {
        FileVault::new(&self.directory)
    }

    fn finish(self) -> Result<(), Error> {
        fs::remove_dir_all(&self.root).map_err(io)
    }
}

fn assert_safe(error: VaultError, fixture: &Fixture) {
    let error = Error::from(error);
    assert!(error.cause.is_none());
    let text = format!("{error:?} {error}");
    for value in [
        SECRET,
        &fixture.root.to_string_lossy(),
        "M2elb+iKrHf0",
        "AAECAwQFBgcICQoL",
    ] {
        assert!(!text.contains(value));
    }
}

#[test]
fn missing_reads_do_not_create_directories_or_keys() -> Result<(), Error> {
    let f = Fixture::new()?;
    assert_eq!(f.vault()?.get("MISSING")?, None);
    assert!(f.vault()?.list()?.is_empty());
    assert!(!f.directory.exists());
    f.finish()
}

#[test]
fn default_vault_is_lazy_legacy_home_storage_not_machine_state_or_repository_configuration()
-> Result<(), Error> {
    let f = Fixture::new()?;
    let home = f.root.to_str().ok_or(VaultError::Directory)?;
    let environment = Value::object(vec![
        ("HOME", Value::string(home)),
        ("USERPROFILE", Value::string(home)),
        (
            "ROSTER_STATE_HOME",
            Value::string(env!("CARGO_MANIFEST_DIR")),
        ),
        ("GITHUB_APP_PRIVATE_KEY_PATH", Value::Number(12.0)),
    ]);
    let vault = FileVault::from_environment(&environment)?;
    assert_eq!(vault.directory, f.root.join(".roster").join("vault"));
    assert_eq!(vault.get("MISSING")?, None);
    assert!(vault.list()?.is_empty());
    assert!(!f.root.join(".roster").exists());
    assert!(FileVault::from_environment(&Value::object(Vec::new())).is_err());
    f.finish()
}

#[test]
fn independent_nist_aes256_gcm_and_native_dotnet_wire_bytes() -> Result<(), Error> {
    let zero = encrypt("", &"\0".repeat(16), &[0; 32], &[0; 12])?;
    assert_eq!(
        std::str::from_utf8(&zero).map_err(|_| VaultError::Envelope)?,
        "{\"version\":1,\"iv\":\"AAAAAAAAAAAAAAAA\",\"tag\":\"0NHIp5mZa/AmW5i11Iq5GQ==\",\"ciphertext\":\"zqdAPU1ga24HTsXTuvOdGA==\"}"
    );
    let key = (0..32).collect::<Vec<u8>>();
    let iv: [u8; 12] = (0..12)
        .collect::<Vec<u8>>()
        .try_into()
        .map_err(|_| VaultError::Envelope)?;
    assert_eq!(encrypt("TOKEN", SECRET, &key, &iv)?, WIRE.as_bytes());
    assert_eq!(decrypt("TOKEN", WIRE.as_bytes(), &key)?, SECRET);
    assert_eq!(entry_name("Token"), "546f6b656e.json");
    let f = Fixture::new()?;
    let directory =
        platform::Directory::prepare(&f.directory, true)?.ok_or(VaultError::Directory)?;
    assert!(directory.atomic(".key", &key, true)?);
    assert!(directory.atomic("544f4b454e.json", WIRE.as_bytes(), false)?);
    drop(directory);
    assert_eq!(f.vault()?.get("TOKEN")?.as_deref(), Some(SECRET));
    let directory =
        platform::Directory::prepare(&f.directory, false)?.ok_or(VaultError::Directory)?;
    let mut pending = directory.complete(WIRE.as_bytes())?;
    let temp_name = fs::read_dir(&f.directory)
        .map_err(io)?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.starts_with(".tmp-"))
        .ok_or(VaultError::Changed)?;
    let temp_path = f.directory.join(&temp_name);
    fs::remove_file(&temp_path).map_err(io)?;
    fs::write(&temp_path, b"foreign-replacement").map_err(io)?;
    assert_eq!(
        pending.publish("544f4b454e.json", false),
        Err(VaultError::Changed)
    );
    drop(pending);
    assert_eq!(fs::read(&temp_path).map_err(io)?, b"foreign-replacement");
    fs::remove_file(&temp_path).map_err(io)?;
    drop(directory);
    f.finish()
}

#[test]
fn master_buffers_and_expanded_aes_keys_require_the_zeroize_on_drop_contract() {
    fn requires_zeroize<T: zeroize::ZeroizeOnDrop>() {}
    requires_zeroize::<Zeroizing<Vec<u8>>>();
    requires_zeroize::<aes::Aes256>();
}

#[test]
fn persisted_values_sorted_names_fresh_nonces_and_no_plaintext_or_temporaries() -> Result<(), Error>
{
    let f = Fixture::new()?;
    let vault = f.vault()?;
    vault.set("Z_KEY", SECRET)?;
    vault.set("A_KEY", "second-test-secret")?;
    assert_eq!(vault.list()?, ["A_KEY", "Z_KEY"]);
    assert_eq!(vault.get("MISSING")?, None);
    assert_eq!(f.vault()?.get("Z_KEY")?.as_deref(), Some(SECRET));
    let key = fs::read(f.directory.join(".key")).map_err(io)?;
    let before = fs::read(f.directory.join(entry_name("Z_KEY"))).map_err(io)?;
    vault.set("Z_KEY", SECRET)?;
    let after = fs::read(f.directory.join(entry_name("Z_KEY"))).map_err(io)?;
    let before: serde_json::Value =
        serde_json::from_slice(&before).map_err(|_| VaultError::Envelope)?;
    let after: serde_json::Value =
        serde_json::from_slice(&after).map_err(|_| VaultError::Envelope)?;
    assert_ne!(before.get("iv"), after.get("iv"));
    assert_ne!(before.get("ciphertext"), after.get("ciphertext"));
    assert_eq!(fs::read(f.directory.join(".key")).map_err(io)?, key);
    assert_eq!(key.len(), 32);
    for entry in fs::read_dir(&f.directory).map_err(io)? {
        let entry = entry.map_err(io)?;
        assert!(!entry.file_name().to_string_lossy().starts_with(".tmp-"));
        let bytes = fs::read(entry.path()).map_err(io)?;
        assert!(
            !bytes
                .windows(SECRET.len())
                .any(|part| part == SECRET.as_bytes())
        );
        assert!(!bytes.windows(18).any(|part| part == b"second-test-secret"));
    }
    vault.set("Z_KEY", "replacement")?;
    assert_eq!(vault.get("Z_KEY")?.as_deref(), Some("replacement"));
    assert_eq!(fs::read_dir(&f.directory).map_err(io)?.count(), 3);
    f.finish()
}

#[test]
fn wrong_aad_tag_ciphertext_and_master_key_are_authentication_failures() -> Result<(), Error> {
    let key = (0..32).collect::<Vec<u8>>();
    assert_eq!(
        decrypt("OTHER", WIRE.as_bytes(), &key),
        Err(VaultError::Envelope)
    );
    assert_eq!(
        decrypt("TOKEN", WIRE.as_bytes(), &[7; 32]),
        Err(VaultError::Envelope)
    );
    for field in ["iv", "tag", "ciphertext"] {
        let mut entry: serde_json::Value =
            serde_json::from_str(WIRE).map_err(|_| VaultError::Envelope)?;
        let encoded = entry
            .get(field)
            .and_then(serde_json::Value::as_str)
            .ok_or(VaultError::Envelope)?;
        let mut bytes = base64().decode(encoded).map_err(|_| VaultError::Envelope)?;
        let first = bytes.first_mut().ok_or(VaultError::Envelope)?;
        *first ^= 1;
        entry[field] = serde_json::Value::String(base64().encode(bytes));
        assert_eq!(
            decrypt("TOKEN", entry.to_string().as_bytes(), &key),
            Err(VaultError::Envelope)
        );
    }
    let f = Fixture::new()?;
    let vault = f.vault()?;
    vault.set("TOKEN", SECRET)?;
    fs::copy(
        f.directory.join(entry_name("TOKEN")),
        f.directory.join(entry_name("COPIED")),
    )
    .map_err(io)?;
    assert_eq!(vault.get("COPIED"), Err(VaultError::Envelope));
    let key = Zeroizing::new(fs::read(f.directory.join(".key")).map_err(io)?);
    fs::write(f.directory.join(".key"), [7; 32]).map_err(io)?;
    assert_eq!(vault.get("TOKEN"), Err(VaultError::Envelope));
    fs::write(f.directory.join(".key"), &*key).map_err(io)?;
    assert_eq!(vault.get("TOKEN")?.as_deref(), Some(SECRET));
    assert_safe(VaultError::Envelope, &f);
    f.finish()
}

#[test]
fn malformed_json_envelopes_and_base64_never_return_values() -> Result<(), Error> {
    let f = Fixture::new()?;
    let vault = f.vault()?;
    vault.set("TOKEN", SECRET)?;
    for source in [
        SECRET,
        "null",
        "[]",
        "{}",
        "{\"version\":999}",
        "{\"version\":1,\"iv\":123,\"tag\":null,\"ciphertext\":[]}",
        "{\"version\":1,\"iv\":\"%%%\",\"tag\":\"%%%\",\"ciphertext\":\"%%%\"}",
        "{\"version\":1,\"iv\":\"AA==\",\"tag\":\"AA==\",\"ciphertext\":\"AA==\"}",
        "{\"version\":1,\"iv\":\"AAAAAAAAAAAAAAAA\",\"tag\":\"AAAAAAAAAAAAAAAAAAAAAA==\",\"ciphertext\":\"?\"}",
    ] {
        fs::write(f.directory.join(entry_name("TOKEN")), source).map_err(io)?;
        let error = vault.get("TOKEN").err().ok_or(VaultError::Envelope)?;
        assert_eq!(error, VaultError::Envelope);
        assert_safe(error, &f);
    }
    f.finish()
}

#[test]
fn missing_and_malformed_keys_are_never_replaced_even_for_new_names() -> Result<(), Error> {
    let f = Fixture::new()?;
    let vault = f.vault()?;
    vault.set("TOKEN", SECRET)?;
    let entry = fs::read(f.directory.join(entry_name("TOKEN"))).map_err(io)?;
    for length in [0, 1, 31, 33, 34] {
        let malformed = vec![b'x'; length];
        fs::write(f.directory.join(".key"), &malformed).map_err(io)?;
        assert_eq!(vault.get("TOKEN"), Err(VaultError::KeySize));
        assert_eq!(vault.set("OTHER", SECRET), Err(VaultError::KeySize));
        assert_eq!(fs::read(f.directory.join(".key")).map_err(io)?, malformed);
    }
    fs::remove_file(f.directory.join(".key")).map_err(io)?;
    assert_eq!(vault.get("TOKEN"), Err(VaultError::MissingKey));
    assert_eq!(vault.set("OTHER", SECRET), Err(VaultError::MissingKey));
    assert!(!f.directory.join(".key").exists());
    assert_eq!(vault.list()?, ["TOKEN"]);
    assert_eq!(
        fs::read(f.directory.join(entry_name("TOKEN"))).map_err(io)?,
        entry
    );
    f.finish()
}

#[test]
fn names_values_app_credentials_and_pem_validation_precede_filesystem_effects() -> Result<(), Error>
{
    let f = Fixture::new()?;
    let vault = f.vault()?;
    for name in [
        "",
        "../TOKEN",
        "..\\TOKEN",
        "A/B",
        "A:B",
        "A\nB",
        "1TOKEN",
        "☃",
        &"A".repeat(65),
    ] {
        assert_eq!(vault.get(name), Err(VaultError::Name));
        assert_eq!(vault.set(name, SECRET), Err(VaultError::Name));
    }
    for name in [
        "GITHUB_APP_ID",
        "GITHUB_APP_PRIVATE_KEY_PATH",
        "github_app_key",
    ] {
        assert_eq!(vault.get(name), Err(VaultError::AppCredential));
        assert_eq!(vault.set(name, SECRET), Err(VaultError::AppCredential));
    }
    assert_eq!(vault.set("TOKEN", ""), Err(VaultError::Value));
    for header in [
        "-----BEGIN RSA PRIVATE KEY-----",
        "-----begin encrypted private key-----",
        "-----BEGIN PRIVATE KEY-----",
    ] {
        assert_eq!(vault.set("TOKEN", header), Err(VaultError::PrivateKey));
    }
    assert!(!f.directory.exists());
    vault.set(&format!("_{}", "A".repeat(63)), " ")?;
    assert_eq!(vault.list()?.len(), 1);
    f.finish()
}

#[test]
fn invalid_and_forbidden_filenames_are_not_silently_ignored() -> Result<(), Error> {
    let f = Fixture::new()?;
    let vault = f.vault()?;
    vault.set("TOKEN", SECRET)?;
    for filename in [
        "not-hex.json",
        "1.json",
        "4A.json",
        "00.json",
        "31.json",
        "c3a9.json",
    ] {
        let path = f.directory.join(filename);
        fs::write(&path, "{}").map_err(io)?;
        assert_eq!(vault.list(), Err(VaultError::InvalidEntry));
        fs::remove_file(path).map_err(io)?;
    }
    let path = f.directory.join(entry_name("GITHUB_APP_ID"));
    fs::write(&path, "{}").map_err(io)?;
    assert_eq!(vault.list(), Err(VaultError::ForbiddenEntry));
    fs::remove_file(path).map_err(io)?;
    fs::create_dir(f.directory.join(entry_name("DIRECTORY"))).map_err(io)?;
    assert_eq!(vault.list(), Err(VaultError::InvalidEntry));
    f.finish()
}

#[test]
fn git_markers_are_refused_before_creation_or_hardening() -> Result<(), Error> {
    let f = Fixture::new()?;
    for kind in ["repository", "worktree"] {
        let repository = f.root.join(kind);
        fs::create_dir(&repository).map_err(io)?;
        if kind == "repository" {
            fs::create_dir(repository.join(".git")).map_err(io)?;
        } else {
            fs::write(repository.join(".git"), "gitdir: ../metadata").map_err(io)?;
        }
        let directory = repository.join(".roster").join("vault");
        assert_eq!(
            FileVault::new(&directory)?.set("TOKEN", SECRET),
            Err(VaultError::Containment)
        );
        assert!(!repository.join(".roster").exists());
        assert_eq!(
            FileVault::new(&repository)?.get("MISSING"),
            Err(VaultError::Containment)
        );
    }
    let parent = f.root.join("not-a-directory");
    fs::write(&parent, SECRET).map_err(io)?;
    let error = FileVault::new(parent.join("vault"))?
        .set("TOKEN", SECRET)
        .err()
        .ok_or(VaultError::Io)?;
    assert_safe(error, &f);
    assert_eq!(fs::read_to_string(parent).map_err(io)?, SECRET);
    f.finish()
}

#[test]
fn hardlinked_entries_keys_and_existing_write_targets_are_refused_without_mutation()
-> Result<(), Error> {
    let f = Fixture::new()?;
    let vault = f.vault()?;
    vault.set("TOKEN", SECRET)?;
    let target = f.root.join("outside-entry");
    fs::write(&target, "unchanged").map_err(io)?;
    fs::hard_link(&target, f.directory.join(entry_name("ALIAS"))).map_err(io)?;
    assert_eq!(vault.get("ALIAS"), Err(VaultError::Linked));
    assert_eq!(vault.set("ALIAS", SECRET), Err(VaultError::Linked));
    assert_eq!(vault.list(), Err(VaultError::InvalidEntry));
    assert_eq!(fs::read_to_string(&target).map_err(io)?, "unchanged");
    fs::remove_file(f.directory.join(entry_name("ALIAS"))).map_err(io)?;
    fs::hard_link(f.directory.join(".key"), f.root.join("key-alias")).map_err(io)?;
    assert_eq!(vault.get("TOKEN"), Err(VaultError::Linked));
    assert_eq!(vault.set("OTHER", SECRET), Err(VaultError::Linked));
    f.finish()
}

#[test]
fn exact_secret_and_envelope_limits_are_enforced_without_partial_publication() -> Result<(), Error>
{
    let f = Fixture::new()?;
    let vault = f.vault()?;
    assert_eq!(
        vault.set("TOKEN", &"x".repeat(MAX_SECRET_BYTES + 1)),
        Err(VaultError::Limit)
    );
    assert!(!f.directory.exists());
    let value = "x".repeat(MAX_SECRET_BYTES);
    vault.set("TOKEN", &value)?;
    assert_eq!(vault.get("TOKEN")?.as_deref(), Some(value.as_str()));
    let original = fs::read(f.directory.join(entry_name("TOKEN"))).map_err(io)?;
    assert!(original.len() < MAX_ENVELOPE_BYTES);
    let mut padded = original.clone();
    padded.resize(MAX_ENVELOPE_BYTES, b' ');
    fs::write(f.directory.join(entry_name("TOKEN")), &padded).map_err(io)?;
    assert_eq!(vault.get("TOKEN")?.as_deref(), Some(value.as_str()));
    padded.push(b' ');
    fs::write(f.directory.join(entry_name("TOKEN")), &padded).map_err(io)?;
    assert_eq!(vault.get("TOKEN"), Err(VaultError::Limit));
    assert_eq!(
        vault.set("OTHER", &"☃".repeat(MAX_SECRET_BYTES / 3 + 1)),
        Err(VaultError::Limit)
    );
    let key = Zeroizing::new(fs::read(f.directory.join(".key")).map_err(io)?);
    let oversized = encrypt("TOKEN", &"x".repeat(MAX_SECRET_BYTES + 1), &key, &[0; 12])?;
    assert!(oversized.len() <= MAX_ENVELOPE_BYTES);
    fs::write(f.directory.join(entry_name("TOKEN")), oversized).map_err(io)?;
    assert_eq!(vault.get("TOKEN"), Err(VaultError::Limit));
    f.finish()
}

#[test]
fn exact_directory_entry_and_path_bounds_do_not_create_unbounded_state() -> Result<(), Error> {
    let f = Fixture::new()?;
    let vault = f.vault()?;
    vault.set("TOKEN", SECRET)?;
    for index in 0..MAX_DIRECTORY_ENTRIES - 2 {
        fs::write(f.directory.join(format!("ignored-{index}")), []).map_err(io)?;
    }
    assert_eq!(vault.list()?, ["TOKEN"]);
    fs::write(f.directory.join("one-too-many"), []).map_err(io)?;
    assert_eq!(vault.list(), Err(VaultError::Limit));
    let base = f.root.to_str().ok_or(VaultError::Directory)?;
    let remaining = MAX_PATH_UNITS
        .checked_sub(base.encode_utf16().count() + 1)
        .ok_or(VaultError::Directory)?;
    let bounded = f.root.join("a".repeat(remaining));
    assert!(FileVault::new(&bounded).is_ok());
    assert!(
        FileVault::new(f.root.join("a".repeat(remaining + 1)))
            .is_err_and(|error| error == VaultError::Directory)
    );
    let mut bounded = f.root.clone();
    for _ in bounded.components().count()..MAX_PATH_COMPONENTS {
        bounded.push("a");
    }
    assert!(FileVault::new(&bounded).is_ok());
    bounded.push("a");
    assert!(FileVault::new(&bounded).is_err_and(|error| error == VaultError::Directory));
    assert!(FileVault::new("").is_err_and(|error| error == VaultError::Directory));
    assert!(FileVault::new(" \t").is_err_and(|error| error == VaultError::Directory));
    for blank in ["\u{feff}", "\u{a0}\u{feff}\t", "\u{2028}\u{2029}"] {
        assert!(FileVault::new(blank).is_err_and(|error| error == VaultError::Directory));
    }
    f.finish()
}

#[test]
fn legitimate_version_one_envelopes_allow_numeric_one_extra_fields_and_unpadded_base64()
-> Result<(), Error> {
    let mut entry: serde_json::Value =
        serde_json::from_str(WIRE).map_err(|_| VaultError::Envelope)?;
    entry["version"] = serde_json::Value::from(1.0);
    entry["legacy-extra"] = serde_json::Value::Bool(true);
    for field in ["iv", "tag", "ciphertext"] {
        let value = entry
            .get(field)
            .and_then(serde_json::Value::as_str)
            .ok_or(VaultError::Envelope)?;
        entry[field] = serde_json::Value::String(value.trim_end_matches('=').to_owned());
    }
    assert_eq!(
        decrypt(
            "TOKEN",
            entry.to_string().as_bytes(),
            &(0..32).collect::<Vec<u8>>()
        )?,
        SECRET
    );
    Ok(())
}

#[test]
fn failed_atomic_replacement_cleans_owned_temporary_and_preserves_original() -> Result<(), Error> {
    let f = Fixture::new()?;
    let directory =
        platform::Directory::prepare(&f.directory, true)?.ok_or(VaultError::Directory)?;
    directory.atomic(".key", &[9; 32], true)?;
    let mut pending = directory.complete(b"complete-new-bytes")?;
    fs::create_dir(f.directory.join(entry_name("TOKEN"))).map_err(io)?;
    assert_eq!(
        pending.publish(&entry_name("TOKEN"), false),
        Err(VaultError::Linked)
    );
    drop(pending);
    assert!(
        !directory
            .entries()?
            .iter()
            .any(|name| name.starts_with(".tmp-"))
    );
    assert_eq!(
        directory
            .read(".key", 33)?
            .as_deref()
            .map(|bytes| bytes.as_slice()),
        Some([9; 32].as_slice())
    );
    drop(directory);
    f.finish()
}

#[cfg(windows)]
#[test]
fn native_atomic_replace_failure_preserves_complete_old_bytes_and_removes_owned_temporary()
-> Result<(), Error> {
    let f = Fixture::new()?;
    f.vault()?.set("TOKEN", SECRET)?;
    let directory =
        platform::Directory::prepare(&f.directory, false)?.ok_or(VaultError::Directory)?;
    let before = directory
        .read(&entry_name("TOKEN"), MAX_ENVELOPE_BYTES)?
        .ok_or(VaultError::Io)?;
    let held = directory
        .inspect(&entry_name("TOKEN"))?
        .ok_or(VaultError::Io)?;
    let mut pending = directory.complete(b"complete-but-unpublished")?;
    let error = pending
        .publish(&entry_name("TOKEN"), false)
        .err()
        .ok_or(VaultError::Io)?;
    assert_eq!(error, VaultError::Io);
    assert_safe(error, &f);
    drop(pending);
    drop(held);
    assert!(
        !directory
            .entries()?
            .iter()
            .any(|name| name.starts_with(".tmp-"))
    );
    assert_eq!(
        &*directory
            .read(&entry_name("TOKEN"), MAX_ENVELOPE_BYTES)?
            .ok_or(VaultError::Io)?,
        &*before
    );
    drop(directory);
    assert_eq!(f.vault()?.get("TOKEN")?.as_deref(), Some(SECRET));
    f.finish()
}

#[test]
fn selected_resolver_uses_real_vault_and_preserves_read_absence() -> Result<(), Error> {
    let f = Fixture::new()?;
    let vault = f.vault()?;
    let get = &mut |name: &str| {
        vault
            .get(name)
            .map(|value| value.map(|value| Value::string(value.as_str())))
            .map_err(Error::from)
    };
    assert_eq!(
        crate::secrets::resolve_secret(&Text::from("MISSING"), &Value::object(Vec::new()), get)?,
        None
    );
    assert!(!f.directory.exists());
    vault.set("TOKEN", "vault-value")?;
    for environment in [
        Value::object(Vec::new()),
        Value::object(vec![("TOKEN", Value::string(""))]),
    ] {
        assert_eq!(
            crate::secrets::resolve_secret(&Text::from("TOKEN"), &environment, get)?,
            Some(Text::from("vault-value"))
        );
    }
    fs::remove_file(f.directory.join(".key")).map_err(io)?;
    let failure =
        crate::secrets::resolve_secret(&Text::from("TOKEN"), &Value::object(Vec::new()), get);
    assert!(failure.is_err_and(
        |error| error.code == Some("ROSTER_SECRET_RESOLUTION") && error.cause.is_none()
    ));
    f.finish()
}

#[test]
fn native_process_worker() -> Result<(), Error> {
    let Some(directory) = std::env::var_os("ROSTER_VAULT_FIXTURE_WORKER") else {
        return Ok(());
    };
    let operation =
        std::env::var("ROSTER_VAULT_FIXTURE_OPERATION").map_err(|_| VaultError::Directory)?;
    let path = PathBuf::from(directory);
    if matches!(operation.as_str(), "set" | "read") {
        let name = std::env::var("ROSTER_VAULT_FIXTURE_NAME").map_err(|_| VaultError::Name)?;
        let ready = path
            .parent()
            .ok_or(VaultError::Directory)?
            .join(format!("ready-{}", hexadecimal(name.as_bytes())));
        fs::write(&ready, "ready").map_err(io)?;
        let release = path.parent().ok_or(VaultError::Directory)?.join("release");
        let started = Instant::now();
        while !release.exists() {
            if started.elapsed() > Duration::from_secs(10) {
                return Err(VaultError::Io.into());
            }
            thread::sleep(Duration::from_millis(2));
        }
        if operation == "set" {
            FileVault::new(path)?.set(&name, &format!("value-{name}"))?;
        } else {
            let vault = FileVault::new(&path)?;
            let mut seen_key = false;
            loop {
                if started.elapsed() > Duration::from_secs(10) {
                    return Err(VaultError::Io.into());
                }
                if let Some(directory) = platform::Directory::prepare(&path, false)? {
                    if let Some(key) = directory.read(".key", 33)? {
                        if key.len() != 32 {
                            return Err(VaultError::KeySize.into());
                        }
                        seen_key = true;
                    } else if seen_key {
                        return Err(VaultError::MissingKey.into());
                    }
                }
                let mut complete = true;
                for name in ["TOKEN", "Token", "SECOND", "THIRD"] {
                    if let Some(value) = vault.get(name)? {
                        assert_eq!(value, format!("value-{name}"));
                    } else {
                        complete = false;
                    }
                }
                if complete {
                    break;
                }
                thread::sleep(Duration::from_millis(2));
            }
        }
    } else {
        let directory = platform::Directory::prepare(&path, true)?.ok_or(VaultError::Directory)?;
        let bytes = vec![17; if operation == "crash-partial" { 16 } else { 32 }];
        let mut pending = directory.complete(&bytes)?;
        if operation == "crash-after" {
            assert!(pending.publish(".key", true)?);
        }
        std::process::exit(73);
    }
    Ok(())
}

struct Worker(std::process::Child);

impl Worker {
    fn wait(&mut self) -> Result<std::process::ExitStatus, std::io::Error> {
        let started = Instant::now();
        loop {
            if let Some(status) = self.0.try_wait()? {
                return Ok(status);
            }
            if started.elapsed() >= Duration::from_secs(10) {
                self.0.kill()?;
                self.0.wait()?;
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Owned vault worker did not exit",
                ));
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn kill(&mut self) -> Result<(), std::io::Error> {
        self.0.kill()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            if self.0.wait().is_err() {
                eprintln!("Owned vault worker cleanup is unverified");
            }
        }
    }
}

fn worker(f: &Fixture, operation: &str, name: &str) -> Result<Worker, Error> {
    Command::new(std::env::current_exe().map_err(io)?)
        .args([
            "--exact",
            "vault::tests::native_process_worker",
            "--nocapture",
        ])
        .env("ROSTER_VAULT_FIXTURE_WORKER", &f.directory)
        .env("ROSTER_VAULT_FIXTURE_OPERATION", operation)
        .env("ROSTER_VAULT_FIXTURE_NAME", name)
        .spawn()
        .map(Worker)
        .map_err(io)
}

#[test]
fn real_multiprocess_first_writes_share_one_complete_single_link_key() -> Result<(), Error> {
    let f = Fixture::new()?;
    let names = ["TOKEN", "Token", "SECOND", "THIRD"];
    let mut children = Vec::new();
    for name in names {
        children.push(worker(&f, "set", name)?);
    }
    for name in ["READER_0", "READER_1"] {
        children.push(worker(&f, "read", name)?);
    }
    let participants = ["TOKEN", "Token", "SECOND", "THIRD", "READER_0", "READER_1"];
    let started = Instant::now();
    while !participants.iter().all(|name| {
        f.root
            .join(format!("ready-{}", hexadecimal(name.as_bytes())))
            .exists()
    }) {
        if started.elapsed() > Duration::from_secs(10) {
            for child in &mut children {
                let _ = child.kill();
                let _ = child.wait();
            }
            return Err(VaultError::Io.into());
        }
        thread::sleep(Duration::from_millis(2));
    }
    fs::write(f.root.join("release"), "release").map_err(io)?;
    for child in &mut children {
        assert!(child.wait().map_err(io)?.success());
    }
    let vault = f.vault()?;
    assert_eq!(vault.list()?, ["SECOND", "THIRD", "TOKEN", "Token"]);
    for name in names {
        assert_eq!(vault.get(name)?, Some(format!("value-{name}")));
    }
    assert_eq!(fs::read(f.directory.join(".key")).map_err(io)?.len(), 32);
    assert_eq!(paths::file_information(&f.directory.join(".key"))?.links, 1);
    assert_eq!(
        fs::read_dir(&f.directory).map_err(io)?.count(),
        names.len() + 1
    );
    f.finish()
}

#[test]
fn crash_before_and_after_publication_never_exposes_partial_or_multilink_keys() -> Result<(), Error>
{
    for operation in ["crash-partial", "crash-before", "crash-after"] {
        let f = Fixture::new()?;
        assert_eq!(
            worker(&f, operation, "")?.wait().map_err(io)?.code(),
            Some(73)
        );
        let vault = f.vault()?;
        if operation != "crash-after" {
            assert!(!f.directory.join(".key").exists());
            assert_eq!(fs::read_dir(&f.directory).map_err(io)?.count(), 1);
            let entry = fs::read_dir(&f.directory)
                .map_err(io)?
                .next()
                .ok_or(VaultError::Io)?
                .map_err(io)?;
            assert_eq!(
                entry.metadata().map_err(io)?.len(),
                if operation == "crash-partial" { 16 } else { 32 }
            );
        } else {
            assert_eq!(fs::read(f.directory.join(".key")).map_err(io)?, [17; 32]);
            assert_eq!(fs::read_dir(&f.directory).map_err(io)?.count(), 1);
        }
        vault.set("TOKEN", SECRET)?;
        assert_eq!(vault.get("TOKEN")?.as_deref(), Some(SECRET));
        assert_eq!(paths::file_information(&f.directory.join(".key"))?.links, 1);
        if operation == "crash-after" {
            assert_eq!(fs::read(f.directory.join(".key")).map_err(io)?, [17; 32]);
        }
        f.finish()?;
    }
    Ok(())
}

#[cfg(windows)]
#[test]
fn unavailable_native_volume_is_an_error_not_a_missing_vault_or_optional_auth() -> Result<(), Error>
{
    let vault = FileVault::new(r"\\?\Volume{00000000-0000-0000-0000-000000000000}\vault")?;
    assert!(vault.get("MISSING").is_err());
    assert!(vault.list().is_err());
    assert!(vault.set("TOKEN", SECRET).is_err());
    let failure = crate::secrets::resolve_secret(
        &Text::from("TOKEN"),
        &Value::object(Vec::new()),
        &mut |name| {
            vault
                .get(name)
                .map(|value| value.map(|value| Value::string(value.as_str())))
                .map_err(Error::from)
        },
    );
    assert!(failure.is_err_and(
        |error| error.code == Some("ROSTER_SECRET_RESOLUTION") && error.cause.is_none()
    ));
    Ok(())
}

#[cfg(windows)]
#[test]
fn actual_windows_owner_only_acls_have_independent_native_external_audit() -> Result<(), Error> {
    let f = Fixture::new()?;
    f.vault()?.set("TOKEN", SECRET)?;
    let system = std::env::var_os("SystemRoot").ok_or(VaultError::Unsupported)?;
    let observer = PathBuf::from(system)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    let script = "$ErrorActionPreference='Stop'; $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value; $paths=@($env:ROSTER_TEST_VAULT)+[System.IO.Directory]::GetFileSystemEntries($env:ROSTER_TEST_VAULT); foreach($path in $paths) { if([System.IO.Directory]::Exists($path)) { $acl=[System.IO.Directory]::GetAccessControl($path) } else { $acl=[System.IO.File]::GetAccessControl($path) }; if($acl.GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $sid -or !$acl.AreAccessRulesProtected) { throw 'Wrong vault owner or inheritance' }; $rules=$acl.GetAccessRules($true,$true,[System.Security.Principal.SecurityIdentifier]); if($rules.Count -ne 1 -or $rules[0].IdentityReference.Value -ne $sid -or $rules[0].FileSystemRights -ne 'FullControl' -or $rules[0].AccessControlType -ne 'Allow') { throw 'Vault ACL is not owner-only' } }; Write-Output ('owner-only-access:'+$paths.Count)";
    let output = Command::new(observer)
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("ROSTER_TEST_VAULT", &f.directory)
        .env("USERNAME", "not-the-principal")
        .output()
        .map_err(io)?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "owner-only-access:3"
    );
    f.finish()
}

#[cfg(windows)]
fn windows_fixture(fixture: &Fixture, script: &str) -> Result<String, Error> {
    let system = std::env::var_os("SystemRoot").ok_or(VaultError::Unsupported)?;
    let observer = PathBuf::from(system)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    let output = Command::new(observer)
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env_remove("PSModulePath")
        .env("ROSTER_TEST_VAULT", &fixture.directory)
        .env("ROSTER_TEST_ROOT", &fixture.root)
        .output()
        .map_err(io)?;
    if !output.status.success() {
        return Err(Error::new(
            "FixtureObserver",
            String::from_utf8_lossy(&output.stderr),
        ));
    }
    String::from_utf8(output.stdout).map_err(|_| VaultError::Envelope.into())
}

#[cfg(windows)]
#[test]
fn windows_owned_legacy_wide_and_null_dacls_are_hardened_before_secret_reads() -> Result<(), Error>
{
    let f = Fixture::new()?;
    let vault = f.vault()?;
    vault.set("TOKEN", SECRET)?;
    windows_fixture(
        &f,
        "$ErrorActionPreference='Stop'; $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User; $world=[System.Security.Principal.SecurityIdentifier]::new('S-1-1-0'); $paths=@($env:ROSTER_TEST_VAULT)+[System.IO.Directory]::GetFileSystemEntries($env:ROSTER_TEST_VAULT); foreach($path in $paths) { if([System.IO.Directory]::Exists($path)) { $acl=[System.IO.Directory]::GetAccessControl($path) } else { $acl=[System.IO.File]::GetAccessControl($path) }; $acl.SetAccessRuleProtection($true,$false); $acl.AddAccessRule([System.Security.AccessControl.FileSystemAccessRule]::new($world,'FullControl','Allow')); if([System.IO.Directory]::Exists($path)) { [System.IO.Directory]::SetAccessControl($path,$acl) } else { [System.IO.File]::SetAccessControl($path,$acl) } }; Write-Output 'wide-owned-fixture'",
    )?;
    assert_eq!(vault.get("TOKEN")?.as_deref(), Some(SECRET));
    let audit = "$ErrorActionPreference='Stop'; $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value; $paths=@($env:ROSTER_TEST_VAULT)+[System.IO.Directory]::GetFileSystemEntries($env:ROSTER_TEST_VAULT); foreach($path in $paths) { if([System.IO.Directory]::Exists($path)) { $acl=[System.IO.Directory]::GetAccessControl($path) } else { $acl=[System.IO.File]::GetAccessControl($path) }; $rules=$acl.GetAccessRules($true,$true,[System.Security.Principal.SecurityIdentifier]); if(!$acl.AreAccessRulesProtected -or $acl.GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $sid -or $rules.Count -ne 1 -or $rules[0].IdentityReference.Value -ne $sid -or $rules[0].FileSystemRights -ne 'FullControl' -or $rules[0].AccessControlType -ne 'Allow') { throw 'Not owner-only' } }; Write-Output 'audited-owner-only'";
    assert_eq!(windows_fixture(&f, audit)?.trim(), "audited-owner-only");
    windows_fixture(
        &f,
        "$ErrorActionPreference='Stop'; $path=[System.IO.Path]::Combine($env:ROSTER_TEST_VAULT,'544f4b454e.json'); $acl=[System.IO.File]::GetAccessControl($path); $acl.SetSecurityDescriptorSddlForm('D:NO_ACCESS_CONTROL',[System.Security.AccessControl.AccessControlSections]::Access); [System.IO.File]::SetAccessControl($path,$acl); Write-Output 'null-dacl-owned-fixture'",
    )?;
    assert_eq!(vault.get("TOKEN")?.as_deref(), Some(SECRET));
    assert_eq!(windows_fixture(&f, audit)?.trim(), "audited-owner-only");
    f.finish()
}

#[cfg(windows)]
#[test]
fn windows_denied_file_access_is_not_absence_or_a_replaced_entry() -> Result<(), Error> {
    let f = Fixture::new()?;
    let vault = f.vault()?;
    vault.set("TOKEN", SECRET)?;
    windows_fixture(
        &f,
        "$ErrorActionPreference='Stop'; $path=[System.IO.Path]::Combine($env:ROSTER_TEST_VAULT,'544f4b454e.json'); $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User; $acl=[System.IO.File]::GetAccessControl($path); $acl.AddAccessRule([System.Security.AccessControl.FileSystemAccessRule]::new($sid,'ReadData','Deny')); [System.IO.File]::SetAccessControl($path,$acl); Write-Output 'read-denied-owned-fixture'",
    )?;
    assert!(vault.get("TOKEN").is_err());
    assert!(vault.set("TOKEN", "must not replace").is_err());
    assert!(vault.list().is_err());
    windows_fixture(
        &f,
        "$ErrorActionPreference='Stop'; $path=[System.IO.Path]::Combine($env:ROSTER_TEST_VAULT,'544f4b454e.json'); $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User; $acl=[System.IO.File]::GetAccessControl($path); $acl.RemoveAccessRuleSpecific([System.Security.AccessControl.FileSystemAccessRule]::new($sid,'ReadData','Deny')); [System.IO.File]::SetAccessControl($path,$acl); Write-Output 'restored-owned-fixture'",
    )?;
    assert_eq!(vault.get("TOKEN")?.as_deref(), Some(SECRET));
    f.finish()
}

#[cfg(windows)]
#[test]
fn windows_real_junction_vault_and_parent_paths_cannot_be_adopted() -> Result<(), Error> {
    let f = Fixture::new()?;
    fs::create_dir(f.root.join("target")).map_err(io)?;
    fs::create_dir(f.root.join("target").join(".git")).map_err(io)?;
    windows_fixture(
        &f,
        "$ErrorActionPreference='Stop'; $target=[System.IO.Path]::Combine($env:ROSTER_TEST_ROOT,'target'); New-Item -ItemType Junction -Path $env:ROSTER_TEST_VAULT -Target $target | Out-Null; Write-Output 'owned-junction-created'",
    )?;
    assert_eq!(f.vault()?.get("TOKEN"), Err(VaultError::Linked));
    assert_eq!(f.vault()?.set("TOKEN", SECRET), Err(VaultError::Linked));
    assert_eq!(f.vault()?.list(), Err(VaultError::Linked));
    let nested = FileVault::new(f.directory.join("nested"))?;
    assert_eq!(nested.set("TOKEN", SECRET), Err(VaultError::Containment));
    assert!(!f.root.join("target").join("nested").exists());
    assert_eq!(fs::read_dir(f.root.join("target")).map_err(io)?.count(), 1);
    fs::remove_dir(&f.directory).map_err(io)?;
    f.finish()
}

#[cfg(windows)]
#[test]
fn concurrent_native_reads_secure_owned_legacy_and_recreated_directory_identities_before_returning()
-> Result<(), Error> {
    let f = Fixture::new()?;
    fs::create_dir(&f.directory).map_err(io)?;
    for _ in 0..2 {
        let mut readers = Vec::new();
        for _ in 0..8 {
            let path = f.directory.clone();
            readers.push(thread::spawn(move || FileVault::new(path)?.get("MISSING")));
        }
        for reader in readers {
            assert_eq!(reader.join().map_err(|_| VaultError::Io)??, None);
        }
        assert!(f.vault()?.list()?.is_empty());
        let audit = "$ErrorActionPreference='Stop'; $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value; $acl=[System.IO.Directory]::GetAccessControl($env:ROSTER_TEST_VAULT); $rules=$acl.GetAccessRules($true,$true,[System.Security.Principal.SecurityIdentifier]); if(!$acl.AreAccessRulesProtected -or $rules.Count -ne 1 -or $rules[0].IdentityReference.Value -ne $sid -or $rules[0].FileSystemRights -ne 'FullControl') { throw 'Concurrent read preceded hardening' }; Write-Output 'native-owner-only'";
        assert_eq!(windows_fixture(&f, audit)?.trim(), "native-owner-only");
        fs::remove_dir(&f.directory).map_err(io)?;
        fs::create_dir(&f.directory).map_err(io)?;
    }
    f.finish()
}

#[cfg(windows)]
#[test]
fn concurrent_native_permission_failures_are_not_cached_as_missing_or_success_and_can_be_repaired()
-> Result<(), Error> {
    let f = Fixture::new()?;
    fs::create_dir(&f.directory).map_err(io)?;
    assert_eq!(f.vault()?.get("MISSING")?, None);
    windows_fixture(
        &f,
        "$ErrorActionPreference='Stop'; $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User; $acl=[System.IO.Directory]::GetAccessControl($env:ROSTER_TEST_VAULT); $acl.AddAccessRule([System.Security.AccessControl.FileSystemAccessRule]::new($sid,'ListDirectory','Deny')); [System.IO.Directory]::SetAccessControl($env:ROSTER_TEST_VAULT,$acl); Write-Output 'read-denied-owned-directory'",
    )?;
    let mut readers = Vec::new();
    for _ in 0..8 {
        let path = f.directory.clone();
        readers.push(thread::spawn(move || FileVault::new(path)?.get("MISSING")));
    }
    for reader in readers {
        assert!(reader.join().map_err(|_| VaultError::Io)?.is_err());
    }
    windows_fixture(
        &f,
        "$ErrorActionPreference='Stop'; $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User; $acl=[System.IO.Directory]::GetAccessControl($env:ROSTER_TEST_VAULT); $acl.RemoveAccessRuleSpecific([System.Security.AccessControl.FileSystemAccessRule]::new($sid,'ListDirectory','Deny')); [System.IO.Directory]::SetAccessControl($env:ROSTER_TEST_VAULT,$acl); Write-Output 'restored-owned-directory'",
    )?;
    assert_eq!(f.vault()?.get("MISSING")?, None);
    assert!(f.vault()?.list()?.is_empty());
    assert!(!f.directory.join(".key").exists());
    f.finish()
}

#[cfg(windows)]
#[test]
fn windows_pinned_directory_and_ancestors_deny_path_swap_adoption() -> Result<(), Error> {
    let f = Fixture::new()?;
    f.vault()?.set("TOKEN", SECRET)?;
    let directory =
        platform::Directory::prepare(&f.directory, false)?.ok_or(VaultError::Directory)?;
    assert!(fs::rename(&f.directory, f.root.join("moved")).is_err());
    assert!(fs::rename(&f.root, f.root.with_extension("moved")).is_err());
    assert!(
        directory
            .read(&entry_name("TOKEN"), MAX_ENVELOPE_BYTES)?
            .is_some()
    );
    drop(directory);
    f.finish()
}

#[cfg(unix)]
#[test]
fn unix_real_file_and_directory_symlinks_are_neither_read_nor_overwritten() -> Result<(), Error> {
    use std::os::unix::fs::symlink;
    let f = Fixture::new()?;
    let vault = f.vault()?;
    vault.set("TOKEN", SECRET)?;
    let target = f.root.join("outside-entry");
    fs::write(&target, "unchanged").map_err(io)?;
    let alias = f.directory.join(entry_name("ALIAS"));
    symlink(&target, &alias).map_err(io)?;
    assert_eq!(vault.get("ALIAS"), Err(VaultError::Linked));
    assert_eq!(vault.set("ALIAS", SECRET), Err(VaultError::Linked));
    assert_eq!(vault.list(), Err(VaultError::InvalidEntry));
    assert_eq!(fs::read_to_string(&target).map_err(io)?, "unchanged");
    fs::remove_file(&alias).map_err(io)?;
    fs::rename(&f.directory, f.root.join("original")).map_err(io)?;
    symlink(f.root.join("original"), &f.directory).map_err(io)?;
    assert_eq!(vault.get("TOKEN"), Err(VaultError::Linked));
    assert_eq!(vault.set("TOKEN", SECRET), Err(VaultError::Linked));
    assert_eq!(vault.list(), Err(VaultError::Linked));
    fs::remove_file(&f.directory).map_err(io)?;
    f.finish()
}

#[cfg(unix)]
#[test]
fn unix_actual_owner_modes_legacy_hardening_and_path_swap_refusal() -> Result<(), Error> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let f = Fixture::new()?;
    let vault = f.vault()?;
    vault.set("TOKEN", SECRET)?;
    assert_eq!(
        fs::metadata(&f.directory).map_err(io)?.mode() & 0o7777,
        0o700
    );
    for entry in fs::read_dir(&f.directory).map_err(io)? {
        let path = entry.map_err(io)?.path();
        let metadata = fs::metadata(&path).map_err(io)?;
        assert_eq!(metadata.mode() & 0o7777, 0o600);
        assert_eq!(metadata.uid(), rustix::process::geteuid().as_raw());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).map_err(io)?;
    }
    fs::set_permissions(&f.directory, fs::Permissions::from_mode(0o777)).map_err(io)?;
    assert_eq!(vault.get("TOKEN")?.as_deref(), Some(SECRET));
    assert_eq!(
        fs::metadata(&f.directory).map_err(io)?.mode() & 0o7777,
        0o700
    );
    let directory =
        platform::Directory::prepare(&f.directory, false)?.ok_or(VaultError::Directory)?;
    fs::rename(&f.directory, f.root.join("moved")).map_err(io)?;
    fs::create_dir(&f.directory).map_err(io)?;
    assert!(
        directory
            .read(&entry_name("TOKEN"), MAX_ENVELOPE_BYTES)
            .is_err_and(|error| error == VaultError::Changed)
    );
    drop(directory);
    f.finish()
}
