use std::cell::RefCell;
use std::collections::VecDeque;
#[cfg(windows)]
use std::process::Command;
use std::rc::Rc;

use super::*;
use crate::cancel::{CancellationSource, Reason};
use crate::llm::http::Response;
#[cfg(windows)]
use crate::state::files;

const ORIGIN: &str = "https://github.com/owned/project.git";
#[cfg(windows)]
const SDK: &str = r#"
import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
if (process.argv[1] !== undefined) throw new Error("SDK import executed as CLI");
function mode() {
  return JSON.parse(readFileSync(resolve(process.cwd(), "..", "key.json"), "utf8"));
}
export function parseOrigin(origin) {
  if (origin !== "https://github.com/owned/project.git") throw new Error("origin");
  const selected = mode();
  if (selected.kind === "grow-key") {
    writeFileSync(resolve(process.cwd(), "..", "key.json"), JSON.stringify(selected) + " ".repeat(65536));
  }
  return { owner: "owned", repository: selected.kind === "wrong-scope" ? "foreign" : "project" };
}
export async function mintInstallationToken(options) {
  const selected = JSON.parse(options.privateKey.toString("utf8"));
  if (process.env.GH_TOKEN || process.env.GITHUB_TOKEN || process.env.NODE_OPTIONS) throw new Error("ambient auth");
  if (options.appId !== "123" || options.additionalPermissions.issues !== "read") throw new Error("permission scope");
  writeFileSync(selected.record, JSON.stringify({
    appId: options.appId,
    owner: options.owner,
    repository: options.repository,
    permissions: options.additionalPermissions
  }));
  if (selected.kind === "fail") throw new Error("test-only-app-token must not escape");
  if (selected.kind === "hold") {
    writeFileSync(selected.record + ".ready", String(process.pid));
    for (;;) await new Promise(resolve => setTimeout(resolve, 100));
  }
  return { token: "test-only-app-token", appSlug: "owned-app" };
}
"#;

#[cfg(windows)]
fn io(cause: std::io::Error) -> Error {
    Error::new("GitHubFixtureError", cause.to_string())
}

struct Bytes(Option<Vec<u8>>);

impl Body for Bytes {
    async fn read(&mut self) -> Result<Option<Vec<u8>>, Error> {
        Ok(self.0.take())
    }
}

struct Mock {
    responses: VecDeque<Response<Bytes>>,
    calls: Rc<RefCell<Vec<(String, String)>>>,
    hold_get: bool,
    ready: Rc<tokio::sync::Notify>,
    hold_delete: bool,
    delete_ready: Rc<tokio::sync::Notify>,
    delete_release: Rc<tokio::sync::Notify>,
    on_get: Option<Box<dyn FnMut() -> Result<(), Error>>>,
}

impl Mock {
    fn new(issue: &str) -> Self {
        Self {
            responses: VecDeque::from([
                Response {
                    status: 200,
                    headers: Vec::new(),
                    body: Some(Bytes(Some(issue.as_bytes().to_vec()))),
                },
                Response {
                    status: 204,
                    headers: Vec::new(),
                    body: None,
                },
            ]),
            calls: Rc::default(),
            hold_get: false,
            ready: Rc::default(),
            hold_delete: false,
            delete_ready: Rc::default(),
            delete_release: Rc::default(),
            on_get: None,
        }
    }
}

impl Transport for Mock {
    type Body = Bytes;

    async fn send(&mut self, request: &Request) -> Result<Response<Bytes>, crate::llm::Failure> {
        assert_eq!(
            request
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("Authorization"))
                .map(|(_, value)| value.as_str()),
            Some("Bearer test-only-app-token")
        );
        assert!(request.body.is_empty());
        self.calls
            .borrow_mut()
            .push((request.method.clone(), request.url.clone()));
        if request.method == "GET" {
            if let Some(action) = self.on_get.as_mut() {
                action()?;
            }
        }
        if self.hold_get && request.method == "GET" {
            self.ready.notify_one();
            std::future::pending::<()>().await;
        }
        if self.hold_delete && request.method == "DELETE" {
            self.delete_ready.notify_one();
            self.delete_release.notified().await;
        }
        self.responses
            .pop_front()
            .ok_or_else(|| Error::type_error("Unexpected additional App HTTP request").into())
    }
}

fn response() -> String {
    "{\"number\":42,\"title\":\"Owned issue\",\"body\":\"Owned ask\",\"state\":\"open\",\"html_url\":\"https://github.com/owned/project/issues/42\"}".to_owned()
}

fn access_fixture() -> Result<Access, Error> {
    access(
        b"{\"token\":\"test-only-app-token\",\"appSlug\":\"owned-app\",\"repository\":\"owned/project\"}".to_vec(),
        &identity::identity_hash(&Text::from(ORIGIN))?,
    )
}

#[test]
fn issue_projection_validates_scope_and_never_returns_the_installation_token() -> Result<(), Error>
{
    let access = access_fixture()?;
    let scope = access.scope.as_ref().map_err(Clone::clone)?;
    let source = response()
        .replace("Owned issue", "title test-only-app-token")
        .replace("Owned ask", "body test-only-app-token");
    let observed = issue(source.as_bytes(), 42, &access, scope)?;
    assert_eq!(observed.number(), 42);
    assert_eq!(
        observed.metadata().get("app"),
        Some(&Value::string("owned-app"))
    );
    assert!(!observed.metadata().json().contains("test-only-app-token"));
    assert!(!observed.body().to_utf8()?.contains("test-only-app-token"));
    for source in [
        response().replace("\"number\":42", "\"number\":43"),
        response().replace("owned/project/issues/42", "foreign/project/issues/42"),
        response().replace("\"state\":\"open\"", "\"state\":\"unknown\""),
        response().replace("\"Owned issue\"", "\" \\t \""),
        response().replace("\"body\":\"Owned ask\",", ""),
        response().replacen('{', "{\"pull_request\":null,", 1),
        "not-json test-only-app-token".to_owned(),
    ] {
        let failure = issue(source.as_bytes(), 42, &access, scope)
            .err()
            .ok_or_else(|| Error::type_error("Invalid issue was accepted"))?;
        assert!(!failure.to_string().contains("test-only-app-token"));
    }
    Ok(())
}

