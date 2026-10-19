use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use super::*;
use crate::paths::{self, FileIdentity};
use crate::state::secure;

pub struct Report {
    identity: Arc<()>,
    root: FileIdentity,
    authority: String,
    stamp: Option<backend::Stamp>,
    value: Value,
    pass: bool,
}

impl Report {
    pub fn as_value(&self) -> &Value {
        &self.value
    }
    pub fn passed(&self) -> bool {
        self.pass
    }
    async fn require_owner(&self, owner: backend::Owner<'_>) -> Result<(), Error> {
        owner.check()?;
        if !Arc::ptr_eq(&self.identity, &owner.identity())
            || self.authority != owner.authority()
            || self.root != paths::file_information(owner.root())?.identity
        {
            return Err(denied(
                "Lifecycle receipt belongs to another root, writer or authority",
            ));
        }
        if let Some(stamp) = &self.stamp {
            return stamp.require_current(owner).await;
        }
        if secure::read(&owner.root().join(".roster").join("hooks.yml"), 65_536)?.is_some() {
            return Err(denied(
                "Previously absent lifecycle manifest appeared after applicability was checked",
            ));
        }
        Ok(())
    }
    pub async fn require_current(&self, boundary: &tools::Boundary) -> Result<(), Error> {
        self.require_owner(backend::Owner::Execution(boundary))
            .await
    }
}

pub struct PrePlanReport<'a> {
    lease: &'a backend::RootLease,
    report: Report,
}

impl PrePlanReport<'_> {
    pub fn as_value(&self) -> &Value {
        self.report.as_value()
    }
    pub async fn require_current(&self, worktree: &Path) -> Result<(), Error> {
        if paths::absolute(worktree)? != self.lease.root() {
            return Err(denied(
                "Pre-plan hook receipt is scoped to another worktree",
            ));
        }
        self.report
            .require_owner(backend::Owner::PrePlan(self.lease))
            .await?;
        if !self.report.pass {
            return Err(Error::new(
                "LifecycleHookError",
                "Pre-plan lifecycle gate did not pass",
            ));
        }
        Ok(())
    }
    pub fn ownership(&self) -> &crate::state::locks::RepoLock {
        self.lease.ownership()
    }
    pub fn token(&self) -> &crate::cancel::CancellationToken {
        self.lease.token()
    }
}

fn read_hook(root: &Path, path: &str, optional: bool) -> Result<Option<secure::Snapshot>, Error> {
    let file = root.join(path);
    paths::ensure_local_path(&file, root)?;
    let current = secure::read(&file, 65_536)?;
    let Some(snapshot) = current else {
        return if optional {
            Ok(None)
        } else {
            Err(Error::new(
                "LifecycleHookError",
                format!("Required lifecycle source is missing: {path}"),
            )
            .coded("ENOENT"))
        };
    };
    if paths::file_information(&file)?.links != 1 || !secure::unchanged(&file, &snapshot, 65_536)? {
        return Err(denied(
            "Lifecycle source must be a stable single-link regular file",
        ));
    }
    Ok(Some(snapshot))
}

pub async fn run_pre_plan<'a>(
    lease: &'a backend::RootLease,
    approval: &backend::Approval,
) -> Result<PrePlanReport<'a>, Error> {
    let report = run_owner(backend::Owner::PrePlan(lease), "pre-plan", approval).await?;
    Ok(PrePlanReport { lease, report })
}

pub async fn run(
    boundary: &tools::Boundary,
    event: &str,
    approval: &backend::Approval,
) -> Result<Report, Error> {
    if event == "pre-plan" {
        return Err(denied(
            "Pre-plan hooks require the root lease interface before TASK authority",
        ));
    }
    run_owner(backend::Owner::Execution(boundary), event, approval).await
}

