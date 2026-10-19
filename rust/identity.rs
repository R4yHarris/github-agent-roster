use sha2::{Digest, Sha256};
use url::Url;

use crate::Error;
use crate::cancel::CancellationToken;
use crate::process::{self, FailureKind, Limits};
use crate::text::{Text, trim};
use std::path::Path;
use std::time::Duration;

fn invalid(message: &str) -> Error {
    Error::new("RepoIdentityError", message).coded("E_IDENTITY_INPUT")
}

fn has_url_scheme(value: &str) -> bool {
    let Some((scheme, _)) = value.split_once("://") else {
        return false;
    };
    scheme
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphabetic)
        && scheme
            .bytes()
            .all(|unit| unit.is_ascii_alphanumeric() || matches!(unit, b'+' | b'-' | b'.'))
}

fn absolute_local_path(value: &str) -> bool {
    let drive = value.as_bytes();
    value.starts_with('/')
        || cfg!(windows) && value.starts_with('\\')
        || drive.len() >= 3
            && drive[0].is_ascii_alphabetic()
            && drive[1] == b':'
            && matches!(drive[2], b'/' | b'\\')
}

fn scp_remote(value: &str) -> Option<(&str, &str)> {
    if absolute_local_path(value) {
        return None;
    }
    let host_path = value
        .split_once('@')
        .filter(|(user, _)| !user.is_empty() && !user.contains(['@', '/', ':']))
        .map_or(value, |(_, rest)| rest);
    let valid_path =
        |path: &str| !path.is_empty() && !path.contains(['\n', '\r', '\u{2028}', '\u{2029}']);
    let bracketed = host_path
        .starts_with('[')
        .then(|| host_path.find(']'))
        .flatten()
        .and_then(|bracket| {
            host_path
                .get(bracket + 1..)?
                .strip_prefix(':')
                .filter(|path| valid_path(path))
                .map(|path| (&host_path[..=bracket], path))
        });
    let (host, path) = if let Some(remote) = bracketed {
        remote
    } else {
        let (host, path) = host_path.split_once(':')?;
        if host.is_empty() || host.contains('/') {
            return None;
        }
        (host, path)
    };
    if !valid_path(path) {
        return None;
    }
    Some((host, path))
}

fn canonical_remote(source: &Text) -> Result<String, Error> {
    let trimmed = String::from_utf16_lossy(trim(&source.0));
    let (host, port, repo_path) = if has_url_scheme(&trimmed) {
        let url = Url::parse(&trimmed).map_err(|_| invalid("Invalid repository remote URL."))?;
        if !["https", "http", "ssh", "git", "file"].contains(&url.scheme())
            || url.query().is_some_and(|value| !value.is_empty())
            || url.fragment().is_some_and(|value| !value.is_empty())
        {
            return Err(invalid("Unsupported repository remote URL or selector."));
        }
        let host = url.host_str().unwrap_or("").to_lowercase();
        if url.scheme() == "file" {
            return Ok(format!("file:{host}{}", url.path()));
        }
        let port = url
            .port()
            .filter(|port| !(url.scheme() == "ssh" && *port == 22));
        (host, port, url.path().to_owned())
    } else if let Some((host, path)) = scp_remote(&trimmed) {
        (host.to_lowercase(), None, path.to_owned())
    } else {
        return Ok(format!("local:{trimmed}"));
    };
    let path = repo_path.trim_start_matches('/').trim_end_matches('/');
    let path = if path.len() >= 4 && path.as_bytes()[path.len() - 4..].eq_ignore_ascii_case(b".git")
    {
        &path[..path.len() - 4]
    } else {
        path
    };
    let path = if host == "github.com" {
        path.to_lowercase()
    } else {
        path.to_owned()
    };
    if host.is_empty() || path.is_empty() {
        return Err(invalid(
            "Repository remote requires a host and repository path.",
        ));
    }
    let port = port.map_or(String::new(), |port| format!(":{port}"));
    Ok(format!("{host}{port}/{path}"))
}

