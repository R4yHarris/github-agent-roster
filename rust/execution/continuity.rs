use std::collections::BTreeSet;

use super::*;
use crate::identity::sha256_hex;
use crate::state::redaction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapsuleVersion {
    NodeV1,
    NativeV2,
}

pub struct Workspace {
    task: Text,
    authority: String,
    root: crate::paths::FileIdentity,
    writer: std::sync::Arc<()>,
    source: Option<String>,
    snapshot: evidence::Snapshot,
    checklist: Value,
    observations: Vec<Value>,
    notices: Vec<Value>,
    next_id: u64,
    enabled: bool,
    checklist_enabled: bool,
    version: CapsuleVersion,
}

fn items(value: &Value) -> Result<&[Value], Error> {
    domain::items(value).ok_or_else(|| Error::type_error("Acceptance state must contain arrays"))
}
fn number(value: &Value, name: &str) -> Result<u64, Error> {
    domain::safe_integer(value.get(name))
        .ok_or_else(|| Error::type_error(format!("Acceptance state requires integer {name}")))
}
fn required_string(value: &Value) -> Result<Text, Error> {
    value
        .as_string()
        .cloned()
        .ok_or_else(|| Error::type_error("Acceptance state requires text"))
}

impl Workspace {
    pub fn new(
        boundary: &tools::Boundary,
        config: &crate::config::Config,
        version: CapsuleVersion,
        capsule: Option<&Value>,
        projection: &evidence::Projection,
    ) -> Result<Self, Error> {
        boundary.check()?;
        let enabled = config
            .as_value()
            .get("seat")
            .and_then(|seat| seat.get("evidence_workspace"))
            == Some(&Value::Boolean(true));
        if capsule.is_some() && !enabled {
            return Err(Error::type_error(
                "Acceptance continuation requires seat.evidence_workspace",
            ));
        }
        let snapshot = evidence::capture(boundary.root(), None, projection)?;
        let source = if enabled {
            Some(snapshot.source()?.to_owned())
        } else {
            None
        };
        let checks = if enabled {
            checklist::authoritative_checks(boundary.task())?
        } else {
            boundary.document().acceptance_checks.clone()
        };
        let mut workspace = Self {
            task: boundary.task().clone(),
            authority: boundary.authority_digest(),
            root: crate::paths::file_information(boundary.root())?.identity,
            writer: boundary.writer_identity(),
            source,
            snapshot,
            checklist: checklist::create(&checks, enabled),
            observations: Vec::new(),
            notices: Vec::new(),
            next_id: 1,
            enabled,
            checklist_enabled: enabled || boundary.document().files_allowed.len() != 1,
            version,
        };
        if let Some(capsule) = capsule {
            let source = workspace.source.as_deref().ok_or_else(|| {
                Error::type_error("Acceptance continuation requires source identity")
            })?;
            let restored = match (version, capsule.get("version")) {
                (CapsuleVersion::NodeV1, Some(Value::Number(value))) if *value == 1.0 => {
                    checklist::import_continuation(
                        capsule,
                        boundary.task(),
                        boundary.root(),
                        &checks,
                        source,
                        boundary.redaction(),
                    )?
                }
                (CapsuleVersion::NativeV2, Some(Value::Number(value))) if *value == 2.0 => {
                    import_native(capsule, boundary, &checks, source)?
                }
                _ => {
                    return Err(Error::type_error(
                        "Acceptance capsule version must be explicitly selected; implicit upgrade or downgrade is forbidden",
                    ));
                }
            };
            workspace.checklist = required(&restored, "checklist")?.clone();
            workspace.observations = items(required(&restored, "observations")?)?.to_vec();
            workspace.notices = items(required(&restored, "notices")?)?.to_vec();
            workspace.next_id = workspace
                .observations
                .iter()
                .filter_map(|entry| domain::safe_integer(entry.get("id")))
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or_else(|| Error::type_error("Observation ID overflow"))?;
        }
        Ok(workspace)
    }

