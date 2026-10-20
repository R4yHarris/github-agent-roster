use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use tokio::process::Command;
use url::Url;
use zeroize::Zeroize;

use crate::cancel::CancellationToken;
use crate::execution::{owned, tools};
use crate::json::{self, Value};
use crate::process::Limits;
use crate::text::Text;
use crate::{Error, config, domain, host, paths, start};

const MAX_RESPONSE: usize = 1024 * 1024;

fn error(code: &'static str, message: &str) -> Error {
    Error::new("HumanFeedbackError", message).coded(code)
}

struct Repository {
    owner: String,
    name: String,
}

impl Repository {
    fn parse(origin: &str) -> Result<Self, Error> {
        let source = origin.strip_prefix("git@github.com:").map_or_else(
            || origin.to_owned(),
            |path| format!("ssh://git@github.com/{path}"),
        );
        let url = Url::parse(&source).map_err(|_| {
            error(
                "ROSTER_HUMAN_FEEDBACK_SCOPE",
                "Human feedback requires a GitHub repository origin",
            )
        })?;
        if url.host_str() != Some("github.com")
            || !matches!(url.scheme(), "https" | "ssh")
            || url.password().is_some()
            || url.port().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.scheme() == "https" && !url.username().is_empty()
            || url.scheme() == "ssh" && url.username() != "git"
        {
            return Err(error(
                "ROSTER_HUMAN_FEEDBACK_SCOPE",
                "Human feedback origin must identify github.com without credentials, ports or selectors",
            ));
        }
        let parts = url
            .path()
            .trim_start_matches('/')
            .split('/')
            .collect::<Vec<_>>();
        let [owner, repository] = parts.as_slice() else {
            return Err(error(
                "ROSTER_HUMAN_FEEDBACK_SCOPE",
                "Human feedback requires exactly one GitHub owner and repository",
            ));
        };
        let name = repository.strip_suffix(".git").unwrap_or(repository);
        if owner.is_empty()
            || owner.len() > 100
            || name.is_empty()
            || name.len() > 100
            || !owner
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            || matches!(name, "." | "..")
        {
            return Err(error(
                "ROSTER_HUMAN_FEEDBACK_SCOPE",
                "Human feedback repository scope is invalid",
            ));
        }
        Ok(Self {
            owner: (*owner).to_owned(),
            name: name.to_owned(),
        })
    }

    fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    fn url(&self, number: u64) -> String {
        format!(
            "https://github.com/{}/{}/pull/{number}",
            self.owner, self.name
        )
    }
}

struct Client {
    image: host::Image,
    environment: BTreeMap<String, String>,
}

impl Client {
    fn new(environment: &Value) -> Result<Self, Error> {
        let image = host::selected_path(environment, "ROSTER_GH")?.ok_or_else(|| {
            error(
                "ROSTER_HUMAN_FEEDBACK_HOST",
                "--post requires ROSTER_GH naming an absolute approved GitHub CLI image",
            )
        })?;
        if config::environment_value(environment, "GH_HOST")?
            .is_some_and(|host| host != &Text::from("github.com"))
        {
            return Err(error(
                "ROSTER_HUMAN_FEEDBACK_SCOPE",
                "Human feedback does not adopt an alternate GitHub host",
            ));
        }
        let image = host::Image::approve(&image)?;
        let mut selected = host::minimal_environment(environment)?;
        for name in [
            "HOME",
            "USERPROFILE",
            "LOCALAPPDATA",
            "APPDATA",
            "XDG_CONFIG_HOME",
            "GH_CONFIG_DIR",
        ] {
            if let Some(value) = config::environment_value(environment, name)? {
                let value = value.to_utf8()?;
                if value.contains('\0') {
                    return Err(error(
                        "ROSTER_HUMAN_FEEDBACK_HOST",
                        "Human GitHub environment values must not contain NUL",
                    ));
                }
                selected.insert(name.to_owned(), value);
            }
        }
        let mut token = match config::environment_value(environment, "GH_TOKEN")?
            .filter(|value| !value.0.is_empty())
        {
            Some(token) => Some(token),
            None => config::environment_value(environment, "GITHUB_TOKEN")?
                .filter(|value| !value.0.is_empty()),
        }
        .ok_or_else(|| error(
            "ROSTER_HUMAN_FEEDBACK_AUTH",
            "--post requires an explicit human GH_TOKEN or GITHUB_TOKEN; no operator authentication store was read",
        ))?
        .to_utf8()?;
        if token.is_empty()
            || token.len() > 8192
            || !token.bytes().all(|byte| (33..=126).contains(&byte))
        {
            token.zeroize();
            return Err(error(
                "ROSTER_HUMAN_FEEDBACK_AUTH",
                "The explicit human GitHub credential is invalid",
            ));
        }
        selected.insert("GH_TOKEN".to_owned(), token);
        selected.insert("GH_HOST".to_owned(), "github.com".to_owned());
        selected.insert("GH_PROMPT_DISABLED".to_owned(), "1".to_owned());
        selected.insert("GH_NO_UPDATE_NOTIFIER".to_owned(), "1".to_owned());
        selected.insert("GH_PAGER".to_owned(), String::new());
        Ok(Self {
            image,
            environment: selected,
        })
    }

