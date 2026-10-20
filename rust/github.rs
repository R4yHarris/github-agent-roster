use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::time::Instant;
use zeroize::{Zeroize, Zeroizing};

use crate::cancel::CancellationToken;
use crate::execution::{owned, tools};
use crate::json::{self, Value};
use crate::llm::http::{Body, Request, Transport};
use crate::process::Limits;
use crate::state::{redaction, secure};
use crate::text::Text;
use crate::{Error, config, domain, host, identity, paths, start};

#[cfg(test)]
mod tests;

const MAX_RESPONSE: usize = 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const REVOCATION_TIMEOUT: Duration = Duration::from_secs(5);
const SDK_AUTH: &str = r#"
import { closeSync, fstatSync, openSync, readSync } from "node:fs";
import { pathToFileURL } from "node:url";
let key;
try {
  if (process.versions.node.split(".")[0] !== "20") throw new Error("node");
  const chunks = [];
  let bytes = 0;
  for await (const chunk of process.stdin) {
    bytes += chunk.length;
    if (bytes > 65536) throw new Error("input");
    chunks.push(chunk);
  }
  const input = JSON.parse(Buffer.concat(chunks).toString("utf8"));
  const sdk = await import(pathToFileURL(input.sdk).href);
  const scope = sdk.parseOrigin(input.origin);
  const descriptor = openSync(input.privateKeyPath, "r");
  let length = 0;
  try {
    const metadata = fstatSync(descriptor);
    if (!metadata.isFile() || metadata.nlink !== 1 || metadata.size > 65536) throw new Error("key");
    key = Buffer.alloc(65537);
    while (length < key.length) {
      const count = readSync(descriptor, key, length, key.length - length, null);
      if (count === 0) break;
      length += count;
    }
  } finally {
    closeSync(descriptor);
  }
  if (length === 0 || length > 65536) throw new Error("key");
  const access = await sdk.mintInstallationToken({
    appId: input.appId,
    privateKey: key.subarray(0, length),
    owner: scope.owner,
    repository: scope.repository,
    additionalPermissions: { issues: "read" }
  });
  process.stdout.write(JSON.stringify({
    token: access.token,
    appSlug: access.appSlug,
    repository: `${scope.owner}/${scope.repository}`
  }) + "\n");
} catch {
  process.stderr.write("App SDK authentication unavailable.\n");
  process.exitCode = 1;
} finally {
  if (Buffer.isBuffer(key)) key.fill(0);
}
"#;

fn error(code: &'static str, message: &str) -> Error {
    Error::new("GitHubIssueError", message).coded(code)
}

fn safe_error(cause: Error) -> Error {
    if cause.name == "GitHubIssueError"
        || matches!(
            cause.code,
            Some("ROSTER_CANCELLED" | "ROSTER_TASK_TIMEOUT" | "ROSTER_PROCESS_CLEANUP")
        )
    {
        return cause;
    }
    let mut safe = Error::new(cause.name, "Private GitHub provider details were omitted");
    safe.code = cause.code;
    error(
        "ROSTER_GITHUB_ISSUE",
        "App-scoped GitHub issue observation failed; no task work or publication was performed",
    )
    .caused_by(safe)
}

async fn cancelled(token: Option<&CancellationToken>) -> Result<crate::cancel::Reason, Error> {
    match token {
        Some(token) => token.cancelled().await,
        None => std::future::pending().await,
    }
}

async fn wait<T>(
    operation: impl Future<Output = T>,
    token: Option<&CancellationToken>,
    deadline: Instant,
) -> Result<T, Error> {
    if let Some(token) = token {
        token.check()?;
    }
    tokio::pin!(operation);
    let value = tokio::select! {
        biased;
        reason = cancelled(token) => return Err(reason?.error()),
        () = tokio::time::sleep_until(deadline) => return Err(error("ROSTER_GITHUB_TIMEOUT", "GitHub observation exceeded its bounded deadline")),
        value = &mut operation => value,
    };
    if let Some(token) = token {
        token.check()?;
    }
    if Instant::now() >= deadline {
        return Err(error(
            "ROSTER_GITHUB_TIMEOUT",
            "GitHub observation exceeded its bounded deadline",
        ));
    }
    Ok(value)
}