    fn check(&self, boundary: &tools::Boundary) -> Result<(), Error> {
        boundary.check()?;
        if self.authority != boundary.authority_digest()
            || !std::sync::Arc::ptr_eq(&self.writer, &boundary.writer_identity())
            || self.task != *boundary.task()
            || self.root != crate::paths::file_information(boundary.root())?.identity
        {
            return Err(denied(
                "Acceptance workspace belongs to different task, authority or worktree",
            ));
        }
        Ok(())
    }

    pub fn refresh(
        &mut self,
        boundary: &tools::Boundary,
        projection: &evidence::Projection,
    ) -> Result<(), Error> {
        self.check(boundary)?;
        let snapshot = evidence::capture(boundary.root(), None, projection)?;
        let source = if self.enabled {
            Some(snapshot.source()?.to_owned())
        } else {
            None
        };
        if self.enabled && (source != self.source || !snapshot.same_source(&self.snapshot)) {
            let binding_changed = source == self.source;
            self.source = source;
            self.observations.retain(|entry| {
                !binding_changed
                    && entry
                        .get("source")
                        .and_then(Value::as_string)
                        .and_then(|source| source.to_utf8().ok())
                        .as_deref()
                        == self.source.as_deref()
            });
            self.invalidate_terminals()?;
        }
        self.snapshot = snapshot;
        Ok(())
    }

    fn invalidate_terminals(&mut self) -> Result<(), Error> {
        let mut next = items(required(&self.checklist, "items")?)?.to_vec();
        for item in &mut next {
            if !["done", "blocked"]
                .iter()
                .any(|state| domain::is_text(item.get("status"), state))
            {
                continue;
            }
            if self
                .observations
                .iter()
                .any(|observation| observation.get("id") == item.get("evidenceRef"))
            {
                continue;
            }
            let notice = Value::string(&format!(
                "Check {}: terminal evidence invalidated by changed source or missing observation.",
                number(item, "id")?
            ));
            if !self.notices.contains(&notice) {
                self.notices.push(notice);
            }
            domain::put(item, "status", Value::string("pending"))?;
            domain::put(item, "evidence", Value::string(""))?;
            domain::put(item, "evidenceRef", Value::Null)?;
        }
        if self.notices.len() > next.len() {
            self.notices.drain(..self.notices.len() - next.len());
        }
        domain::put(&mut self.checklist, "items", Value::Array(next))
    }

    pub fn definitions(&self, boundary: &tools::Boundary) -> Value {
        let mut definitions = match boundary.definitions() {
            Value::Array(definitions) => definitions,
            _ => Vec::new(),
        };
        if self.checklist_enabled
            && boundary.role() == Role::Coder
            && boundary.recipe_permits("update_checklist")
        {
            definitions.push(checklist::definition());
        }
        Value::Array(definitions)
    }

    pub fn checklist(&self) -> &Value {
        &self.checklist
    }
    pub fn observations(&self) -> &[Value] {
        &self.observations
    }
    pub fn notices(&self) -> &[Value] {
        &self.notices
    }
    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    pub fn close_summary(&mut self, summary: &Text) -> Result<(), Error> {
        if self.enabled {
            return Err(denied(
                "Observed acceptance obligations cannot be completed from summary prose",
            ));
        }
        if self.checklist_enabled {
            self.checklist = checklist::close_from_summary(&self.checklist, summary)?;
        }
        Ok(())
    }

    pub fn require_closed(&self) -> Result<(), Error> {
        if !self.checklist_enabled {
            return Ok(());
        }
        if !checklist::open(&self.checklist)?.is_empty() {
            return Err(Error::from_text(
                "Error",
                checklist::correction(&self.checklist)?,
            ));
        }
        if items(required(&self.checklist, "items")?)?
            .iter()
            .any(|item| domain::is_text(item.get("status"), "blocked"))
        {
            return Err(Error::new(
                "Error",
                "Acceptance checklist contains blocked obligations",
            ));
        }
        Ok(())
    }