    async fn request(
        &self,
        root: &Path,
        endpoint: &str,
        input: Option<Vec<u8>>,
        uncertain: &Cell<bool>,
        token: &CancellationToken,
    ) -> Result<Value, Error> {
        token.check()?;
        self.image.require_current()?;
        let mut command = Command::new(self.image.path());
        command
            .current_dir(root)
            .env_clear()
            .envs(&self.environment)
            .args(["api", "--hostname", "github.com", "--method"])
            .arg(if input.is_some() { "POST" } else { "GET" })
            .arg(endpoint);
        let input = if let Some(input) = input {
            command.args(["--input", "-"]);
            Some(owned::FiniteInput::try_from(input)?)
        } else {
            None
        };
        let observed = owned::execute_with_input(
            command,
            Limits {
                timeout: Some(Duration::from_secs(30)),
                max_output_bytes: MAX_RESPONSE,
            },
            Some(token),
            None,
            input,
        )
        .await;
        let output = observed.map_err(|failure| {
            if !failure.cleanup_complete {
                uncertain.set(true);
            }
            let mut safe = error(
                failure.code().unwrap_or("ROSTER_HUMAN_FEEDBACK_PROVIDER"),
                "The human GitHub provider did not complete; private command output was omitted",
            );
            if let Some(cause) = failure.source {
                safe.cause = Some(Box::new(Error::new(
                    "IoError",
                    format!("OS error class: {:?}", cause.kind()),
                )));
            }
            safe
        })?;
        token.check()?;
        self.image.require_current()?;
        if !output.status.success() {
            return Err(error(
                "ROSTER_HUMAN_FEEDBACK_PROVIDER",
                "The human GitHub provider refused the request; private command output was omitted",
            ));
        }
        let source = std::str::from_utf8(&output.captured.stdout).map_err(|_| {
            error(
                "ROSTER_HUMAN_FEEDBACK_INPUT",
                "The human GitHub provider must return bounded UTF-8 JSON",
            )
        })?;
        json::parse(&Text::from(source)).map_err(|_| {
            error(
                "ROSTER_HUMAN_FEEDBACK_INPUT",
                "The human GitHub provider returned malformed JSON",
            )
        })
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Some(token) = self.environment.get_mut("GH_TOKEN") {
            token.zeroize();
        }
    }
}

pub(super) struct Prepared {
    client: Client,
    repository: Repository,
    git: start::Git,
    number: u64,
    body: String,
}

pub(super) enum Plan {
    Local,
    NoSha,
    NoPullRequest,
    Ready(Box<Prepared>),
}

pub enum Status {
    Local,
    NoSha,
    NoPullRequest,
    Posted(String),
}

impl Status {
    pub(super) fn format(&self) -> String {
        match self {
            Self::Local => {
                "Local only; no GitHub feedback was posted and no policy was changed.\n".to_owned()
            }
            Self::NoSha => {
                "No published SHA is known; GitHub feedback was not posted.\n".to_owned()
            }
            Self::NoPullRequest => {
                "No matching pull request was found; GitHub feedback was not posted.\n".to_owned()
            }
            Self::Posted(url) => format!("Human GitHub feedback posted: {url}\n"),
        }
    }
}