async fn run_owner(
    owner: backend::Owner<'_>,
    event: &str,
    approval: &backend::Approval,
) -> Result<Report, Error> {
    approval.validate()?;
    owner.check()?;
    if !["pre-plan", "post-coder", "pre-publish"].contains(&event) {
        return Err(Error::type_error("Unknown lifecycle hook event"));
    }
    let manifest = ".roster/hooks.yml";
    let current = read_hook(owner.root(), manifest, true)?;
    let Some(manifest_snapshot) = current else {
        return Ok(Report {
            identity: owner.identity(),
            root: paths::file_information(owner.root())?.identity,
            authority: owner.authority(),
            stamp: None,
            pass: true,
            value: Value::object(vec![
                ("status", Value::string("none")),
                ("pass", Value::Boolean(true)),
                ("event", Value::string(event)),
                ("entries", Value::Array(Vec::new())),
                ("reasons", Value::Array(Vec::new())),
            ]),
        });
    };
    let hooks = policy::parse_hooks(&Text::from(
        tools::bytes_text(&manifest_snapshot.bytes)?.as_str(),
    ))?;
    let hooks = domain::items(&hooks)
        .ok_or_else(|| Error::type_error("Hook parser did not return entries"))?;
    let head = tools::bytes_text(
        &owner
            .git(
                owner.root(),
                &["rev-parse", "--verify", "HEAD"].map(str::to_owned),
                false,
            )
            .await?,
    )?;
    if !regex(r"^[a-f0-9]{40}(?:[a-f0-9]{24})?$")?.is_match(head.trim()) {
        return Err(denied(
            "Trusted lifecycle source requires a captured commit object",
        ));
    }
    let mut trusted = vec![(manifest.to_owned(), manifest_snapshot)];
    for hook in hooks {
        let path = text(required(hook, "script")?, "Lifecycle script must be text")?;
        if trusted.iter().any(|(old, _)| *old == path) {
            continue;
        }
        trusted.push((
            path.clone(),
            read_hook(owner.root(), &path, false)?
                .ok_or_else(|| denied("Required lifecycle script vanished"))?,
        ));
    }
    for (path, snapshot) in &trusted {
        let committed = owner
            .git(
                owner.root(),
                &["show".to_owned(), format!("{}:{path}", head.trim())],
                false,
            )
            .await
            .map_err(|error| {
                let code = error.code;
                let wrapped = Error::new(
                    "LifecycleHookError",
                    format!("Lifecycle source must be committed at captured HEAD: {path}"),
                )
                .caused_by(error);
                match code {
                    Some(code) => wrapped.coded(code),
                    None => wrapped,
                }
            })?;
        if committed.len() > 65_536
            || tools::bytes_text(&committed)?.replace("\r\n", "\n")
                != tools::bytes_text(&snapshot.bytes)?.replace("\r\n", "\n")
        {
            return Err(denied(format!(
                "Lifecycle source differs from reviewed committed HEAD: {path}"
            )));
        }
    }
    let selected = hooks
        .iter()
        .filter(|hook| domain::is_text(hook.get("event"), event))
        .collect::<Vec<_>>();
    let mut entries = Vec::new();
    let mut reasons = Vec::new();
    if !selected.is_empty() {
        let node = approval.hooks_node.as_deref().ok_or_else(|| {
            unsupported("Selected lifecycle hook requires explicit host-approved Node executable")
        })?;
        counterfactual::node_version(owner, node).await?;
        for hook in selected {
            owner.check()?;
            for (path, expected) in &trusted {
                if paths::file_information(&owner.root().join(path))?.links != 1
                    || !secure::unchanged(&owner.root().join(path), expected, 65_536)?
                {
                    return Err(denied("Reviewed lifecycle source changed before execution"));
                }
            }
            let source = backend::Stamp::capture(owner).await?;
            let path = text(required(hook, "script")?, "Lifecycle script must be text")?;
            let timeout = domain::safe_integer(hook.get("timeout_ms"))
                .ok_or_else(|| Error::type_error("Lifecycle timeout is missing"))?;
            let started = Instant::now();
            let result = owner
                .run(
                    owner.root(),
                    node,
                    std::slice::from_ref(&path),
                    &std::collections::BTreeMap::new(),
                    timeout,
                    4096,
                )
                .await?;
            let changed = source.require_current(owner).await;
            owner.token().check()?;
            let status = if changed.is_err() {
                "mutation"
            } else if result.timed_out {
                "timeout"
            } else if result.overflow {
                "output-limit"
            } else if result.success() {
                "pass"
            } else {
                "fail"
            };
            let output = crate::planning::memory::redact(&result.text()?, owner.redaction());
            let output = output.to_utf8()?;
            let mut start = output.len().saturating_sub(4096);
            while !output.is_char_boundary(start) {
                start += 1;
            }
            let output = Text::from(
                output
                    .get(start..)
                    .ok_or_else(|| Error::type_error("Bounded hook output suffix is invalid"))?,
            );
            entries.push(Value::object(vec![
                ("script", Value::string(&path)),
                ("status", Value::string(status)),
                (
                    "exit_code",
                    result
                        .exit
                        .map_or(Value::Null, |exit| Value::Number(f64::from(exit))),
                ),
                (
                    "duration_ms",
                    Value::Number(
                        started
                            .elapsed()
                            .as_millis()
                            .min(crate::MAX_SAFE_INTEGER as u128) as f64,
                    ),
                ),
                ("output", Value::String(output)),
            ]));
            if status != "pass" {
                reasons.push(Value::string(&format!(
                    "Lifecycle hook: {event} {path} {status}; no successful gate was assumed."
                )));
                break;
            }
        }
    }
    let pass = reasons.is_empty();
    let stamp = backend::Stamp::capture(owner).await?;
    Ok(Report {
        identity: owner.identity(),
        root: paths::file_information(owner.root())?.identity,
        authority: owner.authority(),
        stamp: Some(stamp),
        pass,
        value: Value::object(vec![
            (
                "status",
                Value::string(if pass {
                    if entries.is_empty() { "none" } else { "pass" }
                } else {
                    "fail"
                }),
            ),
            ("pass", Value::Boolean(pass)),
            ("event", Value::string(event)),
            ("entries", Value::Array(entries)),
            ("reasons", Value::Array(reasons)),
        ]),
    })
}