    pub fn render_for(
        &self,
        boundary: &tools::Boundary,
        projection: &evidence::Projection,
    ) -> Result<Text, Error> {
        self.check(boundary)?;
        let current = evidence::capture(boundary.root(), None, projection)?;
        if self.enabled
            && (current.source()? != self.source.as_deref().unwrap_or("")
                || !current.same_source(&self.snapshot))
        {
            return Err(denied(
                "Acceptance workspace source changed; refresh before context hydration",
            ));
        }
        let mut body = Text::from(
            "Host-validated TASK obligations; prose is not acceptance evidence. Rerun checks after changes.\n",
        );
        body.0.extend(
            if self.enabled {
                checklist::render_acceptance(&self.checklist, &self.observations)?
            } else {
                checklist::render(&self.checklist)?
            }
            .0,
        );
        for entry in &self.observations {
            body.0.push(10);
            body.0.extend(checklist::reference_evidence(entry)?.0);
        }
        for notice in &self.notices {
            body.0.push(10);
            body.0.extend(required_string(notice)?.0);
        }
        Ok(body)
    }

    fn update(&mut self, boundary: &tools::Boundary, args: &Value) -> Result<Value, Error> {
        if !self.checklist_enabled
            || boundary.role() != Role::Coder
            || !boundary.recipe_permits("update_checklist")
        {
            return Err(denied(
                "Checklist updates are unavailable in this role or recipe",
            ));
        }
        fields(
            args,
            &["items"],
            &[],
            "update_checklist requires only items",
        )?;
        for item in items(required(args, "items")?)? {
            fields(
                item,
                &["id", "status"],
                &["evidence"],
                "Checklist calls accept only id, status and evidence; observation IDs are host-owned",
            )?;
        }
        let references = if self.enabled {
            Some(checklist::update_references(
                args,
                &self.observations,
                self.source.as_deref().unwrap_or(""),
            )?)
        } else {
            None
        };
        let mut updated = checklist::update(&self.checklist, args)?;
        if let Some(references) = references {
            let mut next = items(required(&updated, "items")?)?.to_vec();
            for (update, reference) in items(required(args, "items")?)?
                .iter()
                .zip(items(&references)?)
            {
                let id = number(update, "id")?;
                let item = next
                    .iter_mut()
                    .find(|item| domain::safe_integer(item.get("id")) == Some(id))
                    .ok_or_else(|| Error::type_error("Unknown checklist ID"))?;
                domain::put(item, "evidenceRef", reference.clone())?;
                if *reference == Value::Null {
                    domain::put(item, "evidence", Value::string(""))?;
                }
            }
            domain::put(&mut updated, "items", Value::Array(next))?;
        }
        self.checklist = updated;
        let mut message = Text::from("Checklist updated.\n");
        message.0.extend(checklist::render(&self.checklist)?.0);
        Ok(Value::String(message))
    }

    pub async fn execute(
        &mut self,
        boundary: &mut tools::Boundary,
        name: &str,
        args: &Value,
        projection: &evidence::Projection,
    ) -> Result<Value, Error> {
        self.refresh(boundary, projection)?;
        if name == "update_checklist" {
            return self.update(boundary, args);
        }
        let test_alias = name == "run_command"
            && args.get("argv")
                == Some(&Value::Array(vec![
                    Value::string("node"),
                    Value::string("--test"),
                ]));
        let observed_name = if test_alias { "run_test" } else { name };
        if self.enabled && observed_name == "run_test" {
            self.observations
                .retain(|entry| !domain::is_text(entry.get("tool"), "run_test"));
            self.invalidate_terminals()?;
        }
        let result = boundary.execute(name, args).await?;
        self.record(boundary, observed_name, args, result, projection)
    }

    pub async fn verify_full(
        &mut self,
        boundary: &mut tools::Boundary,
        projection: &evidence::Projection,
    ) -> Result<Value, Error> {
        self.refresh(boundary, projection)?;
        if self.enabled {
            self.observations
                .retain(|entry| !domain::is_text(entry.get("tool"), "run_test"));
            self.invalidate_terminals()?;
        }
        let result = boundary.verify(true).await?;
        self.record(
            boundary,
            "run_test",
            &Value::object(Vec::new()),
            result,
            projection,
        )
    }