fn string<'a>(value: &'a Value, name: &str) -> Result<&'a Text, Error> {
    value.get(name).and_then(Value::as_string).ok_or_else(|| {
        error(
            "ROSTER_GITHUB_PROTOCOL",
            "GitHub observation returned incomplete typed metadata",
        )
    })
}

fn path_key(path: &Path) -> Result<PathBuf, Error> {
    let path = paths::strip_verbatim(paths::absolute(path)?);
    let source = path
        .to_str()
        .ok_or_else(|| error("ROSTER_GITHUB_HOST", "App paths must be Unicode"))?;
    Ok(PathBuf::from(if cfg!(windows) {
        source.to_lowercase()
    } else {
        source.to_owned()
    }))
}

struct Credentials {
    app_id: Text,
    key: PathBuf,
    node: host::Image,
}

impl Credentials {
    fn select(environment: &Value) -> Result<Self, Error> {
        let missing = || {
            error(
                "ROSTER_GITHUB_APP",
                "GitHub intake requires complete GITHUB_APP_ID and GITHUB_APP_PRIVATE_KEY_PATH; human authentication is never substituted",
            )
        };
        let app_id = config::environment_value(environment, "GITHUB_APP_ID")?
            .ok_or_else(missing)?
            .clone();
        let id = app_id.to_utf8()?;
        if id.is_empty()
            || id.len() > 20
            || id.starts_with('0')
            || !id.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(missing());
        }
        let key =
            host::selected_path(environment, "GITHUB_APP_PRIVATE_KEY_PATH")?.ok_or_else(missing)?;
        let node = host::selected_path(environment, "ROSTER_NODE20")?.ok_or_else(|| {
            error(
                "ROSTER_GITHUB_HOST",
                "The contracts identity adapter requires an absolute approved ROSTER_NODE20",
            )
        })?;
        Ok(Self {
            app_id,
            key,
            node: host::Image::approve(&node)?,
        })
    }
}

pub fn validate_app_configuration(environment: &Value) -> Result<(), Error> {
    Credentials::select(environment)
        .map(|_| ())
        .map_err(safe_error)
}

struct Access {
    token: Zeroizing<String>,
    scope: Result<Scope, Error>,
}

struct Scope {
    app: String,
    repository: String,
}

fn scope(value: &Value, expected: &str) -> Result<Scope, Error> {
    let members = domain::members(value).ok_or_else(|| {
        error(
            "ROSTER_GITHUB_PROTOCOL",
            "The App SDK returned incomplete authentication evidence",
        )
    })?;
    if members.len() != 3
        || members.iter().any(|(name, _)| {
            !["token", "appSlug", "repository"]
                .iter()
                .any(|field| name == &Text::from(*field))
        })
    {
        return Err(error(
            "ROSTER_GITHUB_PROTOCOL",
            "The App SDK returned incomplete authentication evidence",
        ));
    }
    let app = string(value, "appSlug")?.to_utf8()?;
    let repository = string(value, "repository")?.to_utf8()?;
    let parts = repository.split('/').collect::<Vec<_>>();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || part.len() > 100
                || matches!(*part, "." | "..")
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
        || app.is_empty()
        || app.len() > 100
        || app.starts_with('-')
        || !app
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || identity::identity_hash(&Text::from(
            format!("https://github.com/{repository}").as_str(),
        ))? != expected
    {
        return Err(error(
            "ROSTER_GITHUB_PROTOCOL",
            "The App SDK authentication does not match the checked repository",
        ));
    }
    Ok(Scope { app, repository })
}