pub(super) async fn prepare(
    paths: &paths::RepositoryPaths,
    environment: &Value,
    record: &Value,
    uncertain: &Cell<bool>,
    token: &CancellationToken,
) -> Result<Plan, Error> {
    let Some(sha) = record.get("sha").and_then(Value::as_string) else {
        return Ok(Plan::NoSha);
    };
    let sha = sha.to_utf8()?;
    if !crate::learning::valid_sha(&sha) {
        return Err(error(
            "ROSTER_HUMAN_FEEDBACK_INPUT",
            "Human feedback requires a full published commit SHA",
        ));
    }
    let client = Client::new(environment)?;
    let git_image = host::selected_path(environment, "ROSTER_GIT")?.ok_or_else(|| {
        error(
            "ROSTER_HUMAN_FEEDBACK_HOST",
            "Human feedback requires an absolute approved Git image",
        )
    })?;
    let git = start::Git::new(
        tools::Host {
            git: git_image,
            environment: host::minimal_environment(environment)?,
            ..tools::Host::default()
        },
        paths.git_dir.join("roster-human-feedback-disabled-hooks"),
    )?;
    let origin = git
        .text(
            &paths.repository,
            &["config", "--get", "remote.origin.url"],
            token,
        )
        .await?;
    let repository = Repository::parse(origin.trim())?;
    let user = client
        .request(&paths.repository, "user", None, uncertain, token)
        .await?;
    if !domain::is_text(user.get("type"), "User")
        || domain::safe_integer(user.get("id")).is_none_or(|id| id == 0)
        || user
            .get("login")
            .and_then(Value::as_string)
            .is_none_or(|login| {
                login.0.is_empty()
                    || login.0.len() > 39
                    || !login
                        .0
                        .iter()
                        .all(|unit| matches!(*unit, 45 | 48..=57 | 65..=90 | 97..=122))
            })
    {
        return Err(error(
            "ROSTER_EVALUATION_HUMAN",
            "GitHub feedback requires an authenticated human User principal, never an App or bot",
        ));
    }
    let endpoint = format!(
        "repos/{}/commits/{sha}/pulls?per_page=100",
        repository.slug()
    );
    let response = client
        .request(&paths.repository, &endpoint, None, uncertain, token)
        .await?;
    let candidates = domain::items(&response).ok_or_else(|| {
        error(
            "ROSTER_HUMAN_FEEDBACK_INPUT",
            "Commit-associated pull requests must be a JSON array",
        )
    })?;
    if candidates.len() >= 100 {
        return Err(error(
            "ROSTER_HUMAN_FEEDBACK_INPUT",
            "Pull request association is incomplete or exceeds its bound; no feedback was posted",
        ));
    }
    let mut numbers = std::collections::BTreeSet::new();
    for candidate in candidates {
        let number = domain::safe_integer(candidate.get("number"))
            .filter(|number| *number > 0)
            .ok_or_else(|| {
                error(
                    "ROSTER_HUMAN_FEEDBACK_INPUT",
                    "Pull request number is invalid",
                )
            })?;
        let scope = candidate
            .get("base")
            .and_then(|base| base.get("repo"))
            .and_then(|repo| repo.get("full_name"))
            .and_then(Value::as_string)
            .ok_or_else(|| {
                error(
                    "ROSTER_HUMAN_FEEDBACK_SCOPE",
                    "Pull request repository scope is missing",
                )
            })?;
        if !scope.to_utf8()?.eq_ignore_ascii_case(&repository.slug()) {
            return Err(error(
                "ROSTER_HUMAN_FEEDBACK_SCOPE",
                "Pull request association belongs to another repository",
            ));
        }
        numbers.insert(number);
    }
    if numbers.len() > 1 {
        return Err(error(
            "ROSTER_HUMAN_FEEDBACK_AMBIGUOUS",
            "Multiple pull requests match this commit; use local evaluation and resolve the ambiguity before posting",
        ));
    }
    let Some(number) = numbers.first().copied() else {
        return Ok(Plan::NoPullRequest);
    };
    let verdict = record
        .get("verdict")
        .and_then(Value::as_string)
        .ok_or_else(|| {
            error(
                "ROSTER_HUMAN_FEEDBACK_INPUT",
                "Human feedback verdict is missing",
            )
        })?
        .to_utf8()?;
    let difficulty = domain::safe_integer(record.get("difficulty")).ok_or_else(|| {
        error(
            "ROSTER_HUMAN_FEEDBACK_INPUT",
            "Human feedback difficulty is missing",
        )
    })?;
    let again = match record.get("again") {
        Some(Value::Boolean(true)) => "y",
        Some(Value::Boolean(false)) => "n",
        _ => {
            return Err(error(
                "ROSTER_HUMAN_FEEDBACK_INPUT",
                "Human feedback repeat preference is missing",
            ));
        }
    };
    let mut body = format!("AI-Eval: 1|{verdict}|{difficulty}|{again}");
    if let Some(minutes) = record.get("minutes").filter(|value| **value != Value::Null) {
        let minutes = domain::safe_integer(Some(minutes)).ok_or_else(|| {
            error(
                "ROSTER_HUMAN_FEEDBACK_INPUT",
                "Human minutes must be a reported safe integer",
            )
        })?;
        body.push_str(&format!("\nMinutes: {minutes}"));
    }
    Ok(Plan::Ready(Box::new(Prepared {
        client,
        repository,
        git,
        number,
        body,
    })))
}