    fn record(
        &mut self,
        boundary: &tools::Boundary,
        name: &str,
        args: &Value,
        result: Value,
        projection: &evidence::Projection,
    ) -> Result<Value, Error> {
        self.refresh(boundary, projection)?;
        if !self.enabled
            || ![
                "read_file",
                "write_file",
                "edit_file",
                "delete_file",
                "run_test",
            ]
            .contains(&name)
        {
            return Ok(result);
        }
        if self.next_id > crate::MAX_SAFE_INTEGER {
            return Err(Error::type_error(
                "Acceptance observation IDs exceeded safe bounds",
            ));
        }
        if name == "run_test" {
            let proof = boundary.last_tests().ok_or_else(|| {
                denied("run_test did not produce privately sealed observed evidence")
            })?;
            proof.require_current_observation(boundary)?;
            if proof.observed_skipped() {
                let notice = Value::string("Skipped tests are not observed acceptance evidence.");
                if self.notices.len() < items(required(&self.checklist, "items")?)?.len() {
                    self.notices.push(notice);
                }
                return Ok(result);
            }
        }
        let mut observation = if self.version == CapsuleVersion::NodeV1 {
            checklist::observation(
                &Value::object(vec![
                    ("id", Value::Number(self.next_id as f64)),
                    ("tool", Value::string(name)),
                    ("args", args.clone()),
                    ("result", result.clone()),
                    (
                        "source",
                        Value::string(self.source.as_deref().unwrap_or("")),
                    ),
                ]),
                boundary.redaction(),
            )?
        } else {
            self.native_observation(boundary, name, args, &result)?
        };
        if let Some(reference) = observation.get("reference").and_then(Value::as_string) {
            let safe = redaction::evidence(reference, boundary.redaction());
            domain::put(&mut observation, "reference", Value::String(safe))?;
        }
        self.next_id += 1;
        self.observations.push(observation);
        if self.observations.len() > 32 {
            self.observations.remove(0);
            self.invalidate_terminals()?;
        }
        self.check(boundary)?;
        Ok(result)
    }

    fn native_observation(
        &self,
        boundary: &tools::Boundary,
        name: &str,
        args: &Value,
        result: &Value,
    ) -> Result<Value, Error> {
        let verification = if name == "run_test" {
            Some(
                boundary
                    .last_tests()
                    .ok_or_else(|| denied("Missing observed checks"))?
                    .observation_contract(),
            )
        } else {
            None
        };
        let reference = if let Some(verification) = &verification {
            engine_reference(items(required(verification, "observed")?)?)
        } else {
            result
                .get("path")
                .or_else(|| args.get("path"))
                .map(|value| text(value, "Observation path must be text"))
                .transpose()?
                .unwrap_or_default()
        };
        if reference.is_empty() || reference.encode_utf16().count() > 300 {
            return Err(Error::type_error(
                "Acceptance observation reference exceeds bounds",
            ));
        }
        Ok(Value::object(vec![
            ("id", Value::Number(self.next_id as f64)),
            ("tool", Value::string(name)),
            ("reference", Value::string(&reference)),
            (
                "source",
                Value::string(self.source.as_deref().unwrap_or("")),
            ),
            (
                "verdict",
                Value::string(if name == "run_test" {
                    if boundary
                        .last_tests()
                        .is_some_and(|proof| proof.observed_exit() == 0)
                    {
                        "pass"
                    } else {
                        "fail"
                    }
                } else {
                    "observed"
                }),
            ),
            ("verification", verification.unwrap_or(Value::Null)),
        ]))
    }