#[test]
fn valid_sdk_tokens_survive_invalid_metadata_only_for_mandatory_revocation() -> Result<(), Error> {
    let expected = identity::identity_hash(&Text::from(ORIGIN))?;
    for source in [
        "{\"token\":\"test-only-app-token\",\"repository\":\"owned/project\"}",
        "{\"token\":\"test-only-app-token\",\"appSlug\":\"bad_slug\",\"repository\":\"owned/project\"}",
        "{\"token\":\"test-only-app-token\",\"appSlug\":\"owned-app\",\"repository\":\"owned/foreign\"}",
    ] {
        let access = access(source.as_bytes().to_vec(), &expected)?;
        assert_eq!(access.token.as_str(), "test-only-app-token");
        assert!(access.scope.is_err());
    }
    assert!(access(b"{\"token\":\"unsafe token\"}".to_vec(), &expected).is_err());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn app_intake_refuses_precancellation_and_missing_credentials_before_repository_access()
-> Result<(), Error> {
    let mut transport = Mock::new(&response());
    let cancellation = CancellationSource::default();
    cancellation.cancel(Reason::Cancelled)?;
    let failure = read_issue(
        &mut transport,
        Path::new("missing-owned-installation"),
        Path::new("missing-owned-root"),
        &Value::object(vec![("GH_TOKEN", Value::string("not-an-app-token"))]),
        42,
        &cancellation.token(),
    )
    .await
    .err()
    .ok_or_else(|| Error::type_error("Cancelled App intake succeeded"))?;
    assert_eq!(failure.code, Some("ROSTER_CANCELLED"));
    let failure = read_issue(
        &mut transport,
        Path::new("missing-owned-installation"),
        Path::new("missing-owned-root"),
        &Value::object(vec![
            ("GITHUB_APP_ID", Value::string("123")),
            ("GH_TOKEN", Value::string("not-an-app-token")),
        ]),
        42,
        &CancellationSource::default().token(),
    )
    .await
    .err()
    .ok_or_else(|| Error::type_error("Missing App key fell back to human auth"))?;
    assert_eq!(failure.code, Some("ROSTER_GITHUB_APP"));
    assert!(!failure.to_string().contains("not-an-app-token"));
    assert!(transport.calls.borrow().is_empty());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn app_http_byte_limit_is_exact_and_revocation_requires_an_empty_acknowledgement()
-> Result<(), Error> {
    let access = access_fixture()?;
    let cancellation = CancellationSource::default();
    for extra in [0, 1] {
        let mut transport = Mock::new("");
        let first = transport
            .responses
            .front_mut()
            .ok_or_else(|| Error::type_error("Missing owned HTTP response"))?;
        first.body = Some(Bytes(Some(vec![b'a'; MAX_RESPONSE + extra])));
        let result = api(
            &mut transport,
            &access,
            "repos/owned/project/issues/42",
            "GET",
            200,
            Some(&cancellation.token()),
            Instant::now() + REQUEST_TIMEOUT,
        )
        .await;
        if extra == 0 {
            assert_eq!(result?.len(), MAX_RESPONSE);
        } else {
            assert!(result.is_err());
        }
    }
    let mut transport = Mock::new("");
    transport.responses.clear();
    transport.responses.push_back(Response {
        status: 204,
        headers: Vec::new(),
        body: Some(Bytes(Some(b"unexpected".to_vec()))),
    });
    assert!(
        api(
            &mut transport,
            &access,
            "installation/token",
            "DELETE",
            204,
            None,
            Instant::now() + REVOCATION_TIMEOUT,
        )
        .await
        .is_err()
    );
    Ok(())
}

#[cfg(windows)]
struct Fixture {
    container: PathBuf,
    root: PathBuf,
    installation: PathBuf,
    key: PathBuf,
    git: PathBuf,
    node: PathBuf,
    cleaned: bool,
}

#[cfg(windows)]
impl Fixture {
    fn new(kind: &str) -> Result<Self, Error> {
        let container = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(".roster")
            .join(format!(
                "github-app-tests-{}-{}",
                std::process::id(),
                files::nonce()?
            ));
        let git = std::env::var_os("ROSTER_TEST_GIT")
            .map(PathBuf::from)
            .ok_or_else(|| Error::type_error("App fixtures require explicit ROSTER_TEST_GIT"))?;
        let node = std::env::var_os("ROSTER_TEST_NODE")
            .map(PathBuf::from)
            .ok_or_else(|| {
                Error::type_error("App SDK fixtures require explicit ROSTER_TEST_NODE")
            })?;
        host::Image::approve(&git)?;
        host::Image::approve(&node)?;
        let fixture = Self {
            root: container.join("project"),
            installation: container.join("installation"),
            key: container.join("key.json"),
            container,
            git,
            node,
            cleaned: false,
        };
        files::create_private_directories(&fixture.root)?;
        files::create_private_directories(&fixture.installation)?;
        fixture.git(&fixture.root, &["init", "--quiet"])?;
        fixture.git(&fixture.root, &["remote", "add", "origin", ORIGIN])?;
        let sdk = fixture
            .installation
            .join("vendor")
            .join("github-agent-contracts");
        files::create_private_directories(&sdk.join("scripts"))?;
        fs::write(sdk.join("scripts").join("agent-pr.mjs"), SDK).map_err(io)?;
        fixture.git(&sdk, &["init", "--quiet"])?;
        fixture.git(&sdk, &["add", "--", "scripts"])?;
        fixture.git(
            &sdk,
            &["commit", "--quiet", "-m", "Owned synthetic SDK interface"],
        )?;
        let commit = fixture.git(&sdk, &["rev-parse", "HEAD"])?;
        fixture.git(&fixture.installation, &["init", "--quiet"])?;
        fixture.git(
            &fixture.installation,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                "160000",
                &commit,
                "vendor/github-agent-contracts",
            ],
        )?;
        fixture.git(
            &fixture.installation,
            &["commit", "--quiet", "-m", "Owned reviewed SDK pin"],
        )?;
        fs::write(
            &fixture.key,
            Value::object(vec![
                ("kind", Value::string(kind)),
                (
                    "record",
                    Value::string(
                        fixture
                            .container
                            .join("sdk-call.json")
                            .to_str()
                            .ok_or_else(|| Error::type_error("Owned trace path must be Unicode"))?,
                    ),
                ),
            ])
            .json(),
        )
        .map_err(io)?;
        eprintln!(
            "GITHUB_APP_FIXTURE pid={} root={}",
            std::process::id(),
            fixture.container.display()
        );
        Ok(fixture)
    }

    fn git(&self, cwd: &Path, args: &[&str]) -> Result<String, Error> {
        let output = Command::new(&self.git)
            .current_dir(cwd)
            .env_clear()
            .envs(tools::Host::default().environment)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "NUL")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .args([
                "--no-pager",
                "-c",
                "init.templateDir=",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=Owned App Fixture",
                "-c",
                "user.email=owned@example.invalid",
                "-c",
                "protocol.allow=never",
                "-c",
            ])
            .arg(format!(
                "core.hooksPath={}",
                self.container.join("disabled-hooks").display()
            ))
            .args(args)
            .output()
            .map_err(io)?;
        if !output.status.success() {
            return Err(Error::new(
                "GitHubFixtureError",
                "Owned Git preparation failed",
            ));
        }
        String::from_utf8(output.stdout)
            .map(|value| value.trim_end_matches(['\r', '\n']).to_owned())
            .map_err(|_| Error::type_error("Owned Git output must be UTF-8"))
    }

    fn environment(&self) -> Result<Value, Error> {
        let mut entries = tools::Host::default()
            .environment
            .into_iter()
            .map(|(name, value)| (Text::from(name.as_str()), Value::string(&value)))
            .collect::<Vec<_>>();
        for (name, value) in [
            (
                "ROSTER_GIT",
                self.git
                    .to_str()
                    .ok_or_else(|| Error::type_error("Git path must be Unicode"))?,
            ),
            (
                "ROSTER_NODE20",
                self.node
                    .to_str()
                    .ok_or_else(|| Error::type_error("Node path must be Unicode"))?,
            ),
            (
                "GITHUB_APP_PRIVATE_KEY_PATH",
                self.key
                    .to_str()
                    .ok_or_else(|| Error::type_error("Key path must be Unicode"))?,
            ),
            ("GITHUB_APP_ID", "123"),
            ("GH_TOKEN", "not-an-app-token"),
            ("GITHUB_TOKEN", "not-an-app-token"),
        ] {
            entries.push((Text::from(name), Value::string(value)));
        }
        Ok(Value::Object(entries))
    }

    fn delivery_source(&self) -> Result<(), Error> {
        files::create_private_directories(&self.root.join("docs"))?;
        files::create_private_directories(&self.root.join(".roster"))?;
        fs::write(self.root.join(".gitignore"), ".roster/\n.worktrees/\nTASK.md\nRECIPE.yml\nPLAN.md\nESTIMATE.md\nRESULT.md\nREVIEW.md\nCONTEXT.md\nRESEARCH.md\nASSIGNMENT.md\n").map_err(io)?;
        fs::write(
            self.root.join("AGENTS.md"),
            "# Owned consumer\n\nPreserve the original ask and existing source. No publication.\n",
        )
        .map_err(io)?;
        fs::write(
            self.root.join("docs").join("guide.md"),
            "# Guide\n\nBefore.\n",
        )
        .map_err(io)?;
        fs::write(
            self.root.join(".roster").join("config.yml"),
            config::format(config::defaults()?.as_value())?.to_utf8()?,
        )
        .map_err(io)?;
        for name in [
            "docs",
            "implement-task",
            "run-tests",
            "read-before-write",
            "small-diff",
            "result-report",
            "review",
        ] {
            let directory = self.installation.join("skills").join(name);
            files::create_private_directories(&directory)?;
            fs::write(
                directory.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: Use when verifying an owned bounded outcome.\n---\n\n# Owned packet\n\nRead named files and preserve the original ask. Never claim tests, publication or human acceptance without actual evidence.\n"),
            ).map_err(io)?;
        }
        files::create_private_directories(&self.installation.join("principals"))?;
        for name in ["coder", "reviewer"] {
            fs::write(
                self.installation.join("principals").join(format!("{name}.md")),
                "# Owned principal\n\nUse only planned owned scope and report actual evidence. No publication or invented acceptance.\n",
            ).map_err(io)?;
        }
        self.git(
            &self.root,
            &["add", "--", ".gitignore", "AGENTS.md", "docs"],
        )?;
        self.git(
            &self.root,
            &["commit", "--quiet", "-m", "Owned issue-delivery source"],
        )?;
        Ok(())
    }

    fn finish(mut self) -> Result<(), Error> {
        fs::remove_dir_all(&self.container).map_err(io)?;
        self.cleaned = true;
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for Fixture {
    fn drop(&mut self) {
        if !self.cleaned {
            eprintln!(
                "GITHUB_APP_FIXTURE_RETAINED pid={} root={}",
                std::process::id(),
                self.container.display()
            );
        }
    }
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn owned_sdk_identity_bridge_and_native_issue_read_revoke_before_returning_metadata()
-> Result<(), Error> {
    let fixture = Fixture::new("good")?;
    let mut transport = Mock::new(&response());
    let observed = read_issue(
        &mut transport,
        &fixture.installation,
        &fixture.root,
        &fixture.environment()?,
        42,
        &CancellationSource::default().token(),
    )
    .await?;
    assert_eq!(observed.number(), 42);
    assert_eq!(
        observed.metadata().get("state"),
        Some(&Value::string("OPEN"))
    );
    assert_eq!(
        transport.calls.borrow().as_slice(),
        [
            (
                "GET".to_owned(),
                "https://api.github.com/repos/owned/project/issues/42".to_owned()
            ),
            (
                "DELETE".to_owned(),
                "https://api.github.com/installation/token".to_owned()
            ),
        ]
    );
    let source = fs::read_to_string(fixture.container.join("sdk-call.json")).map_err(io)?;
    let captured = json::parse(&Text::from(source.as_str()))?;
    assert_eq!(
        captured.get("permissions"),
        Some(&Value::object(vec![("issues", Value::string("read"))]))
    );
    assert!(!source.contains("not-an-app-token"));
    fixture.finish()
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn wrong_sdk_scope_revokes_the_known_token_without_reading_an_issue() -> Result<(), Error> {
    let fixture = Fixture::new("wrong-scope")?;
    let mut transport = Mock::new(&response());
    transport.responses.pop_front();
    let failure = read_issue(
        &mut transport,
        &fixture.installation,
        &fixture.root,
        &fixture.environment()?,
        42,
        &CancellationSource::default().token(),
    )
    .await
    .err()
    .ok_or_else(|| Error::type_error("Foreign SDK scope was accepted"))?;
    assert_eq!(failure.code, Some("ROSTER_GITHUB_PROTOCOL"));
    assert_eq!(transport.calls.borrow().len(), 1);
    assert_eq!(
        transport
            .calls
            .borrow()
            .first()
            .map(|(method, _)| method.as_str()),
        Some("DELETE")
    );
    fixture.finish()
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn sdk_failure_never_uses_an_ambient_human_token_or_echoes_provider_output()
-> Result<(), Error> {
    let fixture = Fixture::new("fail")?;
    let mut transport = Mock::new(&response());
    let failure = read_issue(
        &mut transport,
        &fixture.installation,
        &fixture.root,
        &fixture.environment()?,
        42,
        &CancellationSource::default().token(),
    )
    .await
    .err()
    .ok_or_else(|| Error::type_error("Failed SDK auth produced an issue"))?;
    assert_eq!(failure.code, Some("ROSTER_GITHUB_APP"));
    assert!(!failure.to_string().contains("test-only-app-token"));
    assert!(!failure.to_string().contains("not-an-app-token"));
    assert!(transport.calls.borrow().is_empty());
    fixture.finish()
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn cancelled_native_issue_read_still_revokes_and_preserves_the_first_reason()
-> Result<(), Error> {
    let fixture = Fixture::new("good")?;
    let mut transport = Mock::new(&response());
    transport.hold_get = true;
    transport.responses.pop_front();
    let ready = transport.ready.clone();
    let calls = transport.calls.clone();
    let cancellation = CancellationSource::default();
    let environment = fixture.environment()?;
    let token = cancellation.token();
    let mut operation = Box::pin(read_issue(
        &mut transport,
        &fixture.installation,
        &fixture.root,
        &environment,
        42,
        &token,
    ));
    tokio::select! {
        result = &mut operation => return Err(result.err().unwrap_or_else(|| Error::type_error("Issue read ended before its readiness barrier"))),
        () = ready.notified() => {},
    }
    cancellation.cancel(Reason::TaskTimeout(25))?;
    let failure = operation
        .await
        .err()
        .ok_or_else(|| Error::type_error("Cancelled issue read succeeded"))?;
    assert_eq!(failure.code, Some("ROSTER_TASK_TIMEOUT"));
    assert_eq!(calls.borrow().len(), 2);
    assert_eq!(
        calls.borrow().last().map(|(method, _)| method.as_str()),
        Some("DELETE")
    );
    fixture.finish()
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn online_status_composes_checked_app_and_local_observations_without_guessing_pr_state()
-> Result<(), Error> {
    let fixture = Fixture::new("good")?;
    let mut transport = Mock::new(&response());
    let observed = crate::status::read(
        &mut transport,
        &fixture.installation,
        &fixture.root,
        &fixture.environment()?,
        &crate::status::Options {
            issue: 42,
            offline: false,
            json: true,
        },
        &CancellationSource::default().token(),
    )
    .await?;
    assert_eq!(
        observed.as_value().get("log"),
        Some(&Value::string("missing"))
    );
    assert_eq!(observed.as_value().get("seat"), Some(&Value::Null));
    let issue = observed
        .as_value()
        .get("github_issue")
        .ok_or_else(|| Error::type_error("Missing actual App issue observation"))?;
    assert_eq!(issue.get("state"), Some(&Value::string("OPEN")));
    assert_eq!(issue.get("number"), Some(&Value::Number(42.0)));
    assert!(issue.get("body").is_none());
    assert!(!observed.format(true)?.contains("test-only-app-token"));
    let text = observed.format(false)?;
    assert!(text.contains(
        "GitHub issue: #42 Owned issue (OPEN) https://github.com/owned/project/issues/42"
    ));
    assert!(text.contains("Open PR: unknown (not queried)"));
    assert_eq!(transport.calls.borrow().len(), 2);
    fixture.finish()
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn offline_status_never_initializes_the_sdk_or_sends_a_github_request() -> Result<(), Error> {
    let fixture = Fixture::new("fail")?;
    let mut transport = Mock::new(&response());
    let observed = crate::status::read(
        &mut transport,
        &fixture.installation,
        &fixture.root,
        &fixture.environment()?,
        &crate::status::Options {
            issue: 42,
            offline: true,
            json: true,
        },
        &CancellationSource::default().token(),
    )
    .await?;
    assert!(observed.as_value().get("github_issue").is_none());
    assert!(transport.calls.borrow().is_empty());
    assert!(!fixture.container.join("sdk-call.json").exists());
    fixture.finish()
}

#[tokio::test(flavor = "current_thread")]
async fn local_only_status_api_refuses_online_options_before_any_scope_or_auth_access()
-> Result<(), Error> {
    let result = crate::status::observe(
        Path::new("missing-owned-root"),
        &Value::object(vec![("GITHUB_APP_ID", Value::string("123"))]),
        &crate::status::Options {
            issue: 42,
            offline: false,
            json: false,
        },
        &CancellationSource::default().token(),
    )
    .await;
    let failure = result
        .err()
        .ok_or_else(|| Error::type_error("Local-only status adopted online mode"))?;
    assert_eq!(failure.code, Some("ROSTER_STATUS_INPUT"));
    assert!(failure.message.contains("no authentication was attempted"));
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn failed_issue_responses_are_revoked_and_failed_revocation_never_returns_metadata()
-> Result<(), Error> {
    let fixture = Fixture::new("good")?;
    for revocation_fails in [false, true] {
        let mut transport = Mock::new(&response());
        if revocation_fails {
            transport
                .responses
                .back_mut()
                .ok_or_else(|| Error::type_error("Missing revocation response"))?
                .status = 500;
        } else {
            transport
                .responses
                .front_mut()
                .ok_or_else(|| Error::type_error("Missing issue response"))?
                .status = 404;
        }
        let failure = read_issue(
            &mut transport,
            &fixture.installation,
            &fixture.root,
            &fixture.environment()?,
            42,
            &CancellationSource::default().token(),
        )
        .await
        .err()
        .ok_or_else(|| Error::type_error("Unacknowledged App operation returned an issue"))?;
        assert_eq!(
            failure.code,
            Some(if revocation_fails {
                "ROSTER_GITHUB_REVOCATION"
            } else {
                "ROSTER_GITHUB_HTTP"
            })
        );
        assert_eq!(transport.calls.borrow().len(), 2);
        assert!(!failure.to_string().contains("test-only-app-token"));
        assert!(!failure.to_string().contains("not-an-app-token"));
    }
    fixture.finish()
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn origin_drift_during_issue_response_revokes_and_refuses_the_old_scope() -> Result<(), Error>
{
    let fixture = Fixture::new("good")?;
    let config = fixture.root.join(".git").join("config");
    let mut transport = Mock::new(&response());
    transport.on_get = Some(Box::new(move || {
        let source = fs::read_to_string(&config).map_err(io)?;
        fs::write(
            &config,
            source.replace(ORIGIN, "https://github.com/owned/foreign.git"),
        )
        .map_err(io)
    }));
    let failure = read_issue(
        &mut transport,
        &fixture.installation,
        &fixture.root,
        &fixture.environment()?,
        42,
        &CancellationSource::default().token(),
    )
    .await
    .err()
    .ok_or_else(|| Error::type_error("Origin drift returned a scoped issue"))?;
    assert_eq!(failure.code, Some("ROSTER_GITHUB_SCOPE"));
    assert_eq!(transport.calls.borrow().len(), 2);
    assert_eq!(
        transport
            .calls
            .borrow()
            .last()
            .map(|(method, _)| method.as_str()),
        Some("DELETE")
    );
    fixture.finish()
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn cancellation_during_failed_revocation_preserves_the_first_timeout_reason()
-> Result<(), Error> {
    let fixture = Fixture::new("good")?;
    let mut transport = Mock::new(&response());
    transport.hold_delete = true;
    transport
        .responses
        .back_mut()
        .ok_or_else(|| Error::type_error("Missing revocation response"))?
        .status = 500;
    let ready = transport.delete_ready.clone();
    let release = transport.delete_release.clone();
    let cancellation = CancellationSource::default();
    let token = cancellation.token();
    let environment = fixture.environment()?;
    let mut operation = Box::pin(read_issue(
        &mut transport,
        &fixture.installation,
        &fixture.root,
        &environment,
        42,
        &token,
    ));
    tokio::select! {
        result = &mut operation => return Err(result.err().unwrap_or_else(|| Error::type_error("Revocation ended before readiness"))),
        () = ready.notified() => {},
    }
    cancellation.cancel(Reason::TaskTimeout(25))?;
    release.notify_one();
    let failure = operation
        .await
        .err()
        .ok_or_else(|| Error::type_error("Cancelled failed revocation returned an issue"))?;
    assert_eq!(failure.code, Some("ROSTER_TASK_TIMEOUT"));
    assert_eq!(
        failure.cause.as_ref().and_then(|cause| cause.code),
        Some("ROSTER_GITHUB_REVOCATION")
    );
    fixture.finish()
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn sdk_key_actual_read_refuses_growth_after_the_native_metadata_check() -> Result<(), Error> {
    let fixture = Fixture::new("grow-key")?;
    let mut transport = Mock::new(&response());
    let failure = read_issue(
        &mut transport,
        &fixture.installation,
        &fixture.root,
        &fixture.environment()?,
        42,
        &CancellationSource::default().token(),
    )
    .await
    .err()
    .ok_or_else(|| Error::type_error("SDK accepted key growth past the actual read bound"))?;
    assert_eq!(failure.code, Some("ROSTER_GITHUB_APP"));
    assert!(transport.calls.borrow().is_empty());
    assert!(!fixture.container.join("sdk-call.json").exists());
    fixture.finish()
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn sdk_key_exact_file_limit_is_supported_and_one_more_byte_refuses_before_auth()
-> Result<(), Error> {
    for extra in [0, 1] {
        let fixture = Fixture::new("good")?;
        let mut source = fs::read(&fixture.key).map_err(io)?;
        source.resize(65_536 + extra, b' ');
        fs::write(&fixture.key, source).map_err(io)?;
        let mut transport = Mock::new(&response());
        let result = read_issue(
            &mut transport,
            &fixture.installation,
            &fixture.root,
            &fixture.environment()?,
            42,
            &CancellationSource::default().token(),
        )
        .await;
        if extra == 0 {
            assert_eq!(result?.number(), 42);
            assert_eq!(transport.calls.borrow().len(), 2);
        } else {
            let failure = result
                .err()
                .ok_or_else(|| Error::type_error("Oversized App key was accepted"))?;
            assert_eq!(failure.code, Some("ROSTER_GITHUB_APP"));
            assert!(transport.calls.borrow().is_empty());
            assert!(!fixture.container.join("sdk-call.json").exists());
        }
        fixture.finish()?;
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn cancellation_during_the_actual_sdk_child_keeps_its_original_timeout_message()
-> Result<(), Error> {
    let fixture = Fixture::new("hold")?;
    let mut transport = Mock::new(&response());
    let cancellation = CancellationSource::default();
    let token = cancellation.token();
    let environment = fixture.environment()?;
    let ready = fixture.container.join("sdk-call.json.ready");
    let barrier = async {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        loop {
            if ready.is_file() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Error::type_error(
                    "Owned SDK child did not reach its readiness barrier",
                ));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    let mut operation = Box::pin(read_issue(
        &mut transport,
        &fixture.installation,
        &fixture.root,
        &environment,
        42,
        &token,
    ));
    tokio::select! {
        result = &mut operation => return Err(result.err().unwrap_or_else(|| Error::type_error("SDK ended before readiness"))),
        result = barrier => result?,
    }
    cancellation.cancel(Reason::TaskTimeout(25))?;
    let failure = operation
        .await
        .err()
        .ok_or_else(|| Error::type_error("Cancelled SDK child produced an issue"))?;
    assert_eq!(failure.code, Some("ROSTER_TASK_TIMEOUT"));
    assert_eq!(failure.message, Reason::TaskTimeout(25).error().message);
    assert!(transport.calls.borrow().is_empty());
    fixture.finish()
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn completed_issue_context_rechecks_origin_and_reviewed_sdk_after_later_awaits()
-> Result<(), Error> {
    for drift in ["origin", "sdk"] {
        let fixture = Fixture::new("good")?;
        let mut transport = Mock::new(&response());
        let cancellation = CancellationSource::default();
        let token = cancellation.token();
        let observed = read_issue(
            &mut transport,
            &fixture.installation,
            &fixture.root,
            &fixture.environment()?,
            42,
            &token,
        )
        .await?;
        observed.require_current(&token).await?;
        if drift == "origin" {
            fixture.git(
                &fixture.root,
                &[
                    "remote",
                    "set-url",
                    "origin",
                    "https://github.com/owned/foreign.git",
                ],
            )?;
        } else {
            let source = fixture
                .installation
                .join("vendor")
                .join("github-agent-contracts")
                .join("scripts")
                .join("agent-pr.mjs");
            let mut bytes = fs::read(&source).map_err(io)?;
            bytes.push(b'\n');
            fs::write(source, bytes).map_err(io)?;
        }
        let failure = observed
            .require_current(&token)
            .await
            .err()
            .ok_or_else(|| Error::type_error("Stale issue context remained current"))?;
        assert!(!failure.to_string().contains("test-only-app-token"));
        assert!(
            !failure
                .to_string()
                .contains(&fixture.container.to_string_lossy().into_owned())
        );
        assert_eq!(transport.calls.borrow().len(), 2);
        fixture.finish()?;
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn review_keys_in_linked_source_roots_refuse_before_sdk_authentication() -> Result<(), Error>
{
    for location in ["consumer-worktree", "installation-canonical"] {
        let fixture = Fixture::new("good")?;
        let linked = fixture.container.join("linked-source");
        let mut environment = fixture.environment()?;
        let (cwd, installation, key) = if location == "consumer-worktree" {
            fixture.git(
                &fixture.root,
                &[
                    "commit",
                    "--quiet",
                    "--allow-empty",
                    "-m",
                    "Owned linked source base",
                ],
            )?;
            fixture.git(
                &fixture.root,
                &[
                    "worktree",
                    "add",
                    "--quiet",
                    "--detach",
                    linked
                        .to_str()
                        .ok_or_else(|| Error::type_error("Owned linked source must be Unicode"))?,
                    "HEAD",
                ],
            )?;
            (
                linked.clone(),
                fixture.installation.clone(),
                linked.join("owned-key.json"),
            )
        } else {
            fixture.git(
                &fixture.installation,
                &[
                    "worktree",
                    "add",
                    "--quiet",
                    "--detach",
                    linked.to_str().ok_or_else(|| {
                        Error::type_error("Owned linked installation must be Unicode")
                    })?,
                    "HEAD",
                ],
            )?;
            domain::put(
                &mut environment,
                "GITHUB_AGENT_CONTRACTS",
                Value::string(
                    fixture
                        .installation
                        .join("vendor")
                        .join("github-agent-contracts")
                        .to_str()
                        .ok_or_else(|| Error::type_error("Owned SDK fallback must be Unicode"))?,
                ),
            )?;
            (
                fixture.root.clone(),
                linked,
                fixture.installation.join("owned-key.json"),
            )
        };
        fs::copy(&fixture.key, &key).map_err(io)?;
        domain::put(
            &mut environment,
            "GITHUB_APP_PRIVATE_KEY_PATH",
            Value::string(
                key.to_str()
                    .ok_or_else(|| Error::type_error("Owned key path must be Unicode"))?,
            ),
        )?;
        let mut transport = Mock::new(&response());
        let failure = read_issue(
            &mut transport,
            &installation,
            &cwd,
            &environment,
            42,
            &CancellationSource::default().token(),
        )
        .await
        .err()
        .ok_or_else(|| Error::type_error("App key inside a linked source tree was accepted"))?;
        assert_eq!(failure.code, Some("ROSTER_GITHUB_APP"), "{location}");
        assert!(transport.calls.borrow().is_empty());
        assert!(!fixture.container.join("sdk-call.json").exists());
        fixture.finish()?;
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn review_cancelled_failed_read_wins_after_successful_token_revocation() -> Result<(), Error>
{
    let fixture = Fixture::new("good")?;
    let mut transport = Mock::new(&response());
    transport.hold_delete = true;
    transport
        .responses
        .front_mut()
        .ok_or_else(|| Error::type_error("Missing issue response"))?
        .status = 404;
    let ready = transport.delete_ready.clone();
    let release = transport.delete_release.clone();
    let cancellation = CancellationSource::default();
    let token = cancellation.token();
    let environment = fixture.environment()?;
    let mut operation = Box::pin(read_issue(
        &mut transport,
        &fixture.installation,
        &fixture.root,
        &environment,
        42,
        &token,
    ));
    tokio::select! {
        result = &mut operation => return Err(result.err().unwrap_or_else(|| Error::type_error("Revocation ended before readiness"))),
        () = ready.notified() => {},
    }
    cancellation.cancel(Reason::TaskTimeout(25))?;
    release.notify_one();
    let failure = operation
        .await
        .err()
        .ok_or_else(|| Error::type_error("Cancelled failed observation returned an issue"))?;
    assert_eq!(failure.code, Some("ROSTER_TASK_TIMEOUT"));
    assert_eq!(failure.message, Reason::TaskTimeout(25).error().message);
    assert_eq!(
        failure.cause.as_ref().and_then(|cause| cause.code),
        Some("ROSTER_GITHUB_HTTP")
    );
    fixture.finish()
}

#[cfg(windows)]
fn delivery_response(state: &str) -> String {
    Value::object(vec![
        ("number", Value::Number(42.0)),
        ("title", Value::string("docs: update owned guide")),
        ("body", Value::string("Document the owned guide outcome.\n\n## Files allowed\n- docs/guide.md\n\n## Acceptance checks\n- The guide documents the owned outcome.\n")),
        ("state", Value::string(state)),
        ("html_url", Value::string("https://github.com/owned/project/issues/42")),
    ]).json()
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn issue_delivery_creates_a_real_issue_branch_and_truthful_stub_in_one_owned_run()
-> Result<(), Error> {
    let fixture = Fixture::new("good")?;
    fixture.delivery_source()?;
    let environment = fixture.environment()?;
    let host = tools::Host {
        git: fixture.git.clone(),
        node20: Some(fixture.node.clone()),
        environment: host::minimal_environment(&environment)?,
        ..Default::default()
    };
    let mut transport = Mock::new(&delivery_response("open"));
    let calls = transport.calls.clone();
    let cancellation = CancellationSource::default();
    let token = cancellation.token();
    let input = crate::delivery::resolve_issue(
        &mut transport,
        &fixture.installation,
        &fixture.root,
        &environment,
        &host,
        42,
        &token,
    )
    .await?;
    let loaded = config::load(&fixture.installation, &fixture.root, &environment)?;
    let credentials = crate::llm::openai::Credentials::default();
    let admission = crate::llm::admission::Admission::default();
    let mut observer = crate::llm::control::NoObserver;
    let mut driver = crate::delivery::Driver {
        transport: &mut transport,
        observer: &mut observer,
        credentials: &credentials,
        admission: &admission,
    };
    let mut ownership = crate::delivery::Ownership::default();
    let outcome = driver
        .run_local(
            crate::delivery::Options {
                cwd: &fixture.root,
                installation: &fixture.installation,
                loaded: &loaded,
                input,
                host,
                approval: crate::execution::backend::Approval::default(),
                start: start::Options::resolve(
                    &loaded.config,
                    Some("current"),
                    Some(start::Sync::Offline),
                )?,
                fetch: start::FetchPolicy::Deny,
                skip_review: false,
            },
            token,
            &mut ownership,
        )
        .await?;
    assert_eq!(
        outcome.status(),
        crate::delivery::Status::Stub,
        "{:?}",
        outcome.failure
    );
    assert!(outcome.succeeded());
    assert_eq!(
        outcome.issue(),
        Some((42, "https://github.com/owned/project/issues/42"))
    );
    assert_eq!(outcome.task, "issue-42");
    assert_eq!(outcome.session, "roster-42-coder");
    assert_eq!(
        fixture.git(&outcome.worktree, &["symbolic-ref", "--short", "HEAD"])?,
        "agents/42"
    );
    assert_eq!(
        fs::read_to_string(outcome.worktree.join("docs").join("guide.md")).map_err(io)?,
        "# Guide\n\nBefore.\n"
    );
    assert!(outcome.coder_run.is_none());
    assert!(outcome.reviewer_run.is_none());
    assert!(
        outcome
            .format()
            .contains("no model calls, product edits or tests")
    );
    assert_eq!(calls.borrow().len(), 2);
    assert!(ownership.root().is_none());
    assert!(
        fs::read_to_string(outcome.worktree.join("RECIPE.yml"))
            .map_err(io)?
            .lines()
            .any(|line| line == "ask: issue:42")
    );
    fixture.finish()
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn issue_delivery_closed_issue_or_original_source_drift_never_claims_a_branch()
-> Result<(), Error> {
    for reason in ["closed", "source-drift", "branch-drift"] {
        let fixture = Fixture::new("good")?;
        fixture.delivery_source()?;
        let original_head = fixture.git(&fixture.root, &["rev-parse", "HEAD"])?;
        let original_index = fs::read(fixture.root.join(".git").join("index")).map_err(io)?;
        let environment = fixture.environment()?;
        let host = tools::Host {
            git: fixture.git.clone(),
            environment: host::minimal_environment(&environment)?,
            ..Default::default()
        };
        let mut transport = Mock::new(&delivery_response(if reason == "closed" {
            "closed"
        } else {
            "open"
        }));
        if reason == "source-drift" {
            let guide = fixture.root.join("docs").join("guide.md");
            transport.on_get = Some(Box::new(move || {
                fs::write(&guide, "Owned external change\n").map_err(io)
            }));
        } else if reason == "branch-drift" {
            fixture.git(&fixture.root, &["branch", "owned-same-commit", "HEAD"])?;
            let head = fixture.root.join(".git").join("HEAD");
            transport.on_get = Some(Box::new(move || {
                fs::write(&head, "ref: refs/heads/owned-same-commit\n").map_err(io)
            }));
        }
        let failure = crate::delivery::resolve_issue(
            &mut transport,
            &fixture.installation,
            &fixture.root,
            &environment,
            &host,
            42,
            &CancellationSource::default().token(),
        )
        .await
        .err()
        .ok_or_else(|| Error::type_error("Invalid issue intake was accepted"))?;
        assert_eq!(
            failure.code,
            Some(if reason == "closed" {
                "ROSTER_ISSUE_CLOSED"
            } else {
                "ROSTER_STALE_SOURCE"
            })
        );
        assert_eq!(
            fixture.git(&fixture.root, &["rev-parse", "HEAD"])?,
            original_head
        );
        assert_eq!(
            fs::read(fixture.root.join(".git").join("index")).map_err(io)?,
            original_index
        );
        if reason == "branch-drift" {
            assert_eq!(
                fixture.git(&fixture.root, &["symbolic-ref", "--short", "HEAD"])?,
                "owned-same-commit"
            );
        }
        assert!(
            fixture
                .git(
                    &fixture.root,
                    &[
                        "for-each-ref",
                        "--format=%(refname)",
                        "refs/heads/agents/42"
                    ]
                )?
                .is_empty()
        );
        assert!(!fixture.root.join(".worktrees").exists());
        assert_eq!(transport.calls.borrow().len(), 2);
        fixture.finish()?;
    }
    Ok(())
}

#[cfg(windows)]
type SendHook = Box<dyn FnMut(usize) -> Result<(), Error>>;

#[cfg(windows)]
struct ModelScript {
    responses: VecDeque<Value>,
    requests: Rc<RefCell<Vec<Value>>>,
    on_send: Option<SendHook>,
}

#[cfg(windows)]
impl Transport for ModelScript {
    type Body = Bytes;

    async fn send(&mut self, request: &Request) -> Result<Response<Bytes>, crate::llm::Failure> {
        assert!(request.url.starts_with("http://owned-model.invalid/"));
        let body = std::str::from_utf8(&request.body)
            .map_err(|_| Error::type_error("Owned model request must be UTF-8"))?;
        let index = {
            let mut requests = self.requests.borrow_mut();
            requests.push(json::parse(&Text::from(body))?);
            requests.len()
        };
        if let Some(action) = self.on_send.as_mut() {
            action(index)?;
        }
        let response = self
            .responses
            .pop_front()
            .ok_or_else(|| Error::type_error("Owned model sequence exhausted"))?;
        Ok(Response {
            status: 200,
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: Some(Bytes(Some(response.json().into_bytes()))),
        })
    }
}

#[cfg(windows)]
fn model_packet(message: Value, finish: &str, model: &str) -> Value {
    Value::object(vec![
        ("model", Value::string(model)),
        (
            "choices",
            Value::Array(vec![Value::object(vec![
                ("message", message),
                ("finish_reason", Value::string(finish)),
            ])]),
        ),
        (
            "usage",
            Value::object(vec![
                ("prompt_tokens", Value::Number(0.0)),
                ("completion_tokens", Value::Number(0.0)),
            ]),
        ),
    ])
}

#[cfg(windows)]
fn model_message(content: &str) -> Value {
    Value::object(vec![
        ("role", Value::string("assistant")),
        ("content", Value::string(content)),
    ])
}

#[cfg(windows)]
fn model_call(id: &str, name: &str, arguments: Value) -> Value {
    model_packet(
        Value::object(vec![
            ("role", Value::string("assistant")),
            ("content", Value::Null),
            (
                "tool_calls",
                Value::Array(vec![Value::object(vec![
                    ("id", Value::string(id)),
                    ("type", Value::string("function")),
                    (
                        "function",
                        Value::object(vec![
                            ("name", Value::string(name)),
                            ("arguments", Value::string(&arguments.json())),
                        ]),
                    ),
                ])]),
            ),
        ]),
        "tool_calls",
        "observed-issue-coder",
    )
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn issue_delivery_configured_planner_coder_and_fresh_reviewer_use_actual_separate_models()
-> Result<(), Error> {
    let fixture = Fixture::new("good")?;
    fixture.delivery_source()?;
    let config_source = include_str!("../../roster.config.example.yml")
        .replace("\r\n", "\n")
        .replace("turn_budget: 1000", "turn_budget: 16")
        .replacen(
            "base_url: \"\"",
            "base_url: http://owned-model.invalid/v1",
            1,
        )
        .replacen("model: \"\"", "model: configured-not-observed", 1)
        .replacen(
            "planner:\n",
            "planner:\n  critic_profile: owned-critic\n",
            1,
        );
    fs::write(
        fixture.root.join(".roster").join("config.yml"),
        config_source,
    )
    .map_err(io)?;
    let fleet = crate::fleet::format(&Value::object(vec![(
        "profiles",
        Value::Array(vec![Value::object(vec![
            ("id", Value::string("owned-critic")),
            ("base_url", Value::string("http://owned-model.invalid/v1")),
            ("model", Value::string("configured-critic-not-observed")),
            ("provider", Value::string("vllm")),
            ("api_key_env", Value::string("ROSTER_CRITIC_API_KEY")),
            ("context_max", Value::Number(32768.0)),
            ("concurrency", Value::Number(1.0)),
            ("hardware", Value::string("owned fixture")),
            (
                "notes",
                Value::string("Controlled model transport, not hardware"),
            ),
        ])]),
    )]))?;
    fs::write(
        fixture.root.join(".roster").join("fleet.yml"),
        fleet.to_utf8()?,
    )
    .map_err(io)?;
    let environment = fixture.environment()?;
    let host = tools::Host {
        git: fixture.git.clone(),
        node20: Some(fixture.node.clone()),
        environment: host::minimal_environment(&environment)?,
        ..Default::default()
    };
    let mut app = Mock::new(&delivery_response("open"));
    let cancellation = CancellationSource::default();
    let token = cancellation.token();
    let input = crate::delivery::resolve_issue(
        &mut app,
        &fixture.installation,
        &fixture.root,
        &environment,
        &host,
        42,
        &token,
    )
    .await?;
    let plan = Value::object(vec![
        ("title", Value::string("docs: update owned guide")),
        (
            "acceptance_checks",
            Value::Array(vec![Value::string(
                "The guide documents the owned outcome.",
            )]),
        ),
        (
            "files_allowed",
            Value::Array(vec![Value::string("docs/guide.md")]),
        ),
        ("task_class", Value::string("docs")),
    ]);
    let verdict = Value::object(vec![
        ("verdict", Value::string("pass")),
        ("reasons", Value::Array(Vec::new())),
        ("security_notes", Value::Array(Vec::new())),
        (
            "checks",
            Value::Array(vec![Value::object(vec![
                ("id", Value::Number(1.0)),
                ("met", Value::Boolean(true)),
                (
                    "evidence",
                    Value::string("The actual owned guide diff documents the named outcome."),
                ),
            ])]),
        ),
    ]);
    let mut model = ModelScript {
        responses: VecDeque::from([
            model_packet(
                model_message(&plan.json()),
                "stop",
                "observed-issue-planner",
            ),
            model_packet(
                model_message("{\"defects\":[]}"),
                "stop",
                "observed-issue-critic",
            ),
            model_call(
                "read",
                "read_file",
                Value::object(vec![("path", Value::string("docs/guide.md"))]),
            ),
            model_call(
                "edit",
                "edit_file",
                Value::object(vec![
                    ("path", Value::string("docs/guide.md")),
                    ("old_string", Value::string("Before.")),
                    (
                        "new_string",
                        Value::string("The owned outcome is documented."),
                    ),
                ]),
            ),
            model_packet(
                model_message("1. done: the owned guide documents the named outcome."),
                "stop",
                "observed-issue-coder",
            ),
            model_packet(
                model_message(&verdict.json()),
                "stop",
                "observed-issue-self-review",
            ),
            model_packet(
                model_message(&verdict.json()),
                "stop",
                "observed-issue-reviewer",
            ),
        ]),
        requests: Rc::default(),
        on_send: None,
    };
    let requests = model.requests.clone();
    let loaded = config::load(&fixture.installation, &fixture.root, &environment)?;
    let credentials = crate::llm::openai::Credentials {
        environment: Some(Value::object(vec![
            ("ROSTER_API_KEY", Value::string("test-only-model-key")),
            (
                "ROSTER_CRITIC_API_KEY",
                Value::string("test-only-critic-key"),
            ),
        ])),
        ..Default::default()
    };
    let admission = crate::llm::admission::Admission::default();
    let mut observer = crate::llm::control::NoObserver;
    let mut driver = crate::delivery::Driver {
        transport: &mut model,
        observer: &mut observer,
        credentials: &credentials,
        admission: &admission,
    };
    let mut ownership = crate::delivery::Ownership::default();
    let outcome = driver
        .run_local(
            crate::delivery::Options {
                cwd: &fixture.root,
                installation: &fixture.installation,
                loaded: &loaded,
                input,
                host,
                approval: crate::execution::backend::Approval {
                    counterfactual: true,
                    classifications: true,
                    ..Default::default()
                },
                start: start::Options::resolve(
                    &loaded.config,
                    Some("current"),
                    Some(start::Sync::Offline),
                )?,
                fetch: start::FetchPolicy::Deny,
                skip_review: false,
            },
            token,
            &mut ownership,
        )
        .await?;
    assert_eq!(
        outcome.status(),
        crate::delivery::Status::Verified,
        "{:?}",
        outcome.failure
    );
    assert!(outcome.succeeded());
    assert_eq!(
        outcome
            .planner_run
            .as_ref()
            .map(|run| run.metrics().model.as_str()),
        Some("observed-issue-planner")
    );
    assert_eq!(
        outcome
            .coder_run
            .as_ref()
            .map(|run| run.metrics().model.as_str()),
        Some("observed-issue-coder")
    );
    assert_eq!(
        outcome
            .reviewer_run
            .as_ref()
            .map(|run| run.metrics().model.as_str()),
        Some("observed-issue-reviewer")
    );
    assert_eq!(
        outcome
            .coder_run
            .as_ref()
            .and_then(|run| run.metrics().usage.prompt_tokens),
        Some(0)
    );
    assert_eq!(
        outcome.issue(),
        Some((42, "https://github.com/owned/project/issues/42"))
    );
    assert_eq!(
        fs::read_to_string(outcome.worktree.join("docs").join("guide.md")).map_err(io)?,
        "# Guide\n\nThe owned outcome is documented.\n"
    );
    assert_eq!(app.calls.borrow().len(), 2);
    assert_eq!(requests.borrow().len(), 7);
    assert!(model.responses.is_empty());
    assert!(ownership.root().is_none());
    fixture.finish()
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn issue_delivery_known_legacy_attempt_and_remote_claims_are_never_adopted_or_replaced()
-> Result<(), Error> {
    for claimed in [
        "refs/heads/issue-42",
        "refs/heads/agents/42-a1",
        "refs/remotes/origin/agents/42",
    ] {
        let fixture = Fixture::new("good")?;
        fixture.delivery_source()?;
        let head = fixture.git(&fixture.root, &["rev-parse", "HEAD"])?;
        fixture.git(&fixture.root, &["update-ref", claimed, &head])?;
        let environment = fixture.environment()?;
        let host = tools::Host {
            git: fixture.git.clone(),
            environment: host::minimal_environment(&environment)?,
            ..Default::default()
        };
        let mut transport = Mock::new(&delivery_response("open"));
        let cancellation = CancellationSource::default();
        let token = cancellation.token();
        let input = crate::delivery::resolve_issue(
            &mut transport,
            &fixture.installation,
            &fixture.root,
            &environment,
            &host,
            42,
            &token,
        )
        .await?;
        let loaded = config::load(&fixture.installation, &fixture.root, &environment)?;
        let credentials = crate::llm::openai::Credentials::default();
        let admission = crate::llm::admission::Admission::default();
        let mut observer = crate::llm::control::NoObserver;
        let mut driver = crate::delivery::Driver {
            transport: &mut transport,
            observer: &mut observer,
            credentials: &credentials,
            admission: &admission,
        };
        let mut ownership = crate::delivery::Ownership::default();
        let failure = driver
            .run_local(
                crate::delivery::Options {
                    cwd: &fixture.root,
                    installation: &fixture.installation,
                    loaded: &loaded,
                    input,
                    host,
                    approval: crate::execution::backend::Approval::default(),
                    start: start::Options::resolve(
                        &loaded.config,
                        Some("current"),
                        Some(start::Sync::Offline),
                    )?,
                    fetch: start::FetchPolicy::Deny,
                    skip_review: false,
                },
                token,
                &mut ownership,
            )
            .await
            .err()
            .ok_or_else(|| Error::type_error("Existing issue ownership was adopted"))?;
        assert!(failure.to_string().contains("ownership collision"));
        assert_eq!(fixture.git(&fixture.root, &["rev-parse", claimed])?, head);
        assert!(!fixture.root.join(".worktrees").exists());
        assert!(
            fixture
                .git(
                    &fixture.root,
                    &[
                        "for-each-ref",
                        "--format=%(refname)",
                        "refs/heads/agents/42"
                    ]
                )?
                .lines()
                .all(|name| name != "refs/heads/agents/42")
        );
        assert!(ownership.root().is_none());
        fixture.finish()?;
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn issue_delivery_retains_original_source_branch_and_index_seals_until_preparation()
-> Result<(), Error> {
    for drift in ["source", "index", "branch"] {
        let fixture = Fixture::new("good")?;
        fixture.delivery_source()?;
        let original_head = fixture.git(&fixture.root, &["rev-parse", "HEAD"])?;
        let index = fixture.root.join(".git").join("index");
        let original_index = fs::read(&index).map_err(io)?;
        let environment = fixture.environment()?;
        let host = tools::Host {
            git: fixture.git.clone(),
            environment: host::minimal_environment(&environment)?,
            ..Default::default()
        };
        let mut transport = Mock::new(&delivery_response("open"));
        let cancellation = CancellationSource::default();
        let token = cancellation.token();
        let input = crate::delivery::resolve_issue(
            &mut transport,
            &fixture.installation,
            &fixture.root,
            &environment,
            &host,
            42,
            &token,
        )
        .await?;
        match drift {
            "source" => fs::write(
                fixture.root.join("docs").join("guide.md"),
                "Owned change after App intake\n",
            )
            .map_err(io)?,
            "index" => {
                fixture.git(
                    &fixture.root,
                    &["update-index", "--assume-unchanged", "--", "docs/guide.md"],
                )?;
                assert_ne!(fs::read(&index).map_err(io)?, original_index);
                assert!(
                    fixture
                        .git(&fixture.root, &["status", "--porcelain"])?
                        .is_empty()
                );
            }
            "branch" => {
                fixture.git(&fixture.root, &["branch", "owned-after-intake", "HEAD"])?;
                fixture.git(
                    &fixture.root,
                    &["symbolic-ref", "HEAD", "refs/heads/owned-after-intake"],
                )?;
                assert_eq!(fs::read(&index).map_err(io)?, original_index);
            }
            _ => return Err(Error::type_error("Unknown owned intake drift")),
        }
        let loaded = config::load(&fixture.installation, &fixture.root, &environment)?;
        let credentials = crate::llm::openai::Credentials::default();
        let admission = crate::llm::admission::Admission::default();
        let mut observer = crate::llm::control::NoObserver;
        let mut driver = crate::delivery::Driver {
            transport: &mut transport,
            observer: &mut observer,
            credentials: &credentials,
            admission: &admission,
        };
        let mut ownership = crate::delivery::Ownership::default();
        let failure = driver
            .run_local(
                crate::delivery::Options {
                    cwd: &fixture.root,
                    installation: &fixture.installation,
                    loaded: &loaded,
                    input,
                    host,
                    approval: crate::execution::backend::Approval::default(),
                    start: start::Options::resolve(
                        &loaded.config,
                        Some("current"),
                        Some(start::Sync::Offline),
                    )?,
                    fetch: start::FetchPolicy::Deny,
                    skip_review: false,
                },
                token,
                &mut ownership,
            )
            .await
            .err()
            .ok_or_else(|| {
                Error::type_error("A fresh snapshot replaced the original intake seal")
            })?;
        assert_eq!(failure.code, Some("ROSTER_STALE_SOURCE"));
        assert_eq!(
            fs::read_to_string(fixture.root.join("docs").join("guide.md")).map_err(io)?,
            if drift == "source" {
                "Owned change after App intake\n"
            } else {
                "# Guide\n\nBefore.\n"
            }
        );
        assert_eq!(
            fixture.git(&fixture.root, &["rev-parse", "HEAD"])?,
            original_head
        );
        if drift == "branch" {
            assert_eq!(
                fixture.git(&fixture.root, &["symbolic-ref", "--short", "HEAD"])?,
                "owned-after-intake"
            );
        }
        assert!(!fixture.root.join(".worktrees").exists());
        assert!(ownership.root().is_none());
        fixture.finish()?;
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn issue_delivery_unresolved_or_foreign_intake_never_becomes_a_local_task()
-> Result<(), Error> {
    for mode in ["unresolved", "foreign"] {
        let fixture = Fixture::new("good")?;
        fixture.delivery_source()?;
        let other = Fixture::new("good")?;
        other.delivery_source()?;
        let environment = fixture.environment()?;
        let host = tools::Host {
            git: fixture.git.clone(),
            environment: host::minimal_environment(&environment)?,
            ..Default::default()
        };
        let mut transport = Mock::new(&delivery_response("open"));
        let cancellation = CancellationSource::default();
        let token = cancellation.token();
        let input = if mode == "unresolved" {
            crate::delivery::Input::GitHubIssue(42)
        } else {
            crate::delivery::resolve_issue(
                &mut transport,
                &fixture.installation,
                &fixture.root,
                &environment,
                &host,
                42,
                &token,
            )
            .await?
        };
        let loaded = config::load(&other.installation, &other.root, &other.environment()?)?;
        let credentials = crate::llm::openai::Credentials::default();
        let admission = crate::llm::admission::Admission::default();
        let mut observer = crate::llm::control::NoObserver;
        let mut driver = crate::delivery::Driver {
            transport: &mut transport,
            observer: &mut observer,
            credentials: &credentials,
            admission: &admission,
        };
        let mut ownership = crate::delivery::Ownership::default();
        let failure = driver
            .run_local(
                crate::delivery::Options {
                    cwd: &other.root,
                    installation: &other.installation,
                    loaded: &loaded,
                    input,
                    host,
                    approval: crate::execution::backend::Approval::default(),
                    start: start::Options::resolve(
                        &loaded.config,
                        Some("current"),
                        Some(start::Sync::Offline),
                    )?,
                    fetch: start::FetchPolicy::Deny,
                    skip_review: false,
                },
                token,
                &mut ownership,
            )
            .await
            .err()
            .ok_or_else(|| Error::type_error("Invalid intake became a local delivery"))?;
        assert_eq!(
            failure.code,
            Some(if mode == "unresolved" {
                "ROSTER_NATIVE_UNSUPPORTED"
            } else {
                "ROSTER_TOOL_DENIED"
            })
        );
        assert!(!fixture.root.join(".worktrees").exists());
        assert!(!other.root.join(".worktrees").exists());
        assert!(ownership.root().is_none());
        fixture.finish()?;
        other.finish()?;
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn issue_delivery_index_only_drift_during_app_intake_refuses_before_claim()
-> Result<(), Error> {
    for mode in ["main", "linked", "split", "linked-split"] {
        let fixture = Fixture::new("good")?;
        fixture.delivery_source()?;
        let cwd = if mode.contains("linked") {
            let linked = fixture.container.join("linked");
            fixture.git(
                &fixture.root,
                &[
                    "worktree",
                    "add",
                    "--quiet",
                    "-b",
                    "owned-intake",
                    linked
                        .to_str()
                        .ok_or_else(|| Error::type_error("Owned linked path must be Unicode"))?,
                ],
            )?;
            linked
        } else {
            fixture.root.clone()
        };
        let shared = if mode.contains("split") {
            fixture.git(&cwd, &["update-index", "--split-index"])?;
            let raw = fixture.git(&cwd, &["rev-parse", "--shared-index-path"])?;
            if raw.is_empty() {
                return Err(Error::type_error("Owned split index has no shared file"));
            }
            Some(paths::resolve(&cwd, Path::new(&raw))?)
        } else {
            None
        };
        let environment = fixture.environment()?;
        let host = tools::Host {
            git: fixture.git.clone(),
            environment: host::minimal_environment(&environment)?,
            ..Default::default()
        };
        let main_index = fixture.root.join(".git").join("index");
        let main_before = fs::read(&main_index).map_err(io)?;
        let index = paths::resolve(
            &cwd,
            Path::new(&fixture.git(&cwd, &["rev-parse", "--git-path", "index"])?),
        )?;
        let before = fs::read(&index).map_err(io)?;
        let git = fixture.git.clone();
        let root = cwd.clone();
        let process_environment = tools::Host::default().environment;
        let mut transport = Mock::new(&delivery_response("open"));
        if let Some(shared) = &shared {
            let shared = shared.clone();
            let mut changed = fs::read(&shared).map_err(io)?;
            changed.push(0);
            transport.on_get = Some(Box::new(move || fs::write(&shared, &changed).map_err(io)));
        } else {
            transport.on_get = Some(Box::new(move || {
                let output = Command::new(&git)
                    .current_dir(&root)
                    .env_clear()
                    .envs(&process_environment)
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .env("GIT_CONFIG_GLOBAL", "NUL")
                    .env("GIT_OPTIONAL_LOCKS", "0")
                    .args([
                        "--no-pager",
                        "update-index",
                        "--assume-unchanged",
                        "--",
                        "docs/guide.md",
                    ])
                    .output()
                    .map_err(io)?;
                if !output.status.success() {
                    return Err(Error::type_error(
                        "Owned index-only mutation did not execute",
                    ));
                }
                Ok(())
            }));
        }
        let result = crate::delivery::resolve_issue(
            &mut transport,
            &fixture.installation,
            &cwd,
            &environment,
            &host,
            42,
            &CancellationSource::default().token(),
        )
        .await;
        if shared.is_some() {
            assert_eq!(fs::read(&index).map_err(io)?, before);
        } else {
            assert_ne!(fs::read(&index).map_err(io)?, before);
            assert!(fixture.git(&cwd, &["status", "--porcelain"])?.is_empty());
            assert!(fixture.git(&cwd, &["diff", "HEAD", "--"])?.is_empty());
        }
        if mode.contains("linked") {
            assert_eq!(fs::read(&main_index).map_err(io)?, main_before);
        }
        let failure = result.err().ok_or_else(|| {
            Error::type_error("Index-only drift was hidden by unchanged source/status")
        })?;
        assert_eq!(failure.code, Some("ROSTER_STALE_SOURCE"));
        assert!(!fixture.root.join(".worktrees").exists());
        assert!(!cwd.join(".worktrees").exists());
        assert_eq!(transport.calls.borrow().len(), 2);
        fixture.finish()?;
    }
    Ok(())
}

#[cfg(windows)]
fn issue_payload(title: &str, body: &str) -> String {
    Value::object(vec![
        ("number", Value::Number(42.0)),
        ("title", Value::string(title)),
        ("body", Value::string(body)),
        ("state", Value::string("open")),
        (
            "html_url",
            Value::string("https://github.com/owned/project/issues/42"),
        ),
    ])
    .json()
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn issue_delivery_review_title_body_classification_and_metadata_contracts()
-> Result<(), Error> {
    {
        let fixture = Fixture::new("good")?;
        fixture.delivery_source()?;
        let environment = fixture.environment()?;
        let host = tools::Host {
            git: fixture.git.clone(),
            node20: Some(fixture.node.clone()),
            environment: host::minimal_environment(&environment)?,
            ..Default::default()
        };
        let mut transport = Mock::new(&issue_payload(
            "Roadmap item",
            "Build a complete standalone platform documented in docs/guide.md.",
        ));
        let token = CancellationSource::default().token();
        let input = crate::delivery::resolve_issue(
            &mut transport,
            &fixture.installation,
            &fixture.root,
            &environment,
            &host,
            42,
            &token,
        )
        .await?;
        let loaded = config::load(&fixture.installation, &fixture.root, &environment)?;
        let credentials = crate::llm::openai::Credentials::default();
        let admission = crate::llm::admission::Admission::default();
        let mut observer = crate::llm::control::NoObserver;
        let mut driver = crate::delivery::Driver {
            transport: &mut transport,
            observer: &mut observer,
            credentials: &credentials,
            admission: &admission,
        };
        let mut ownership = crate::delivery::Ownership::default();
        let outcome = driver
            .run_local(
                crate::delivery::Options {
                    cwd: &fixture.root,
                    installation: &fixture.installation,
                    loaded: &loaded,
                    input,
                    host,
                    approval: crate::execution::backend::Approval::default(),
                    start: start::Options::resolve(
                        &loaded.config,
                        Some("current"),
                        Some(start::Sync::Offline),
                    )?,
                    fetch: start::FetchPolicy::Deny,
                    skip_review: false,
                },
                token,
                &mut ownership,
            )
            .await?;
        assert_eq!(outcome.ask_kind, crate::planning::intake::Kind::Initiative);
        assert_eq!(outcome.status(), crate::delivery::Status::PlanningOnly);
        assert!(outcome.worktree.join("PLAN.md").is_file());
        assert!(!outcome.worktree.join("RESULT.md").exists());
        fixture.finish()?;
    }
    {
        let fixture = Fixture::new("good")?;
        fixture.delivery_source()?;
        fs::write(fixture.root.join("README.md"), "# Owned\n").map_err(io)?;
        fixture.git(&fixture.root, &["add", "--", "README.md"])?;
        fixture.git(
            &fixture.root,
            &["commit", "--quiet", "-m", "Add owned README"],
        )?;
        let environment = fixture.environment()?;
        let host = tools::Host {
            git: fixture.git.clone(),
            node20: Some(fixture.node.clone()),
            environment: host::minimal_environment(&environment)?,
            ..Default::default()
        };
        let mut transport = Mock::new(&issue_payload(
            "Feature: README",
            "# Ask\n\nAdd a one-liner to README.md\n\n## Acceptance checks\n- README.md includes the one-liner.\n\n## Task metadata\n\ntask_class: docs\ndifficulty: 4\nestimate_min: 25\n",
        ));
        let token = CancellationSource::default().token();
        let input = crate::delivery::resolve_issue(
            &mut transport,
            &fixture.installation,
            &fixture.root,
            &environment,
            &host,
            42,
            &token,
        )
        .await?;
        let loaded = config::load(&fixture.installation, &fixture.root, &environment)?;
        let credentials = crate::llm::openai::Credentials::default();
        let admission = crate::llm::admission::Admission::default();
        let mut observer = crate::llm::control::NoObserver;
        let mut driver = crate::delivery::Driver {
            transport: &mut transport,
            observer: &mut observer,
            credentials: &credentials,
            admission: &admission,
        };
        let mut ownership = crate::delivery::Ownership::default();
        let outcome = driver
            .run_local(
                crate::delivery::Options {
                    cwd: &fixture.root,
                    installation: &fixture.installation,
                    loaded: &loaded,
                    input,
                    host,
                    approval: crate::execution::backend::Approval::default(),
                    start: start::Options::resolve(
                        &loaded.config,
                        Some("current"),
                        Some(start::Sync::Offline),
                    )?,
                    fetch: start::FetchPolicy::Deny,
                    skip_review: false,
                },
                token,
                &mut ownership,
            )
            .await?;
        assert_eq!(outcome.ask_kind, crate::planning::intake::Kind::Slice);
        assert_eq!(
            outcome.status(),
            crate::delivery::Status::Stub,
            "{:?}",
            outcome.failure
        );
        let task = fs::read_to_string(outcome.worktree.join("TASK.md")).map_err(io)?;
        assert!(task.contains("# Task: Feature: README"));
        assert!(task.contains("difficulty: 4"));
        assert!(task.contains("estimate_min: 25"));
        assert!(task.contains("## Ask\nAdd a one-liner to README.md"));
        assert!(!task.contains("## Ask\nFeature: README"));
        assert!(!task.contains("## Task metadata"));
        fixture.finish()?;
    }
    for (title, body) in [
        ("docs: empty body", "   \n  "),
        (
            "docs: invalid metadata",
            "# Ask\n\nUpdate docs/guide.md.\n\n## Task metadata\n\ntask_class: docs\ndifficulty: 9\nestimate_min: 10\n",
        ),
        (
            &format!("docs: {}", "t".repeat(195)),
            &format!(
                "Update docs/guide.md.\n{}\n## Files allowed\n- docs/guide.md\n\n## Acceptance checks\n- The guide is updated.\n",
                "b".repeat(16_150)
            ),
        ),
    ] {
        let fixture = Fixture::new("good")?;
        fixture.delivery_source()?;
        let environment = fixture.environment()?;
        let host = tools::Host {
            git: fixture.git.clone(),
            node20: Some(fixture.node.clone()),
            environment: host::minimal_environment(&environment)?,
            ..Default::default()
        };
        let mut transport = Mock::new(&issue_payload(title, body));
        let token = CancellationSource::default().token();
        let resolved = crate::delivery::resolve_issue(
            &mut transport,
            &fixture.installation,
            &fixture.root,
            &environment,
            &host,
            42,
            &token,
        )
        .await;
        if let Ok(input) = resolved {
            let loaded = config::load(&fixture.installation, &fixture.root, &environment)?;
            let credentials = crate::llm::openai::Credentials::default();
            let admission = crate::llm::admission::Admission::default();
            let mut observer = crate::llm::control::NoObserver;
            let mut driver = crate::delivery::Driver {
                transport: &mut transport,
                observer: &mut observer,
                credentials: &credentials,
                admission: &admission,
            };
            let mut ownership = crate::delivery::Ownership::default();
            let failure = driver
                .run_local(
                    crate::delivery::Options {
                        cwd: &fixture.root,
                        installation: &fixture.installation,
                        loaded: &loaded,
                        input,
                        host,
                        approval: crate::execution::backend::Approval::default(),
                        start: start::Options::resolve(
                            &loaded.config,
                            Some("current"),
                            Some(start::Sync::Offline),
                        )?,
                        fetch: start::FetchPolicy::Deny,
                        skip_review: false,
                    },
                    token,
                    &mut ownership,
                )
                .await;
            assert!(failure.is_err());
        }
        assert!(!fixture.root.join(".worktrees").exists());
        fixture.finish()?;
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn issue_delivery_review_origin_drift_during_execution_refuses_verified_outcome()
-> Result<(), Error> {
    let fixture = Fixture::new("good")?;
    fixture.delivery_source()?;
    let config_source = include_str!("../../roster.config.example.yml")
        .replace("\r\n", "\n")
        .replace("turn_budget: 1000", "turn_budget: 16")
        .replacen(
            "base_url: \"\"",
            "base_url: http://owned-model.invalid/v1",
            1,
        )
        .replacen("model: \"\"", "model: configured-not-observed", 1);
    fs::write(
        fixture.root.join(".roster").join("config.yml"),
        config_source,
    )
    .map_err(io)?;
    let environment = fixture.environment()?;
    let host = tools::Host {
        git: fixture.git.clone(),
        node20: Some(fixture.node.clone()),
        environment: host::minimal_environment(&environment)?,
        ..Default::default()
    };
    let mut app = Mock::new(&issue_payload(
        "Roadmap item",
        "Build a complete standalone platform documented in docs/guide.md.",
    ));
    let token = CancellationSource::default().token();
    let input = crate::delivery::resolve_issue(
        &mut app,
        &fixture.installation,
        &fixture.root,
        &environment,
        &host,
        42,
        &token,
    )
    .await?;
    let outline = Value::object(vec![
        ("title", Value::string("Roadmap item")),
        ("summary", Value::string("Split the platform initiative.")),
        (
            "issues",
            Value::Array(vec![Value::object(vec![
                ("title", Value::string("docs: slice one")),
                ("ask", Value::string("Document slice one in docs/guide.md.")),
                (
                    "acceptance_checks",
                    Value::Array(vec![Value::string("docs/guide.md documents slice one.")]),
                ),
                (
                    "files_allowed",
                    Value::Array(vec![Value::string("docs/guide.md")]),
                ),
            ])]),
        ),
    ]);
    let git = fixture.git.clone();
    let root = fixture.root.clone();
    let process_environment = tools::Host::default().environment;
    let mut model = ModelScript {
        responses: VecDeque::from([model_packet(
            model_message(&outline.json()),
            "stop",
            "observed-issue-planner",
        )]),
        requests: Rc::default(),
        on_send: Some(Box::new(move |_| {
            let output = Command::new(&git)
                .current_dir(&root)
                .env_clear()
                .envs(&process_environment)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "NUL")
                .env("GIT_OPTIONAL_LOCKS", "0")
                .args([
                    "--no-pager",
                    "remote",
                    "set-url",
                    "origin",
                    "https://github.com/owned/foreign.git",
                ])
                .output()
                .map_err(io)?;
            if !output.status.success() {
                return Err(Error::type_error("Origin mutation did not execute"));
            }
            Ok(())
        })),
    };
    let loaded = config::load(&fixture.installation, &fixture.root, &environment)?;
    let credentials = crate::llm::openai::Credentials {
        environment: Some(Value::object(vec![(
            "ROSTER_API_KEY",
            Value::string("test-only-model-key"),
        )])),
        ..Default::default()
    };
    let admission = crate::llm::admission::Admission::default();
    let mut observer = crate::llm::control::NoObserver;
    let mut driver = crate::delivery::Driver {
        transport: &mut model,
        observer: &mut observer,
        credentials: &credentials,
        admission: &admission,
    };
    let mut ownership = crate::delivery::Ownership::default();
    let failure = driver
        .run_local(
            crate::delivery::Options {
                cwd: &fixture.root,
                installation: &fixture.installation,
                loaded: &loaded,
                input,
                host,
                approval: crate::execution::backend::Approval::default(),
                start: start::Options::resolve(
                    &loaded.config,
                    Some("current"),
                    Some(start::Sync::Offline),
                )?,
                fetch: start::FetchPolicy::Deny,
                skip_review: false,
            },
            token,
            &mut ownership,
        )
        .await
        .err()
        .ok_or_else(|| Error::type_error("Origin drift during execution was accepted"))?;
    assert_eq!(failure.code, Some("ROSTER_GITHUB_SCOPE"));
    fixture.finish()
}