pub(crate) fn sha256_hex(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

pub fn identity_hash(remote: &Text) -> Result<String, Error> {
    if trim(&remote.0).is_empty() {
        return Err(invalid("repo identity requires the origin remote URL."));
    }
    let canonical = canonical_remote(remote)?;
    Ok(format!(
        "sha256-{}",
        sha256_hex(format!("remote-url:{canonical}\n").as_bytes())
    ))
}

fn query_failure(root: &Path, common: bool, failure: process::Failure) -> Error {
    match &failure.kind {
        FailureKind::Cancelled(reason) => return reason.error(),
        FailureKind::Control(error)
            if error.code == Some("ROSTER_CANCELLED")
                || error.code == Some("ROSTER_TASK_TIMEOUT") =>
        {
            return (**error).clone();
        }
        FailureKind::Cleanup => {
            return Error::new("Error", failure.to_string()).coded("ROSTER_PROCESS_CLEANUP");
        }
        _ => {}
    }
    let (action, hint) = if common {
        (
            "the git common dir",
            "Run inside a git repository or initialize one with `git init`.",
        )
    } else {
        (
            "the origin remote",
            "Add one with `git remote add origin <url>`, or record the repository identity manually.",
        )
    };
    let cause = Error::new("Error", failure.to_string());
    Error::new(
        "RepoIdentityError",
        format!(
            "could not read {action} for {}: {cause}. {hint}",
            root.display()
        ),
    )
    .coded("E_GIT_METADATA")
    .caused_by(cause)
}

async fn git_query(
    root: &Path,
    args: &[&str],
    token: Option<&CancellationToken>,
) -> Result<Text, process::Failure> {
    let mut command = tokio::process::Command::new("git");
    command.arg("--no-pager").args(args).current_dir(root);
    let output = process::execute(
        &mut command,
        Limits {
            timeout: Some(Duration::from_secs(10)),
            max_output_bytes: 1024 * 1024,
        },
        token,
    )
    .await?;
    Ok(Text::from(
        String::from_utf8_lossy(&output.captured.stdout).as_ref(),
    ))
}

pub async fn resolve_repo_identity(
    root: &Path,
    token: Option<&CancellationToken>,
) -> Result<String, Error> {
    let (common, origin) = tokio::join!(
        git_query(root, &["rev-parse", "--git-common-dir"], token),
        git_query(root, &["config", "--get", "remote.origin.url"], token),
    );
    let common = common.map_err(|error| query_failure(root, true, error))?;
    if trim(&common.0).is_empty() {
        return Err(invalid(
            "git returned an empty common directory; refusing ambiguous repository identity.",
        ));
    }
    let origin = origin.map_err(|error| query_failure(root, false, error))?;
    let origin = Text(trim(&origin.0).to_vec());
    if origin.0.is_empty() {
        return Err(invalid(
            "Origin remote is empty; refusing ambiguous repository identity.",
        ));
    }
    let text = String::from_utf16_lossy(&origin.0);
    let scp_like = text.split_once(':').is_some_and(|(host, path)| {
        !host.is_empty()
            && !host.contains('/')
            && path
                .chars()
                .next()
                .is_some_and(|first| !matches!(first, '\n' | '\r' | '\u{2028}' | '\u{2029}'))
    });
    if !has_url_scheme(&text) && !scp_like {
        let remote = crate::paths::resolve(root, Path::new(&text))?;
        return identity_hash(&Text::from(remote.to_string_lossy().as_ref()));
    }
    identity_hash(&origin)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Comparison {
    Match,
    Initialize(String),
    Mismatch { recorded: String, current: String },
}

impl Comparison {
    pub fn json(&self) -> String {
        match self {
            Self::Match => "{\"status\":\"match\"}".to_owned(),
            Self::Initialize(identity) => format!(
                "{{\"status\":\"initialize\",\"identity\":{}}}",
                Text::from(identity.as_str()).json()
            ),
            Self::Mismatch { recorded, current } => format!(
                "{{\"status\":\"mismatch\",\"recorded\":{},\"current\":{},\"message\":{}}}",
                Text::from(recorded.as_str()).json(),
                Text::from(current.as_str()).json(),
                Text::from(
                    "the recorded repository identity does not match this checkout. \
                     Restore the original origin remote or explicitly reinitialize repository state after preserving it. \
                     Legacy path-derived history remains untouched and is not automatically reassigned."
                ).json()
            ),
        }
    }
}

pub fn compare_identity(recorded: Option<&str>, current: &str) -> Comparison {
    match recorded {
        Some(recorded) if recorded == current => Comparison::Match,
        None => Comparison::Initialize(current.to_owned()),
        Some(recorded) => Comparison::Mismatch {
            recorded: recorded.to_owned(),
            current: current.to_owned(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_identity_ignores_credentials_and_equivalent_transport() -> Result<(), Error> {
        let expected = identity_hash(&Text::from("https://git.example.invalid/team/repo.git"))?;
        for remote in [
            "git@git.example.invalid:team/repo.git",
            "ssh://git@git.example.invalid:22/team/repo.git",
            "https://reader@git.example.invalid:443/team/repo.git/",
            "https://GIT.EXAMPLE.INVALID/team/repo",
        ] {
            assert_eq!(identity_hash(&Text::from(remote))?, expected);
        }
        assert_eq!(
            identity_hash(&Text::from("git@github.com:Owner/Repo.git"))?,
            identity_hash(&Text::from("https://github.com/owner/repo"))?
        );
        assert!(!expected.contains("example.invalid"));
        Ok(())
    }

    #[test]
    fn identities_preserve_server_and_path_boundaries() -> Result<(), Error> {
        let expected = identity_hash(&Text::from("https://git.example.invalid/team/repo.git"))?;
        for remote in [
            "https://git.example.invalid/Team/Repo.git",
            "https://git.example.invalid:8443/team/repo.git",
            "https://git.example.invalid/other/repo.git",
            "/srv/team/repo.git",
        ] {
            assert_ne!(identity_hash(&Text::from(remote))?, expected);
        }
        for remote in [
            "",
            " ",
            "https://example.invalid/",
            "https://example.invalid/repo?selector=other",
            "https://example.invalid/repo#head",
            "ftp://example.invalid/repo",
        ] {
            assert!(identity_hash(&Text::from(remote)).is_err());
        }
        assert_eq!(
            compare_identity(None, &expected),
            Comparison::Initialize(expected.clone())
        );
        assert_eq!(
            compare_identity(Some(&expected), &expected),
            Comparison::Match
        );
        Ok(())
    }
}