fn access(mut bytes: Vec<u8>, expected: &str) -> Result<Access, Error> {
    let decoded = std::str::from_utf8(&bytes).map(Text::from);
    bytes.zeroize();
    let mut source = decoded.map_err(|_| {
        error(
            "ROSTER_GITHUB_PROTOCOL",
            "The App SDK returned an invalid protocol response",
        )
    })?;
    let parsed = json::parse(&source);
    source.0.zeroize();
    let mut value = parsed.map_err(|_| {
        error(
            "ROSTER_GITHUB_PROTOCOL",
            "The App SDK returned an invalid protocol response",
        )
    })?;
    let result = (|| {
        let token = Zeroizing::new(string(&value, "token")?.to_utf8()?);
        if !(1..=4096).contains(&token.len())
            || !token.bytes().all(|byte| (33..=126).contains(&byte))
        {
            return Err(error(
                "ROSTER_GITHUB_PROTOCOL",
                "The App SDK returned no usable installation token; issuance is unconfirmed and must not be retried automatically",
            ));
        }
        Ok(Access {
            token,
            scope: scope(&value, expected),
        })
    })();
    if let Value::Object(fields) = &mut value {
        for (_, value) in fields {
            if let Value::String(value) = value {
                value.0.zeroize();
            }
        }
    }
    result
}

struct SecretRequest(Request);

impl Drop for SecretRequest {
    fn drop(&mut self) {
        for (name, value) in &mut self.0.headers {
            if name.eq_ignore_ascii_case("authorization") {
                value.zeroize();
            }
        }
    }
}

async fn api<T: Transport>(
    transport: &mut T,
    access: &Access,
    path: &str,
    method: &str,
    status: u16,
    token: Option<&CancellationToken>,
    deadline: Instant,
) -> Result<Vec<u8>, Error> {
    let request = SecretRequest(Request {
        url: format!("https://api.github.com/{path}"),
        method: method.to_owned(),
        headers: vec![
            (
                "Accept".to_owned(),
                "application/vnd.github+json".to_owned(),
            ),
            (
                "Authorization".to_owned(),
                format!("Bearer {}", access.token.as_str()),
            ),
            ("User-Agent".to_owned(), "github-agent-roster".to_owned()),
            ("X-GitHub-Api-Version".to_owned(), "2022-11-28".to_owned()),
        ],
        body: Vec::new(),
    });
    let mut response = wait(transport.send(&request.0), token, deadline)
        .await?
        .map_err(|_| {
            error(
                "ROSTER_GITHUB_HTTP",
                "The App GitHub request did not complete",
            )
        })?;
    if response.status != status {
        return Err(error(
            "ROSTER_GITHUB_HTTP",
            "The App GitHub response did not acknowledge the requested operation",
        ));
    }
    let mut bytes = Vec::new();
    if let Some(body) = &mut response.body {
        while let Some(chunk) = wait(body.read(), token, deadline).await?.map_err(|_| {
            error(
                "ROSTER_GITHUB_HTTP",
                "The App GitHub response could not be read",
            )
        })? {
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE {
                return Err(error(
                    "ROSTER_GITHUB_LIMIT",
                    "The App GitHub response exceeded its 1 MiB bound",
                ));
            }
            bytes.extend(chunk);
        }
    } else if status != 204 {
        return Err(error(
            "ROSTER_GITHUB_PROTOCOL",
            "The GitHub issue response has no body",
        ));
    }
    if status == 204 && !bytes.is_empty() {
        return Err(error(
            "ROSTER_GITHUB_PROTOCOL",
            "App token revocation returned an unexpected body",
        ));
    }
    Ok(bytes)
}

struct Facts {
    number: u64,
    title: Text,
    body: Text,
    url: String,
    state: &'static str,
    app: String,
}

impl Facts {
    fn number(&self) -> u64 {
        self.number
    }

    fn body(&self) -> &Text {
        &self.body
    }

