use super::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use github_agent_roster::cancel::Reason;
use github_agent_roster::llm;
use github_agent_roster::llm::http::{Body, Request, Response, Transport};
use tokio::sync::oneshot;

struct GatedBody {
    ready: Option<oneshot::Sender<()>>,
    dropped: Rc<Cell<usize>>,
}

impl Body for GatedBody {
    async fn read(&mut self) -> Result<Option<Vec<u8>>, Error> {
        if let Some(ready) = self.ready.take() {
            ready.send(()).map_err(|_| {
                Error::type_error("Actual invocation body readiness receiver disappeared")
            })?;
        }
        std::future::pending().await
    }
}

impl Drop for GatedBody {
    fn drop(&mut self) {
        self.dropped.set(self.dropped.get() + 1);
    }
}

struct Gated {
    body: Option<GatedBody>,
    requests: usize,
}

impl Transport for Gated {
    type Body = GatedBody;
    async fn send(&mut self, _: &Request) -> Result<Response<GatedBody>, llm::Failure> {
        self.requests += 1;
        Ok(Response {
            status: 200,
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: Some(
                self.body
                    .take()
                    .ok_or_else(|| Error::type_error("Only one owned pending body was approved"))?,
            ),
        })
    }
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn actual_local_invocation_cancel_and_immediate_future_drop_release_admission_and_preserve_caller_owned_cleanup()
-> Result<(), Error> {
    for immediate_drop in [false, true] {
        let fixture = Fixture::new()?;
        fixture.configure(Some("http://127.0.0.1:1/v1"))?;
        fixture.task(
            &["docs/guide.md"],
            "The guide documents the requested bounded outcome.",
        )?;
        let mut loaded = fixture.loaded()?;
        loaded.config =
            loaded
                .config
                .with_profile(&github_agent_roster::fleet::validate_profile(
                    &Value::object(vec![
                        ("id", Value::string("owned-local-profile")),
                        ("base_url", Value::string("http://127.0.0.1:1/v1")),
                        ("model", Value::string("owned-route")),
                        ("provider", Value::string("vllm")),
                        ("context_max", Value::Number(32768.0)),
                        ("concurrency", Value::Number(1.0)),
                        ("hardware", Value::string("owned fixture")),
                        ("notes", Value::string("No external endpoint")),
                    ]),
                )?)?;
        let (ready, received) = oneshot::channel();
        let dropped = Rc::new(Cell::new(0));
        let mut transport = Gated {
            body: Some(GatedBody {
                ready: Some(ready),
                dropped: dropped.clone(),
            }),
            requests: 0,
        };
        let mut observer = NoObserver;
        let credentials = Credentials::default();
        let admission = Admission::default();
        let source = CancellationSource::default();
        let (host, approval) = fixture.host()?;
        let mut ownership = delivery::Ownership::default();
        let mut driver = delivery::Driver {
            transport: &mut transport,
            observer: &mut observer,
            credentials: &credentials,
            admission: &admission,
        };
        let mut work = Box::pin(driver.run_local(
            delivery::Options {
                cwd: &fixture.root,
                installation: Path::new(env!("CARGO_MANIFEST_DIR")),
                loaded: &loaded,
                input: Input::ExistingTask,
                host,
                approval,
                start: start::Options::resolve(&loaded.config, None, None)?,
                fetch: start::FetchPolicy::Deny,
                skip_review: false,
            },
            source.token(),
            &mut ownership,
        ));
        tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(600), received) =>
                result.map_err(|_| Error::type_error("Actual invocation body never became ready"))?.map_err(|_| Error::type_error("Actual invocation ended before readiness"))?,
            result = &mut work => return Err(Error::new("FixtureError", format!("Invocation ended before its real body barrier: {:?}", result.err()))),
        }
        let profile = Text::from("owned-local-profile");
        let queued = admission.acquire(&profile, None, None)?;
        assert_eq!(admission.depth(&profile)?, 1);
        source.cancel(Reason::Cancelled)?;
        if immediate_drop {
            drop(work);
            assert!(ownership.root().is_some());
            ownership.close().await?;
        } else {
            let error = work
                .await
                .err()
                .ok_or_else(|| Error::type_error("Cancelled invocation returned success"))?;
            assert_eq!(error.code, Some("ROSTER_CANCELLED"));
        }
        assert_eq!(dropped.get(), 1);
        assert!(ownership.root().is_none());
        let mut successor = tokio::time::timeout(Duration::from_secs(10), queued.wait())
            .await
            .map_err(|_| {
                Error::type_error("Dropped invocation leaked shared request admission")
            })??;
        successor.release()?;
        assert_eq!(admission.depth(&profile)?, 0);
        assert_eq!(transport.requests, 1);
        assert_eq!(
            fs::read_to_string(fixture.root.join("docs").join("guide.md")).map_err(io)?,
            "# Guide\n\nBefore.\n"
        );
        assert!(
            !fixture
                .root
                .join(".roster")
                .join("locks")
                .join("run.lock")
                .exists()
        );
        for file in ["RESULT.md", "REVIEW.md"] {
            if let Ok(body) = fs::read_to_string(fixture.root.join(file)) {
                assert!(!body.contains("Checks: PASS") && !body.contains("Verdict: pass"));
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn dropped_real_preplan_hook_future_stops_the_owned_child_but_quarantines_its_writer()
-> Result<(), Error> {
    use github_agent_roster::execution::hooks;
    use winsafe::{HPROCESS, co, prelude::*};

    let fixture = Fixture::new()?;
    fixture.write(
        ".roster/hooks.yml",
        "hooks:\n  - event: pre-plan\n    script: .roster/hooks/owned.mjs\n    timeout_ms: 60000\n",
    )?;
    fixture.write(".roster/hooks/owned.mjs", "import { writeFileSync } from 'node:fs';\nsetInterval(() => {}, 1000);\nwriteFileSync('.roster/logs/actual-hook.json', JSON.stringify({ pid: process.pid }), { flag: 'wx' });\n")?;
    fixture.git(&["add", "-f", ".roster/hooks.yml", ".roster/hooks/owned.mjs"])?;
    fixture.git(&["commit", "--quiet", "-m", "Owned human preplan hook"])?;
    secure::private_directory(&fixture.root.join(".roster").join("logs"), true)?;
    let source = CancellationSource::default();
    let (host, approval) = fixture.host()?;
    let mut lease = backend::RootLease::open(
        &fixture.root,
        "owned-hook-drop",
        host,
        Default::default(),
        source.token(),
    )
    .await?;
    let mut work = Box::pin(hooks::run_pre_plan(&lease, &approval));
    let ready_path = fixture
        .root
        .join(".roster")
        .join("logs")
        .join("actual-hook.json");
    let wait = async {
        loop {
            if let Some(snapshot) = secure::read(&ready_path, 4096)? {
                let value = github_agent_roster::json::parse(&Text::from(text(&snapshot.bytes)?))?;
                if let Some(value) = value.get("pid").and_then(Value::as_number) {
                    if value.fract() == 0.0 && (1.0..=f64::from(u32::MAX)).contains(&value) {
                        return Ok::<_, Error>(value as u32);
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    let pid = tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(180), wait) => result.map_err(|_| Error::type_error("Actual preplan child readiness was not observed"))??,
        result = &mut work => return Err(Error::new("FixtureError", format!("Hook ended before actual readiness: {:?}", result.err()))),
    };
    let handle = HPROCESS::OpenProcess(
        co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::SYNCHRONIZE,
        false,
        pid,
    )
    .map_err(|error| Error::new("FixtureError", error.to_string()))?;
    assert_eq!(
        handle
            .WaitForSingleObject(Some(0))
            .map_err(|error| Error::new("FixtureError", error.to_string()))?,
        co::WAIT::TIMEOUT
    );
    source.cancel(Reason::Cancelled)?;
    drop(work);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if handle
                .WaitForSingleObject(Some(0))
                .map_err(|error| Error::new("FixtureError", error.to_string()))?
                == co::WAIT::OBJECT_0
            {
                return Ok::<_, Error>(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| Error::type_error("Owned preplan child survived its dropped future"))??;
    assert_eq!(
        lease.close().await.err().and_then(|error| error.code),
        Some("ROSTER_PROCESS_CLEANUP")
    );
    assert!(lease.ownership().path.is_file());
    assert!(!fixture.root.join("TASK.md").exists());
    Ok(())
}
