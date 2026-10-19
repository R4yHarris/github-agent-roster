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