    fn metadata(&self) -> Value {
        Value::object(vec![
            ("number", Value::Number(self.number as f64)),
            ("title", Value::String(self.title.clone())),
            ("url", Value::string(&self.url)),
            ("state", Value::string(self.state)),
            ("app", Value::string(&self.app)),
        ])
    }
}

pub struct Issue {
    facts: Facts,
    context: Context,
}

struct Context {
    paths: paths::RepositoryPaths,
    worktree: paths::FileIdentity,
    common: paths::FileIdentity,
    identity: String,
    git: start::Git,
    sdk: start::Contracts,
}

impl Issue {
    pub fn number(&self) -> u64 {
        self.facts.number()
    }

    pub fn body(&self) -> &Text {
        self.facts.body()
    }

    pub fn title(&self) -> &Text {
        &self.facts.title
    }

    pub fn url(&self) -> &str {
        &self.facts.url
    }

    pub fn is_open(&self) -> bool {
        self.facts.state == "OPEN"
    }

    pub fn metadata(&self) -> Value {
        self.facts.metadata()
    }

    pub fn cleanup_certain(&self) -> bool {
        self.context.git.cleanup_certain()
    }

    pub async fn require_current(&self, token: &CancellationToken) -> Result<(), Error> {
        self.require_current_with_git(&self.context.git, token)
            .await
    }

    pub(crate) async fn require_current_with_git(
        &self,
        git: &start::Git,
        token: &CancellationToken,
    ) -> Result<(), Error> {
        let result = async {
            token.check()?;
            if !self.cleanup_certain() || !git.cleanup_certain() {
                return Err(error(
                    "ROSTER_PROCESS_CLEANUP",
                    "Issue observation Git cleanup is unverified; no task or status may proceed",
                ));
            }
            let current = paths::repository_paths(&self.context.paths.worktree)?;
            if paths::file_information(&current.worktree)?.identity != self.context.worktree
                || paths::file_information(&current.common_dir)?.identity != self.context.common
            {
                return Err(error(
                    "ROSTER_GITHUB_SCOPE",
                    "The issue observation repository identity changed",
                ));
            }
            self.context.sdk.require_current(git, token).await?;
            let origin = git
                .text(
                    &current.worktree,
                    &["config", "--get", "remote.origin.url"],
                    token,
                )
                .await?;
            if identity::identity_hash(&Text::from(origin.as_str()))? != self.context.identity {
                return Err(error(
                    "ROSTER_GITHUB_SCOPE",
                    "Repository origin changed after the issue observation",
                ));
            }
            if !self.cleanup_certain() || !git.cleanup_certain() {
                return Err(error(
                    "ROSTER_PROCESS_CLEANUP",
                    "Issue observation Git cleanup is unverified; no task or status may proceed",
                ));
            }
            token.check()
        }
        .await;
        result.map_err(safe_error)
    }
}

fn issue(bytes: &[u8], number: u64, access: &Access, scope: &Scope) -> Result<Facts, Error> {
    let source = std::str::from_utf8(bytes).map_err(|_| {
        error(
            "ROSTER_GITHUB_PROTOCOL",
            "The GitHub issue response must be UTF-8",
        )
    })?;
    let value = json::parse(&Text::from(source)).map_err(|_| {
        error(
            "ROSTER_GITHUB_PROTOCOL",
            "The GitHub issue response must be complete JSON",
        )
    })?;
    let expected = format!("https://github.com/{}/issues/{number}", scope.repository);
    let title = string(&value, "title")?.clone();
    let url = string(&value, "html_url")?.to_utf8()?;
    let state = match value.get("state") {
        Some(value) if domain::is_text(Some(value), "open") => "OPEN",
        Some(value) if domain::is_text(Some(value), "closed") => "CLOSED",
        _ => {
            return Err(error(
                "ROSTER_GITHUB_PROTOCOL",
                "The GitHub issue state is invalid",
            ));
        }
    };
    let body = match value.get("body") {
        Some(Value::Null) => Text::default(),
        Some(Value::String(value)) if value.0.len() <= 65_536 => value.clone(),
        _ => {
            return Err(error(
                "ROSTER_GITHUB_PROTOCOL",
                "The GitHub issue body is missing or oversized",
            ));
        }
    };
    if domain::safe_integer(value.get("number")) != Some(number)
        || !url.eq_ignore_ascii_case(&expected)
        || crate::text::trim(&title.0).is_empty()
        || title.0.len() > 1024
        || title
            .0
            .iter()
            .any(|unit| matches!(unit, 0..=31 | 127..=159 | 8232 | 8233))
        || value.get("pull_request").is_some()
    {
        return Err(error(
            "ROSTER_GITHUB_PROTOCOL",
            "The App response does not identify the requested GitHub issue",
        ));
    }
    let redaction = redaction::Config {
        credential_values: vec![Text::from(access.token.as_str())],
        ..Default::default()
    };
    Ok(Facts {
        number,
        title: redaction::diagnostic(&title, &redaction),
        body: redaction::evidence(&body, &redaction),
        url: expected,
        state,
        app: scope.app.clone(),
    })
}