    pub fn export(
        &self,
        boundary: &tools::Boundary,
        outcome: &str,
        projection: &evidence::Projection,
    ) -> Result<Value, Error> {
        self.check(boundary)?;
        if !self.enabled {
            return Ok(Value::Null);
        }
        let current = evidence::capture(boundary.root(), None, projection)?;
        if current.source()? != self.source.as_deref().unwrap_or("")
            || !current.same_source(&self.snapshot)
        {
            return Err(denied(
                "Acceptance source changed before continuation export; refresh and rerun required evidence",
            ));
        }
        if outcome == "verified" {
            self.require_closed()?;
            if boundary.checks_required()? {
                let proof = boundary.last_tests().ok_or_else(|| {
                    denied("Verified continuation requires actual privately sealed checks")
                })?;
                proof.require_current_observation(boundary)?;
                proof.require_current_coverage(boundary)?;
                if proof.observed_exit() != 0 || proof.observed_skipped() {
                    return Err(denied(
                        "Failed or skipped checks cannot verify acceptance continuation",
                    ));
                }
            }
        }
        let mut capsule = checklist::export_continuation(
            &Value::object(vec![
                ("checklist", self.checklist.clone()),
                ("observations", Value::Array(self.observations.clone())),
                ("priorNotices", Value::Array(self.notices.clone())),
                (
                    "source",
                    Value::string(self.source.as_deref().unwrap_or("")),
                ),
                ("outcome", Value::string(outcome)),
            ]),
            boundary.task(),
            boundary.root(),
        )?;
        if self.version == CapsuleVersion::NativeV2 {
            domain::put(&mut capsule, "version", Value::Number(2.0))?;
        }
        Ok(capsule)
    }
}

pub(crate) fn next_context(
    boundary: &tools::Boundary,
    config: &crate::config::Config,
    capsule: Option<&Value>,
    unmet: &[u64],
    projection: &evidence::Projection,
) -> Result<Option<Value>, Error> {
    let mut workspace = Workspace::new(
        boundary,
        config,
        CapsuleVersion::NativeV2,
        capsule,
        projection,
    )?;
    if !workspace.enabled {
        return Ok(None);
    }
    if capsule.is_none() {
        return Err(denied(
            "Current acceptance continuation is missing; disk receipts cannot replace it",
        ));
    }
    let mut obligations = items(required(&workspace.checklist, "items")?)?.to_vec();
    for id in unmet {
        if !obligations
            .iter()
            .any(|item| domain::safe_integer(item.get("id")) == Some(*id))
        {
            return Err(denied(
                "Independent review named a nonauthoritative acceptance ID",
            ));
        }
    }
    workspace.observations.retain(|entry| {
        !domain::is_text(entry.get("tool"), "run_test")
            || !domain::is_text(entry.get("verdict"), "pass")
    });
    for item in &mut obligations {
        let id = number(item, "id")?;
        if unmet.contains(&id) || domain::is_text(item.get("status"), "done") {
            domain::put(item, "status", Value::string("pending"))?;
            domain::put(item, "evidence", Value::string(""))?;
            domain::put(item, "evidenceRef", Value::Null)?;
            workspace.notices.push(Value::string(&format!(
                "Check {id}: {}.",
                if unmet.contains(&id) {
                    "reopened by independent review"
                } else {
                    "positive evidence revoked for fresh context"
                }
            )));
        }
    }
    if workspace.notices.len() > obligations.len() {
        workspace
            .notices
            .drain(..workspace.notices.len() - obligations.len());
    }
    domain::put(&mut workspace.checklist, "items", Value::Array(obligations))?;
    workspace.export(boundary, "failed", projection).map(Some)
}

fn engine_reference(observed: &[Value]) -> String {
    let cargo = observed
        .iter()
        .any(|engine| domain::is_text(Some(engine), "cargo"));
    let node = observed
        .iter()
        .any(|engine| domain::is_text(Some(engine), "node"));
    if cargo {
        if node {
            "cargo and node checks"
        } else {
            "cargo checks"
        }
    } else {
        "node --test"
    }
    .to_owned()
}

