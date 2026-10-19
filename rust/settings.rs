use std::collections::HashSet;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::Error;
use crate::cancel::CancellationToken;
use crate::clock;
use crate::paths;
use crate::process::{self, FailureKind, Limits};
use crate::state::files;
use crate::text::Text;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Name {
    Config,
    Fleet,
    Capabilities,
}

impl Name {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config.yml",
            Self::Fleet => "fleet.yml",
            Self::Capabilities => "capabilities.yml",
        }
    }

    pub fn parse(name: &str) -> Result<Self, Error> {
        match name {
            "config.yml" => Ok(Self::Config),
            "fleet.yml" => Ok(Self::Fleet),
            "capabilities.yml" => Ok(Self::Capabilities),
            _ => Err(Error::type_error("Unsupported private settings filename")),
        }
    }
}

fn invalid(message: &str) -> Error {
    Error::new("Error", format!("Invalid roster config: {message}"))
}

fn file_path(root: &Path, name: Name) -> PathBuf {
    root.join(".roster").join(name.as_str())
}

pub fn read_config_file(file: &Path) -> Result<Text, Error> {
    let parent = file
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let directory =
        fs::symlink_metadata(parent).map_err(|error| io_error("Read config directory", error))?;
    if !directory.is_dir() || directory.file_type().is_symlink() {
        return Err(invalid("expected a non-symlink config directory"));
    }
    let entry = fs::symlink_metadata(file).map_err(|error| io_error("Read config file", error))?;
    if !entry.is_file() || entry.file_type().is_symlink() || entry.len() > 65_536 {
        return Err(invalid(
            "expected a regular, non-symlink file of at most 64 KiB",
        ));
    }
    let mut bytes = Vec::new();
    fs::File::open(file)
        .and_then(|file| file.take(65_537).read_to_end(&mut bytes))
        .map_err(|error| io_error("Read config file", error))?;
    if bytes.len() > 65_536 {
        return Err(invalid(
            "expected a regular, non-symlink file of at most 64 KiB",
        ));
    }
    let source = String::from_utf8(bytes).map_err(|_| invalid("config file must be UTF-8"))?;
    Ok(Text::from(source.as_str()))
}

fn io_error(operation: &str, error: io::Error) -> Error {
    let code = match error.kind() {
        io::ErrorKind::NotFound => "ENOENT",
        io::ErrorKind::AlreadyExists => "EEXIST",
        io::ErrorKind::PermissionDenied => "EACCES",
        _ => "EIO",
    };
    Error::new("Error", format!("{operation} failed: {error}")).coded(code)
}

pub fn read_private_file(root: &Path, name: Name) -> Result<Option<Text>, Error> {
    let file = file_path(root, name);
    paths::ensure_local_path(&file, root)?;
    match read_config_file(&file) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.code == Some("ENOENT") => Ok(None),
        Err(error) => Err(error),
    }
}

async fn git(
    root: &Path,
    args: &[String],
    token: Option<&CancellationToken>,
) -> Result<process::Output, process::Failure> {
    let mut command = tokio::process::Command::new("git");
    command.arg("--no-pager").args(args).current_dir(root);
    process::execute(
        &mut command,
        Limits {
            timeout: Some(Duration::from_secs(10)),
            max_output_bytes: 1024 * 1024,
        },
        token,
    )
    .await
}

fn process_error(failure: process::Failure) -> Error {
    let code = failure.code();
    let mut error = Error::new("Error", failure.to_string());
    if let Some(code) = code {
        error = error.coded(code);
    }
    error
}