impl Plan {
    pub(super) async fn execute(
        self,
        root: &Path,
        uncertain: &Cell<bool>,
        token: &CancellationToken,
    ) -> Result<Status, Error> {
        match self {
            Self::Local => Ok(Status::Local),
            Self::NoSha => Ok(Status::NoSha),
            Self::NoPullRequest => Ok(Status::NoPullRequest),
            Self::Ready(prepared) => {
                let Prepared {
                    client,
                    repository,
                    git,
                    number,
                    body,
                } = *prepared;
                let origin = git
                    .text(root, &["config", "--get", "remote.origin.url"], token)
                    .await?;
                if !Repository::parse(origin.trim())?
                    .slug()
                    .eq_ignore_ascii_case(&repository.slug())
                {
                    return Err(error(
                        "ROSTER_HUMAN_FEEDBACK_SCOPE",
                        "The repository origin changed before feedback; no comment was authorized",
                    ));
                }
                let endpoint = format!("repos/{}/issues/{number}/comments", repository.slug());
                let payload = Value::object(vec![("body", Value::string(&body))])
                    .json()
                    .into_bytes();
                let response = client
                    .request(root, &endpoint, Some(payload), uncertain, token)
                    .await?;
                if response.get("body") != Some(&Value::string(&body))
                    || domain::safe_integer(response.get("id")).is_none_or(|id| id == 0)
                {
                    return Err(error(
                        "ROSTER_HUMAN_FEEDBACK_UNKNOWN",
                        "GitHub feedback acknowledgement was invalid; inspect the pull request before retrying",
                    ));
                }
                token.check()?;
                Ok(Status::Posted(repository.url(number)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_origin_scope_accepts_only_credential_free_exact_repositories() -> Result<(), Error> {
        for source in [
            "https://github.com/Owner/Repository.git",
            "ssh://git@github.com/Owner/Repository.git",
            "git@github.com:Owner/Repository.git",
        ] {
            assert_eq!(Repository::parse(source)?.slug(), "Owner/Repository");
        }
        for source in [
            "http://github.com/owner/repo",
            "https://private-token@github.com/owner/repo",
            "https://user:private-token@github.com/owner/repo",
            "https://not-github.invalid/owner/repo",
            "ssh://other@github.com/owner/repo",
            "ssh://git@github.com:22/owner/repo",
            "https://github.com/owner/repo?selector=private",
            "https://github.com/owner/repo#private",
            "https://github.com/owner/repo/extra",
            "https://github.com/owner",
            "https://github.com/owner/%2frepo",
        ] {
            let failure = Repository::parse(source)
                .err()
                .ok_or_else(|| error("FIXTURE", "Unsafe origin scope was accepted"))?;
            assert!(!failure.message.contains("private-token"));
        }
        Ok(())
    }

    #[test]
    fn explicit_human_token_is_required_even_without_home_and_precedes_an_unselected_invalid_token()
    -> Result<(), Error> {
        let image =
            std::env::current_exe().map_err(|_| error("FIXTURE", "Missing owned test image"))?;
        let image = image
            .to_str()
            .ok_or_else(|| error("FIXTURE", "Test image must be Unicode"))?;
        let missing = Value::object(vec![("ROSTER_GH", Value::string(image))]);
        let failure = Client::new(&missing).err().ok_or_else(|| {
            error(
                "FIXTURE",
                "Unselected operator authentication store was adopted",
            )
        })?;
        assert_eq!(failure.code, Some("ROSTER_HUMAN_FEEDBACK_AUTH"));
        let selected = Value::object(vec![
            ("ROSTER_GH", Value::string(image)),
            ("GH_TOKEN", Value::string("test-only-explicit-human-token")),
            ("GITHUB_TOKEN", Value::Number(1.0)),
        ]);
        let client = Client::new(&selected)?;
        assert_eq!(
            client.environment.get("GH_TOKEN").map(String::as_str),
            Some("test-only-explicit-human-token")
        );
        assert!(!client.environment.contains_key("GITHUB_TOKEN"));
        Ok(())
    }
}