fn validate_verification(verification: &Value, observation: &Value) -> Result<(), Error> {
    fields(
        verification,
        &[
            "required",
            "observed",
            "commands_digest",
            "command_count",
            "cargo_version",
        ],
        &[],
        "Native verification reference is malformed",
    )?;
    let required_engines = items(required(verification, "required")?)?;
    let observed = items(required(verification, "observed")?)?;
    let mut required_names = BTreeSet::new();
    let mut observed_names = BTreeSet::new();
    for (engines, names) in [
        (required_engines, &mut required_names),
        (observed, &mut observed_names),
    ] {
        if engines.is_empty() || engines.len() > 2 {
            return Err(Error::type_error(
                "Native verification must name actual required and observed engines",
            ));
        }
        for engine in engines {
            let engine = text(engine, "Verification engine must be text")?;
            if !["node", "cargo"].contains(&engine.as_str()) || !names.insert(engine) {
                return Err(Error::type_error(
                    "Native verification engines must be distinct and supported",
                ));
            }
        }
    }
    let command_count = number(verification, "command_count")?;
    let minimum = if observed_names.contains("cargo") {
        3
    } else {
        0
    } + u64::from(observed_names.contains("node"));
    if command_count == 0
        || command_count > MAX_FILES as u64 + 4
        || !regex(r"^[a-f0-9]{64}$")?.is_match(&text(
            required(verification, "commands_digest")?,
            "Command digest must be text",
        )?)
        || !["pass", "fail"]
            .iter()
            .any(|verdict| domain::is_text(observation.get("verdict"), verdict))
        || !domain::is_text(observation.get("reference"), &engine_reference(observed))
        || domain::is_text(observation.get("verdict"), "pass")
            && (!required_names.is_subset(&observed_names) || command_count < minimum)
    {
        return Err(Error::type_error(
            "Native verification reference lacks actual engine evidence",
        ));
    }
    if observed_names.contains("cargo") {
        let version = text(
            required(verification, "cargo_version")?,
            "Observed Cargo version must be text",
        )?;
        if version.len() > 1024 || !regex(r"^cargo [0-9]+\.[0-9]+\.[0-9]+")?.is_match(&version) {
            return Err(Error::type_error(
                "Native verification needs an observed Cargo version",
            ));
        }
    } else if verification.get("cargo_version") != Some(&Value::Null) {
        return Err(Error::type_error(
            "Node verification cannot claim a Cargo version",
        ));
    }
    Ok(())
}