pub async fn ensure_private_files_ignored(
    root: &Path,
    names: &[Name],
    token: Option<&CancellationToken>,
) -> Result<(), Error> {
    let ignore_file = root.join(".gitignore");
    paths::ensure_local_path(&ignore_file, root)?;
    let source = match fs::symlink_metadata(&ignore_file) {
        Ok(entry) => {
            if !entry.is_file()
                || entry.file_type().is_symlink()
                || paths::file_information(&ignore_file)?.links != 1
            {
                return Err(Error::new(
                    "Error",
                    ".gitignore must be a regular single-link file",
                ));
            }
            Text::from(
                String::from_utf8_lossy(
                    &fs::read(&ignore_file)
                        .map_err(|error| io_error("Read ignore rules", error))?,
                )
                .as_ref(),
            )
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Text::default(),
        Err(error) => return Err(io_error("Inspect ignore rules", error)),
    };
    let patterns = names
        .iter()
        .flat_map(|name| {
            [
                format!(".roster/{}", name.as_str()),
                format!(".roster/{}.*", name.as_str()),
            ]
        })
        .collect::<Vec<_>>();
    let marker = match fs::symlink_metadata(root.join(".git")) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(io_error("Inspect Git marker", error)),
    };
    let ignored = if marker {
        let mut args = vec!["ls-files".to_owned(), "--".to_owned()];
        args.extend(patterns.iter().cloned());
        let tracked = git(root, &args, token).await.map_err(process_error)?;
        if !String::from_utf8_lossy(&tracked.captured.stdout)
            .trim()
            .is_empty()
        {
            return Err(Error::new(
                "Error",
                "Private settings are tracked by Git; untrack them before writing",
            ));
        }
        let mut ignored = true;
        for name in names {
            let base = format!(".roster/{}", name.as_str());
            let args = |file: String| {
                vec![
                    "check-ignore".to_owned(),
                    "--quiet".to_owned(),
                    "--".to_owned(),
                    file,
                ]
            };
            let first = args(base.clone());
            let second = args(format!("{base}.tmp"));
            let (first, second) = tokio::join!(git(root, &first, token), git(root, &second, token));
            for result in [first, second] {
                match result {
                    Ok(_) => {}
                    Err(failure) if matches!(failure.kind, FailureKind::Exit(Some(1))) => {
                        ignored = false
                    }
                    Err(failure) => {
                        return Err(Error::new(
                            "Error",
                            "Could not verify private settings ignore rules",
                        )
                        .caused_by(process_error(failure)));
                    }
                }
            }
        }
        ignored
    } else {
        let normalized = source.normalized_lines();
        patterns.iter().all(|pattern| {
            normalized
                .0
                .split(|unit| *unit == 10)
                .any(|line| line.iter().copied().eq(pattern.encode_utf16()))
        })
    };
    if !ignored {
        if let Some(token) = token {
            token.check()?;
        }
        paths::ensure_local_path(&ignore_file, root)?;
        let separator = if !source.0.is_empty() && source.0.last() != Some(&10) {
            "\n"
        } else {
            ""
        };
        let mut options = fs::OpenOptions::new();
        options.append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&ignore_file)
            .map_err(|error| io_error("Open ignore rules", error))?;
        if !file
            .metadata()
            .map_err(|error| io_error("Inspect open ignore rules", error))?
            .is_file()
            || paths::handle_information(&file)?.links != 1
        {
            return Err(Error::new(
                "Error",
                ".gitignore must be a regular single-link file",
            ));
        }
        file.write_all(format!("{separator}{}\n", patterns.join("\n")).as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|error| io_error("Append private ignore rules", error))?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Document {
    pub name: Name,
    pub source: Text,
    pub expected_source: Option<Option<Text>>,
}

struct Prepared {
    file: PathBuf,
    temporary: PathBuf,
    name: Name,
    source: Text,
    previous: Option<Text>,
}

fn write_new(file: &Path, source: &Text) -> Result<(), Error> {
    let mut handle = files::create_private_file(file)
        .map_err(|error| io_error("Create private settings", error))?;
    handle
        .write_all(String::from_utf16_lossy(&source.0).as_bytes())
        .and_then(|()| handle.sync_all())
        .map_err(|error| io_error("Write private settings", error))
}

fn cleanup(prepared: &[Prepared], prior: Option<Error>) -> Result<(), Error> {
    let mut failure = prior;
    for document in prepared {
        if let Err(error) = fs::remove_file(&document.temporary) {
            if error.kind() != io::ErrorKind::NotFound {
                let next = io_error("Remove private settings temporary", error);
                failure = Some(match failure {
                    Some(prior) => next.caused_by(prior),
                    None => next,
                });
            }
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

pub fn write_private_documents(root: &Path, documents: &[Document]) -> Result<(), Error> {
    write_private_documents_with(root, documents, |from, to| fs::rename(from, to))
}

pub fn write_private_documents_with(
    root: &Path,
    documents: &[Document],
    mut rename: impl FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<(), Error> {
    if documents.is_empty()
        || documents
            .iter()
            .map(|document| document.name)
            .collect::<HashSet<_>>()
            .len()
            != documents.len()
    {
        return Err(Error::type_error("Private writes need distinct documents"));
    }
    let mut prepared = Vec::new();
    let mut committed = Vec::new();
    let outcome = (|| -> Result<(), Error> {
        for document in documents {
            if document.source.utf8_len() > 65_536 {
                return Err(Error::type_error(
                    "Private settings must be UTF-8 text of at most 64 KiB",
                ));
            }
            let file = file_path(root, document.name);
            let previous = read_private_file(root, document.name)?;
            if document
                .expected_source
                .as_ref()
                .is_some_and(|expected| *expected != previous)
            {
                return Err(Error::new(
                    "Error",
                    "Private settings changed during setup; refusing to overwrite them",
                ));
            }
            if previous.as_ref() == Some(&document.source) {
                continue;
            }
            let temporary = file.with_file_name(format!(
                "{}.{}.tmp",
                document.name.as_str(),
                files::nonce()?
            ));
            prepared.push(Prepared {
                file,
                temporary,
                name: document.name,
                source: document.source.clone(),
                previous,
            });
        }
        files::create_private_directories(&root.join(".roster"))?;
        for document in &prepared {
            paths::ensure_local_path(&document.file, root)?;
            write_new(&document.temporary, &document.source)?;
        }
        for document in &prepared {
            if let Some(previous) = &document.previous {
                let backup = document.file.with_file_name(format!(
                    "{}.{}.{}.bak",
                    document.name.as_str(),
                    clock::now_milliseconds()?,
                    files::nonce()?
                ));
                write_new(&backup, previous)?;
            }
        }
        for (index, document) in prepared.iter().enumerate() {
            if read_private_file(root, document.name)? != document.previous {
                return Err(Error::new(
                    "Error",
                    "Private settings changed before save; refusing to overwrite them",
                ));
            }
            rename(&document.temporary, &document.file)
                .map_err(|error| io_error("Save private settings", error))?;
            committed.push(index);
        }
        Ok(())
    })();
    if let Err(error) = outcome {
        let rollback = (|| -> Result<(), Error> {
            for index in committed.into_iter().rev() {
                let document = &prepared[index];
                if read_private_file(root, document.name)?.as_ref() != Some(&document.source) {
                    return Err(Error::new(
                        "Error",
                        "Private write failed and rollback was refused because a saved file changed",
                    ));
                }
                match &document.previous {
                    None => fs::remove_file(&document.file)
                        .map_err(|error| io_error("Rollback new private settings", error))?,
                    Some(previous) => {
                        write_new(&document.temporary, previous)?;
                        rename(&document.temporary, &document.file)
                            .map_err(|error| io_error("Restore private settings", error))?;
                    }
                }
            }
            Ok(())
        })();
        let error = match rollback {
            Ok(()) => error,
            Err(rollback) => rollback.caused_by(error),
        };
        return cleanup(&prepared, Some(error));
    }
    cleanup(&prepared, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn ignore_rule_updates_refuse_hardlinks_without_modifying_the_other_file()
    -> Result<(), Error> {
        let fixture = Fixture::new()?;
        let original = fixture.0.join("other.txt");
        fs::write(&original, b"untouched\n")
            .map_err(|error| io_error("Write hardlink fixture", error))?;
        fs::hard_link(&original, fixture.0.join(".gitignore"))
            .map_err(|error| io_error("Link ignore fixture", error))?;
        assert!(
            ensure_private_files_ignored(&fixture.0, &[Name::Config], None)
                .await
                .is_err_and(|error| error.message.contains("single-link"))
        );
        assert_eq!(
            fs::read(&original).map_err(|error| io_error("Read hardlink fixture", error))?,
            b"untouched\n"
        );
        Ok(())
    }

    #[test]
    fn private_settings_preserve_exact_utf8_bytes_and_atomic_replacement() -> Result<(), Error> {
        let fixture = Fixture::new()?;
        let source = "llm:\n  model: fixture-model\n";
        write_private_documents(&fixture.0, &[document(Name::Config, source)])?;
        assert_eq!(
            fs::read(fixture.0.join(".roster/config.yml"))
                .map_err(|error| io_error("Read config bytes", error))?,
            source.as_bytes()
        );
        assert_eq!(
            read_private_file(&fixture.0, Name::Config)?,
            Some(Text::from(source))
        );
        write_private_documents(&fixture.0, &[document(Name::Fleet, "profiles: []\n")])?;
        assert_eq!(
            read_private_file(&fixture.0, Name::Fleet)?,
            Some(Text::from("profiles: []\n"))
        );
        write_private_documents(
            &fixture.0,
            &[document(Name::Fleet, "profiles:\n  - id: updated\n")],
        )?;
        assert_eq!(
            fs::read(fixture.0.join(".roster/fleet.yml"))
                .map_err(|error| io_error("Read replaced fleet bytes", error))?,
            b"profiles:\n  - id: updated\n"
        );
        assert_eq!(
            fs::read(fixture.0.join(".roster/config.yml"))
                .map_err(|error| io_error("Read preserved config bytes", error))?,
            source.as_bytes()
        );
        Ok(())
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Result<Self, Error> {
            let path =
                std::env::temp_dir().join(format!("roster-native-settings-{}", files::nonce()?));
            files::create_private_directories(&path)?;
            Ok(Self(path))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.0) {
                eprintln!("Private settings fixture cleanup failed: {error}");
            }
        }
    }

    fn document(name: Name, source: &str) -> Document {
        Document {
            name,
            source: Text::from(source),
            expected_source: None,
        }
    }

    #[test]
    fn private_settings_preserve_backups_and_compare_expected_source() -> Result<(), Error> {
        let fixture = Fixture::new()?;
        assert_eq!(read_private_file(&fixture.0, Name::Config)?, None);
        write_private_documents(&fixture.0, &[document(Name::Config, "first")])?;
        let mut next = document(Name::Config, "second");
        next.expected_source = Some(Some(Text::from("wrong")));
        assert!(write_private_documents(&fixture.0, &[next.clone()]).is_err());
        assert_eq!(
            read_private_file(&fixture.0, Name::Config)?,
            Some(Text::from("first"))
        );
        next.expected_source = Some(Some(Text::from("first")));
        write_private_documents(&fixture.0, &[next])?;
        assert_eq!(
            read_private_file(&fixture.0, Name::Config)?,
            Some(Text::from("second"))
        );
        let backups = fs::read_dir(fixture.0.join(".roster"))
            .map_err(|error| io_error("Read fixture", error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| io_error("Read fixture entry", error))?;
        assert!(
            backups
                .iter()
                .any(|entry| entry.file_name().to_string_lossy().ends_with(".bak"))
        );
        assert!(
            !backups
                .iter()
                .any(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
        );
        Ok(())
    }

    #[test]
    fn partial_multi_document_commits_roll_back_and_clean_all_temporaries() -> Result<(), Error> {
        let fixture = Fixture::new()?;
        write_private_documents(
            &fixture.0,
            &[
                document(Name::Config, "old config"),
                document(Name::Fleet, "old fleet"),
            ],
        )?;
        let mut calls = 0;
        let result = write_private_documents_with(
            &fixture.0,
            &[
                document(Name::Config, "new config"),
                document(Name::Fleet, "new fleet"),
            ],
            |from, to| {
                calls += 1;
                if calls == 2 {
                    Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "injected second commit failure",
                    ))
                } else {
                    fs::rename(from, to)
                }
            },
        );
        assert!(result.is_err());
        assert_eq!(calls, 3);
        assert_eq!(
            read_private_file(&fixture.0, Name::Config)?,
            Some(Text::from("old config"))
        );
        assert_eq!(
            read_private_file(&fixture.0, Name::Fleet)?,
            Some(Text::from("old fleet"))
        );
        for entry in fs::read_dir(fixture.0.join(".roster"))
            .map_err(|error| io_error("Read fixture", error))?
        {
            assert!(
                !entry
                    .map_err(|error| io_error("Read fixture entry", error))?
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            );
        }
        Ok(())
    }

    #[test]
    fn private_config_reads_reject_bad_utf8_and_invalid_document_names() -> Result<(), Error> {
        let fixture = Fixture::new()?;
        files::create_private_directories(&fixture.0.join(".roster"))?;
        let file = file_path(&fixture.0, Name::Config);
        fs::write(&file, [0xff]).map_err(|error| io_error("Write invalid fixture", error))?;
        assert!(
            read_private_file(&fixture.0, Name::Config)
                .is_err_and(|error| error.message.contains("UTF-8"))
        );
        assert!(Name::parse("../outside").is_err());
        assert!(
            write_private_documents(
                &fixture.0,
                &[document(Name::Config, "a"), document(Name::Config, "b")]
            )
            .is_err()
        );
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn private_ignore_rules_are_verified_with_actual_git_and_preserve_tracked_denial()
    -> Result<(), Error> {
        let fixture = Fixture::new()?;
        let init = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&fixture.0)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|error| io_error("Initialize Git fixture", error))?;
        if !init.status.success() {
            return Err(Error::new("Error", "Git fixture initialization failed"));
        }
        ensure_private_files_ignored(&fixture.0, &[Name::Config, Name::Fleet], None).await?;
        write_private_documents(&fixture.0, &[document(Name::Config, "private fixture")])?;
        ensure_private_files_ignored(&fixture.0, &[Name::Config], None).await?;
        let add = std::process::Command::new("git")
            .args(["add", "--force", "--", ".roster/config.yml"])
            .current_dir(&fixture.0)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|error| io_error("Track private fixture", error))?;
        if !add.status.success() {
            return Err(Error::new("Error", "Git fixture staging failed"));
        }
        assert!(
            ensure_private_files_ignored(&fixture.0, &[Name::Config], None)
                .await
                .is_err_and(|error| error.message.contains("tracked"))
        );
        Ok(())
    }
}