pub async fn read_issue<T: Transport>(
    transport: &mut T,
    installation: &Path,
    cwd: &Path,
    environment: &Value,
    number: u64,
    token: &CancellationToken,
) -> Result<Issue, Error> {
    let result = async {
        token.check()?;
        if number == 0 || number > crate::MAX_SAFE_INTEGER {
            return Err(error("ROSTER_GITHUB_INPUT", "GitHub issue number must be a positive safe integer"));
        }
        let credentials = Credentials::select(environment)?;
        let paths = paths::repository_paths(cwd)?;
        let installation_paths = paths::repository_paths(installation)?;
        let worktree = paths::file_information(&paths.worktree)?.identity;
        let common = paths::file_information(&paths.common_dir)?.identity;
        let key_path = path_key(&credentials.key)?;
        let mut key_in_source = false;
        for root in [
            &paths.worktree,
            &paths.repository,
            &installation_paths.worktree,
            &installation_paths.repository,
        ] {
            key_in_source |= key_path.starts_with(path_key(root)?);
        }
        secure::check_components(&credentials.key)?;
        let key = paths::file_information(&credentials.key)?;
        let metadata = fs::symlink_metadata(&credentials.key).map_err(|_| {
            error("ROSTER_GITHUB_APP", "The App private-key file could not be inspected")
        })?;
        if !metadata.is_file()
            || secure::linked(&metadata)
            || key.links != 1
            || metadata.len() > 65_536
            || key_in_source
        {
            return Err(error("ROSTER_GITHUB_APP", "App authentication requires an unlinked bounded key file outside both repositories"));
        }
        let git_image = host::selected_path(environment, "ROSTER_GIT")?.ok_or_else(|| {
            error("ROSTER_GITHUB_HOST", "App issue observation requires an absolute approved ROSTER_GIT")
        })?;
        let git = start::Git::new(
            tools::Host {
                git: git_image,
                environment: host::minimal_environment(environment)?,
                ..tools::Host::default()
            },
            paths.git_dir.join("roster-github-disabled-hooks"),
        )?;
        let sdk = paths::resolve_contracts_path(installation, cwd, host::contracts_override(environment)?.as_deref())?;
        let seal = start::Contracts::open(installation, &sdk, &git, token).await?;
        let origin = git.text(&paths.worktree, &["config", "--get", "remote.origin.url"], token).await?;
        let expected = identity::identity_hash(&Text::from(origin.as_str()))?;
        let input = Value::object(vec![
            ("sdk", Value::string(sdk.join("scripts").join("agent-pr.mjs").to_str().ok_or_else(|| error("ROSTER_GITHUB_HOST", "SDK path must be Unicode"))?)),
            ("origin", Value::string(&origin)),
            ("appId", Value::String(credentials.app_id.clone())),
            ("privateKeyPath", Value::string(credentials.key.to_str().ok_or_else(|| error("ROSTER_GITHUB_HOST", "App key path must be Unicode"))?)),
        ]);
        let mut command = tokio::process::Command::new(credentials.node.path());
        command
            .current_dir(&paths.worktree)
            .env_clear()
            .envs(host::minimal_environment(environment)?)
            .args(["--input-type=module", "--eval", SDK_AUTH]);
        credentials.node.require_current()?;
        let mut output = owned::execute_with_input(
            command,
            Limits { timeout: Some(REQUEST_TIMEOUT), max_output_bytes: 65_536 },
            Some(token),
            None,
            Some(owned::FiniteInput::try_from(input.json().into_bytes())?),
        ).await.map_err(|mut failure| {
            let cancellation = match &failure.kind {
                crate::process::FailureKind::Cancelled(reason) => Some(reason.error()),
                crate::process::FailureKind::Control(cause)
                    if matches!(cause.code, Some("ROSTER_CANCELLED" | "ROSTER_TASK_TIMEOUT")) =>
                {
                    Some((**cause).clone())
                }
                _ => None,
            };
            failure.captured.stdout.zeroize();
            failure.captured.stderr.zeroize();
            if !failure.cleanup_complete {
                let failure = error("ROSTER_PROCESS_CLEANUP", "App SDK cleanup is unverified; no task may start from this observation");
                if let Some(cause) = cancellation {
                    failure.caused_by(cause)
                } else {
                    failure
                }
            } else if let Some(reason) = cancellation {
                reason
            } else {
                error("ROSTER_GITHUB_APP", "App SDK authentication failed; no human authentication or automatic retry was used")
            }
        })?;
        output.captured.stderr.zeroize();
        let access = access(output.captured.stdout, &expected)?;
        let observed = async {
            let scope = access.scope.as_ref().map_err(Clone::clone)?;
            credentials.node.require_current()?;
            if paths::file_information(&credentials.key)?.identity != key.identity {
                return Err(error("ROSTER_GITHUB_APP", "The App key identity changed during authentication"));
            }
            seal.require_current(&git, token).await?;
            if identity::identity_hash(&Text::from(git.text(&paths.worktree, &["config", "--get", "remote.origin.url"], token).await?.as_str()))? != expected {
                return Err(error("ROSTER_GITHUB_SCOPE", "Repository identity changed during App authentication"));
            }
            let bytes = api(
                transport,
                &access,
                &format!("repos/{}/issues/{number}", scope.repository),
                "GET",
                200,
                Some(token),
                Instant::now() + REQUEST_TIMEOUT,
            ).await?;
            issue(&bytes, number, &access, scope)
        }.await;
        let revoked = api(
            transport,
            &access,
            "installation/token",
            "DELETE",
            204,
            None,
            Instant::now() + REVOCATION_TIMEOUT,
        ).await;
        if let Err(cause) = revoked {
            let failure = error("ROSTER_GITHUB_REVOCATION", "App token revocation was not acknowledged; observation is unavailable and must not be retried automatically").caused_by(cause);
            if let Err(reason) = token.check() {
                return Err(reason.caused_by(failure));
            }
            return Err(match observed {
                Err(primary) if matches!(primary.code, Some("ROSTER_CANCELLED" | "ROSTER_TASK_TIMEOUT" | "ROSTER_PROCESS_CLEANUP")) => primary.caused_by(failure),
                Err(primary) => failure.caused_by(primary),
                Ok(_) => failure,
            });
        }
        if let Err(reason) = token.check() {
            return Err(match observed {
                Err(cause) => reason.caused_by(cause),
                Ok(_) => reason,
            });
        }
        let observed = observed?;
        let observed = Issue {
            facts: observed,
            context: Context { paths, worktree, common, identity: expected, git, sdk: seal },
        };
        observed.require_current(token).await?;
        Ok(observed)
    }.await;
    result.map_err(safe_error)
}