fn import_native(
    capsule: &Value,
    boundary: &tools::Boundary,
    checks: &[Text],
    source: &str,
) -> Result<Value, Error> {
    fields(
        capsule,
        &[
            "version",
            "task",
            "worktree",
            "source",
            "outcome",
            "items",
            "observations",
            "notices",
        ],
        &[],
        "Native acceptance capsule is malformed",
    )?;
    if capsule.get("version") != Some(&Value::Number(2.0))
        || capsule.json().encode_utf16().count() > 65_536
        || !["verified", "failed", "cancelled"]
            .iter()
            .any(|outcome| domain::is_text(capsule.get("outcome"), outcome))
    {
        return Err(Error::type_error("Native acceptance capsule is malformed"));
    }
    let task = sha256_hex(boundary.task().to_utf8()?.as_bytes());
    let path = crate::paths::absolute(boundary.root())?;
    let owner = sha256_hex(
        path.to_str()
            .ok_or_else(|| Error::type_error("Acceptance owner must be Unicode"))?
            .as_bytes(),
    );
    if !domain::is_text(capsule.get("task"), &task)
        || !domain::is_text(capsule.get("worktree"), &owner)
        || !domain::is_text(capsule.get("source"), source)
    {
        return Err(denied(
            "Native acceptance capsule task, owner or current source mismatch",
        ));
    }
    let observed = items(required(capsule, "observations")?)?;
    let obligations = items(required(capsule, "items")?)?;
    let notices = items(required(capsule, "notices")?)?;
    if observed.len() > 32 || obligations.len() > checks.len() || notices.len() > checks.len() {
        return Err(Error::type_error(
            "Native acceptance capsule exceeds bounded references",
        ));
    }
    let mut ids = BTreeSet::new();
    for observation in observed {
        fields(
            observation,
            &[
                "id",
                "tool",
                "reference",
                "source",
                "verdict",
                "verification",
            ],
            &[],
            "Native observation is malformed",
        )?;
        let id = number(observation, "id")?;
        let tool = text(
            required(observation, "tool")?,
            "Observation tool must be text",
        )?;
        let reference = required_string(required(observation, "reference")?)?;
        if id == 0
            || !ids.insert(id)
            || ![
                "read_file",
                "write_file",
                "edit_file",
                "delete_file",
                "run_test",
            ]
            .contains(&tool.as_str())
            || reference.0.is_empty()
            || reference.0.len() > 300
            || !domain::is_text(observation.get("source"), source)
        {
            return Err(Error::type_error("Native observation reference is invalid"));
        }
        let verification = required(observation, "verification")?;
        if tool == "run_test" {
            validate_verification(verification, observation)?;
        } else if *verification != Value::Null
            || !domain::is_text(observation.get("verdict"), "observed")
        {
            return Err(Error::type_error(
                "Non-test observation cannot claim verification",
            ));
        }
    }
    let mut checklist = checklist::create(checks, true);
    let mut next = items(required(&checklist, "items")?)?.to_vec();
    let mut seen = BTreeSet::new();
    for obligation in obligations {
        fields(
            obligation,
            &["id", "check", "status", "evidence", "evidenceRef"],
            &[],
            "Native acceptance obligation is malformed",
        )?;
        let id = number(obligation, "id")?;
        let item = next
            .iter_mut()
            .find(|item| domain::safe_integer(item.get("id")) == Some(id))
            .ok_or_else(|| Error::type_error("Native obligation ID is not authoritative"))?;
        if !seen.insert(id)
            || item.get("check") != obligation.get("check")
            || !["pending", "in_progress", "done", "blocked"]
                .iter()
                .any(|status| domain::is_text(obligation.get("status"), status))
        {
            return Err(Error::type_error(
                "Native obligation changes authoritative checks",
            ));
        }
        if ["done", "blocked"]
            .iter()
            .any(|status| domain::is_text(obligation.get("status"), status))
        {
            let observation = observed
                .iter()
                .find(|entry| entry.get("id") == obligation.get("evidenceRef"))
                .ok_or_else(|| Error::type_error("Native obligation lacks an observation"))?;
            if obligation.get("evidence")
                != Some(&Value::String(checklist::reference_evidence(observation)?))
                || domain::is_text(obligation.get("status"), "done")
                    && (!domain::is_text(capsule.get("outcome"), "verified")
                        || domain::is_text(observation.get("verdict"), "fail"))
                || domain::is_text(obligation.get("status"), "blocked")
                    && !domain::is_text(observation.get("verdict"), "fail")
            {
                return Err(Error::type_error(
                    "Native terminal obligation has invalid evidence",
                ));
            }
        } else if obligation.get("evidenceRef") != Some(&Value::Null)
            || !domain::is_text(obligation.get("evidence"), "")
        {
            return Err(Error::type_error(
                "Open native obligation has terminal evidence",
            ));
        }
        *item = obligation.clone();
    }
    if next
        .iter()
        .filter(|item| domain::is_text(item.get("status"), "in_progress"))
        .count()
        > 1
    {
        return Err(Error::type_error(
            "Only one native obligation may be in progress",
        ));
    }
    domain::put(&mut checklist, "items", Value::Array(next))?;
    for notice in notices {
        if required_string(notice)?.0.len() > 300 {
            return Err(Error::type_error("Native acceptance notice exceeds bounds"));
        }
    }
    let mut safe_observations = observed.to_vec();
    for observation in &mut safe_observations {
        let reference = required_string(required(observation, "reference")?)?;
        domain::put(
            observation,
            "reference",
            Value::String(redaction::evidence(&reference, boundary.redaction())),
        )?;
    }
    let mut safe_items = items(required(&checklist, "items")?)?.to_vec();
    for item in &mut safe_items {
        if let Some(observation) = safe_observations
            .iter()
            .find(|entry| entry.get("id") == item.get("evidenceRef"))
        {
            domain::put(
                item,
                "evidence",
                Value::String(checklist::reference_evidence(observation)?),
            )?;
        }
    }
    domain::put(&mut checklist, "items", Value::Array(safe_items))?;
    Ok(Value::object(vec![
        ("checklist", checklist),
        ("observations", Value::Array(safe_observations)),
        (
            "notices",
            Value::Array(
                notices
                    .iter()
                    .map(|notice| {
                        required_string(notice).map(|notice| {
                            Value::String(redaction::evidence(&notice, boundary.redaction()))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        ),
    ]))
}
