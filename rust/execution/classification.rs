use std::path::Path;

use super::*;
use counterfactual::{Engine, Target, Verdict};

pub struct Report {
    stamp: backend::Stamp,
    observation: std::sync::Arc<()>,
    value: Value,
    outside_proven: bool,
}

impl Report {
    pub fn as_value(&self) -> &Value {
        &self.value
    }
    pub fn all_outside_proven(&self) -> bool {
        self.outside_proven
    }
    pub(crate) fn matches_observation(&self, proof: &tools::TestEvidence) -> bool {
        std::sync::Arc::ptr_eq(&self.observation, &proof.observation_identity())
    }
    pub async fn require_current(&self, boundary: &tools::Boundary) -> Result<(), Error> {
        self.stamp
            .require_current(backend::Owner::Execution(boundary))
            .await
    }
}

fn failure_path(candidate: &str, root: &Path) -> Result<Option<String>, Error> {
    let absolute = if candidate.starts_with("file:") {
        match url::Url::parse(candidate)
            .ok()
            .and_then(|value| value.to_file_path().ok())
        {
            Some(path) => path,
            None => return Ok(None),
        }
    } else {
        crate::paths::resolve(root, Path::new(candidate))?
    };
    let Ok(relative) = absolute.strip_prefix(root) else {
        return Ok(None);
    };
    let file = relative
        .to_str()
        .ok_or_else(|| Error::type_error("Failure path must be Unicode"))?
        .replace('\\', "/");
    Ok(policy::repair_test_file(&file)?.then_some(file))
}

struct NodeFailure {
    name: Option<Text>,
    file: Option<String>,
    supported_kind: Option<bool>,
    location_seen: bool,
}

struct NodeFailures {
    entries: Vec<NodeFailure>,
    complete: bool,
}

fn node_failures(output: &Text, root: &Path, commands: usize) -> Result<NodeFailures, Error> {
    let output = output.to_utf8()?;
    let row = regex(r"^(\s*)(not ok|ok)\b")?;
    let named = regex(r"^\s*not ok [0-9]+ - (.*?)(?: # (SKIP|TODO)\b.*)?$")?;
    let location = regex(r#"^\s*location:\s*['"]?(.+?):\d+:\d+['"]?\s*$"#)?;
    let failures = regex(r"^# fail ([0-9]+)\s*$")?;
    let cancelled = regex(r"^# cancelled ([0-9]+)\s*$")?;
    let kind = regex(r#"^\s*failureType:\s*['"]?([^'"]+?)['"]?\s*$"#)?;
    let asynchronous = regex(
        r"^# (?:Error|Warning): .*?(?:generated asynchronous activity|uncaughtException|unhandledRejection)",
    )?;
    let interrupted = regex(
        r#"^\s*(?:signal:\s*['"]?SIG|failureType:\s*['"]?(?:testTimeoutFailure|testAborted|cancelledByParent))"#,
    )?;
    let mut entries: Vec<NodeFailure> = Vec::new();
    let mut pending = None;
    let mut summaries = 0_usize;
    let mut total = 0_usize;
    let mut complete = true;
    for line in output.lines() {
        if let Some(capture) = row.captures(line) {
            pending = None;
            if capture
                .get(2)
                .is_some_and(|value| value.as_str() == "not ok")
            {
                let name = named.captures(line);
                if name
                    .as_ref()
                    .is_some_and(|capture| capture.get(2).is_some())
                {
                    continue;
                }
                if entries.len() >= MAX_FILES {
                    return Err(unsupported(
                        "Observed Node failure inventory exceeds its bound",
                    ));
                }
                let indent = capture.get(1).map_or(0, |value| value.as_str().len());
                let name = name.and_then(|capture| {
                    capture.get(1).map(|name| {
                        Text::from(
                            name.as_str()
                                .replace("\\#", "#")
                                .replace("\\\\", "\\")
                                .as_str(),
                        )
                    })
                });
                pending = Some((entries.len(), indent));
                entries.push(NodeFailure {
                    name,
                    file: None,
                    supported_kind: None,
                    location_seen: false,
                });
            }
            continue;
        }
        if let Some(capture) = failures.captures(line) {
            let count = capture
                .get(1)
                .map_or("", |value| value.as_str())
                .parse::<usize>()
                .map_err(|_| {
                    Error::type_error("Node failure count exceeds bounded integer input")
                })?;
            total = total.checked_add(count).ok_or_else(|| {
                Error::type_error("Node failure count exceeds bounded integer input")
            })?;
            summaries += 1;
        }
        if let Some(capture) = cancelled.captures(line) {
            complete &= capture.get(1).is_some_and(|value| value.as_str() == "0");
        }
        if asynchronous.is_match(line) {
            complete = false;
        }
        if let Some((index, indent)) = pending {
            if line.len() - line.trim_start_matches([' ', '\t']).len() != indent + 2 {
                continue;
            }
            if interrupted.is_match(line) {
                complete = false;
            }
            if let Some(capture) = kind.captures(line) {
                let entry = entries
                    .get_mut(index)
                    .ok_or_else(|| denied("Observed failure inventory lost its kind record"))?;
                if entry.supported_kind.is_some() {
                    complete = false;
                }
                entry.supported_kind = Some(capture.get(1).is_some_and(|value| {
                    ["testCodeFailure", "subtestsFailed"].contains(&value.as_str())
                }));
            }
            if let Some(candidate) = location.captures(line).and_then(|capture| capture.get(1)) {
                let file = failure_path(candidate.as_str(), root)?;
                let entry = entries
                    .get_mut(index)
                    .ok_or_else(|| denied("Observed failure inventory lost its record"))?;
                if entry.location_seen {
                    complete = false;
                }
                entry.location_seen = true;
                entry.file = file;
            }
        }
    }
    complete &= commands != 0 && summaries == commands && total != 0 && total <= entries.len();
    complete &= entries
        .iter()
        .all(|entry| entry.supported_kind == Some(true));
    Ok(NodeFailures { entries, complete })
}

pub fn failing_test_paths(output: &Text, root: &Path) -> Result<Vec<String>, Error> {
    let output = output.to_utf8()?;
    let mut files = Vec::new();
    let mut failed = false;
    let tap = regex(r"^\s*(?:not )?ok\b")?;
    let location = regex(r#"^\s*location:\s*['"]?(.+?):\d+:\d+['"]?\s*$"#)?;
    let at = regex(r"^\s*test at (.+):\d+:\d+\s*$")?;
    for line in output.lines() {
        if tap.is_match(line) {
            failed = regex(r"^\s*not ok\b")?.is_match(line);
        }
        let capture = at
            .captures(line)
            .or_else(|| failed.then(|| location.captures(line)).flatten());
        let Some(candidate) = capture.as_ref().and_then(|capture| capture.get(1)) else {
            continue;
        };
        let Some(file) = failure_path(candidate.as_str(), root)? else {
            continue;
        };
        if !files.contains(&file) {
            files.push(file);
        }
    }
    Ok(files)
}

fn data(
    status: &str,
    original_exit: i32,
    regression: &[String],
    preexisting: &[String],
    transient: &[String],
    unclassified: &[String],
) -> Value {
    Value::object(vec![
        ("status", Value::string(status)),
        ("original_exit", Value::Number(f64::from(original_exit))),
        ("regression_files", strings(regression)),
        ("preexisting_files", strings(preexisting)),
        ("transient_files", strings(transient)),
        ("unclassified_files", strings(unclassified)),
        ("clean_full_suite", Value::Boolean(false)),
    ])
}

async fn node_setup_control(
    owner: backend::Owner<'_>,
    approval: &backend::Approval,
    scratch: &scratch::Scratch<'_>,
    directory: &Path,
    target: &Target,
    base: bool,
) -> Result<bool, Error> {
    let _scope = owner.span(timing::Label::ConsumerSetup);
    if target.engine != Engine::Node {
        return Ok(true);
    }
    let node = approval.node.as_deref().ok_or_else(|| {
        unsupported("Node setup control requires explicit host executable approval")
    })?;
    let home = scratch.directory(if base { "ib" } else { "ic" })?;
    let run = owner
        .run(
            directory,
            node,
            &[
                "--test".to_owned(),
                "--test-reporter=tap".to_owned(),
                format!("--test-timeout={}", approval.timeout()),
                "--test-name-pattern=(?!)".to_owned(),
                target.file.clone(),
            ],
            &counterfactual::environment(&home),
            approval.timeout(),
            evidence::MAX_DIFF_BYTES,
        )
        .await?;
    Ok(run.success())
}

pub async fn run(
    boundary: &tools::Boundary,
    approval: &backend::Approval,
) -> Result<Report, Error> {
    approval.validate()?;
    let owner = backend::Owner::Execution(boundary);
    let prepare_scope = owner.span(timing::Label::ClassPrepare);
    let proof = boundary
        .last_tests()
        .ok_or_else(|| denied("Classification requires privately sealed actual failed checks"))?;
    proof.require_current_observation(boundary)?;
    if proof.observed_exit() == 0 || proof.observed_skipped() {
        return Err(denied(
            "Classification cannot invent failures from passing or skipped checks",
        ));
    }
    let initial = backend::Run {
        exit: Some(proof.observed_exit()),
        stdout: proof
            .result()
            .get("stdout")
            .and_then(Value::as_string)
            .cloned()
            .unwrap_or_default()
            .to_utf8()?
            .into_bytes(),
        stderr: proof
            .result()
            .get("stderr")
            .and_then(Value::as_string)
            .cloned()
            .unwrap_or_default()
            .to_utf8()?
            .into_bytes(),
        timed_out: false,
        overflow: false,
    };
    let stamp = backend::Stamp::capture(owner).await?;
    let observation = proof.observation_identity();
    if !approval.classifications {
        return Ok(Report {
            stamp,
            observation,
            value: data(
                "unavailable",
                proof.observed_exit(),
                &[],
                &[],
                &[],
                &["classification host approval is missing".to_owned()],
            ),
            outside_proven: false,
        });
    }
    let changes = evidence::changes(boundary, None).await?;
    let mut targets: Vec<Target> = Vec::new();
    let mut unmapped = Vec::new();
    let node_commands = proof
        .observed_commands()
        .iter()
        .filter(|command| command.starts_with("node --test"))
        .count();
    let observed = node_failures(
        &Text::from(tools::bytes_text(&initial.stdout)?.as_str()),
        owner.root(),
        node_commands,
    )?;
    if node_commands != 0 && !observed.complete {
        unmapped.push("Node failed output does not provide complete uncancelled failure coverage for every observed command".to_owned());
    }
    let paths = failing_test_paths(&initial.text()?, owner.root())?;
    for file in paths {
        if node_commands != 0 && !observed.complete {
            unmapped.push(file.clone());
        }
        let source = evidence::read_product(owner.root(), &file, MAX_FILE_BYTES)?
            .ok_or_else(|| denied("Observed failing test file vanished"))?;
        let mut names = Vec::new();
        let declared = substance::counterfactual_names(&file, &source)?;
        for name in &declared {
            if counterfactual::verdict(&initial, name, Engine::Node)? == Verdict::Fail
                && !names.contains(name)
            {
                if declared.iter().filter(|other| *other == name).count() != 1 {
                    unmapped.push(file.clone());
                }
                names.push(name.clone());
            }
        }
        if !names.is_empty() {
            targets.push(Target {
                file,
                names,
                engine: Engine::Node,
            });
        } else {
            unmapped.push(file);
        }
    }
    for entry in &observed.entries {
        let mapped = entry
            .file
            .as_ref()
            .zip(entry.name.as_ref())
            .is_some_and(|(file, name)| {
                targets
                    .iter()
                    .filter(|target| target.file == *file && target.names.contains(name))
                    .count()
                    == 1
                    && observed
                        .entries
                        .iter()
                        .filter(|other| other.name.as_ref() == Some(name))
                        .count()
                        == 1
            });
        if !mapped {
            unmapped.push(entry.file.clone().unwrap_or_else(|| {
                "Observed Node failure has no unique readable source and static name".to_owned()
            }));
        }
    }
    if targets.is_empty()
        && proof
            .observed_commands()
            .iter()
            .any(|command| command.starts_with("cargo test"))
    {
        for file in enrichment::source_files(owner.root(), "tests", MAX_FILES, false)? {
            if !file.ends_with(".rs") || file.split('/').count() != 2 {
                continue;
            }
            let Some(source) = evidence::read_product(owner.root(), &file, MAX_FILE_BYTES)? else {
                continue;
            };
            let mut names = Vec::new();
            for name in substance::counterfactual_names(&file, &source)? {
                if counterfactual::verdict(&initial, &name, Engine::Cargo)? == Verdict::Fail
                    && !names.contains(&name)
                {
                    names.push(name);
                }
            }
            if !names.is_empty() {
                targets.push(Target {
                    file,
                    names,
                    engine: Engine::Cargo,
                });
            }
        }
    }
    if targets.is_empty() {
        return Ok(Report {stamp, observation, value: data("unavailable", proof.observed_exit(), &[], &[], &[], &["No unique actual named test failure maps to supported consumer/integration sources; compiler/setup and inline Rust failures are not baseline proof.".to_owned()]), outside_proven: false});
    }
    let mut unclassified = targets
        .iter()
        .skip(8)
        .map(|target| target.file.clone())
        .collect::<Vec<_>>();
    unclassified.extend(unmapped);
    targets.truncate(8);
    for left in 0..targets.len() {
        for right in left + 1..targets.len() {
            if targets
                .get(left)
                .zip(targets.get(right))
                .is_some_and(|(a, b)| a.names.iter().any(|name| b.names.contains(name)))
            {
                return Ok(Report {
                    stamp,
                    observation,
                    value: data(
                        "unavailable",
                        proof.observed_exit(),
                        &[],
                        &[],
                        &[],
                        &["Observed test names are ambiguous across candidate files.".to_owned()],
                    ),
                    outside_proven: false,
                });
            }
        }
    }
    if targets.iter().any(|target| target.engine == Engine::Cargo)
        && changes
            .head_product(boundary, "Cargo.toml")
            .await?
            .is_none()
    {
        return Ok(Report {stamp, observation, value: data("unavailable", proof.observed_exit(), &[], &[], &[], &["Captured base has no native Cargo manifest; no candidate production or legacy framework is reconstructed.".to_owned()]), outside_proven: false});
    }
    timing::finish(prepare_scope, None);
    let only_node = targets.iter().all(|target| target.engine == Engine::Node);
    if targets.iter().any(|target| target.engine == Engine::Node) {
        counterfactual::node_version(
            owner,
            approval.node.as_deref().ok_or_else(|| {
                unsupported("Node baseline classification requires approved Node 20")
            })?,
        )
        .await?;
    }
    let scratch = scratch::Scratch::create(owner).await?;
    let outcome = async {
        let mut transient = Vec::new();
        let mut preexisting = Vec::new();
        let mut regression = Vec::new();
        let mut persistent = Vec::new();
        for target in targets {
            if unclassified.contains(&target.file) {
                continue;
            }
            let _scope = owner.span(timing::Label::ClassCandidate);
            if !node_setup_control(owner, approval, &scratch, owner.root(), &target, false).await? {
                unclassified.push(target.file);
                continue;
            }
            let at_candidate =
                counterfactual::run_one(owner, approval, &scratch, owner.root(), &target, false)
                    .await?;
            let results = target
                .names
                .iter()
                .map(|name| counterfactual::verdict(&at_candidate, name, target.engine))
                .collect::<Result<Vec<_>, _>>()?;
            if at_candidate.success() && results.iter().all(|result| *result == Verdict::Pass) {
                transient.push(target.file);
            } else if results.iter().all(|result| *result == Verdict::Fail) {
                persistent.push(target);
            } else {
                unclassified.push(target.file);
            }
        }
        if !persistent.is_empty() {
            let head = text(
                required(&changes.validation, "head")?,
                "Classification captured HEAD is missing",
            )?;
            let base = scratch.base(&head, approval).await?;
            for target in persistent {
                if changes
                    .head_product(boundary, &target.file)
                    .await?
                    .is_none()
                {
                    unclassified.push(target.file);
                    continue;
                }
                let _scope = owner.span(timing::Label::ClassBaseline);
                let source_scope = owner.span(timing::Label::ProofSource);
                let before = evidence::capture(&base, None, &evidence::Projection::default());
                timing::finish(source_scope, before.as_ref().err());
                let before = before?;
                let before_git = backend::git_signature(owner, &base).await?;
                let at_base = if node_setup_control(owner, approval, &scratch, &base, &target, true)
                    .await?
                {
                    Some(
                        counterfactual::run_one(owner, approval, &scratch, &base, &target, true)
                            .await?,
                    )
                } else {
                    None
                };
                let compare_scope = owner.span(timing::Label::ProofCompare);
                if !before.same_source(&{
                    let scope = owner.span(timing::Label::ProofSource);
                    let result = evidence::capture(&base, None, &evidence::Projection::default());
                    timing::finish(scope, result.as_ref().err());
                    result?
                }) || before_git != backend::git_signature(owner, &base).await?
                {
                    return Err(denied(
                        "Baseline classification test mutated captured base files",
                    ));
                }
                timing::finish(compare_scope, None);
                let Some(at_base) = at_base else {
                    unclassified.push(target.file);
                    continue;
                };
                let results = target
                    .names
                    .iter()
                    .map(|name| counterfactual::verdict(&at_base, name, target.engine))
                    .collect::<Result<Vec<_>, _>>()?;
                if at_base.success() && results.iter().all(|result| *result == Verdict::Pass) {
                    regression.push(target.file);
                } else if results.iter().all(|result| *result == Verdict::Fail) {
                    preexisting.push(target.file);
                } else {
                    unclassified.push(target.file);
                }
            }
        }
        let scope = boundary.scope()?;
        let outside = unclassified.is_empty()
            && regression.is_empty()
            && (!preexisting.is_empty() || !transient.is_empty());
        unclassified.sort();
        unclassified.dedup();
        let outside = proof.full_exception_eligible()
            && only_node
            && outside
            && preexisting
                .iter()
                .chain(&transient)
                .all(|file| !crate::planning::is_allowed(file, &scope));
        Ok((
            data(
                if unclassified.is_empty() {
                    "observed"
                } else {
                    "unavailable"
                },
                proof.observed_exit(),
                &regression,
                &preexisting,
                &transient,
                &unclassified,
            ),
            outside,
        ))
    }
    .await;
    let (value, outside_proven) = backend::finish_owned(owner, outcome, scratch.close().await)?;
    stamp.require_current(owner).await?;
    Ok(Report {
        stamp,
        observation,
        value,
        outside_proven,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_summaries_counts_cancellation_and_signal_data_cannot_be_complete_failure_coverage()
    -> Result<(), Error> {
        let root =
            std::env::current_dir().map_err(|error| Error::new("Error", error.to_string()))?;
        let file = url::Url::from_file_path(root.join("tests").join("outside.test.mjs"))
            .map_err(|_| Error::type_error("Owned failure URL fixture is invalid"))?;
        let valid = format!(
            "TAP version 13\nnot ok 1 - real name\n  ---\n  location: '{file}:4:1'\n  failureType: 'testCodeFailure'\n  ...\n# fail 1\n# cancelled 0\n"
        );
        let observed = node_failures(&Text::from(valid.as_str()), &root, 1)?;
        assert!(observed.complete);
        assert_eq!(observed.entries.len(), 1);
        assert_eq!(
            observed
                .entries
                .first()
                .and_then(|entry| entry.file.as_deref()),
            Some("tests/outside.test.mjs")
        );
        for invalid in [
            valid.replace("# fail 1\n", ""),
            valid.replace("# fail 1", "# fail 2"),
            valid.replace("# cancelled 0", "# cancelled 1"),
            valid.replace("failureType: 'testCodeFailure'", "signal: 'SIGTERM'"),
            valid.replace(
                "failureType: 'testCodeFailure'",
                "failureType: 'hookFailed'",
            ),
            format!(
                "{valid}# Error: A resource generated asynchronous activity after the test ended.\n"
            ),
            "TAP version 13\nnot ok 1 - real name\n".to_owned(),
        ] {
            assert!(!node_failures(&Text::from(invalid.as_str()), &root, 1)?.complete);
        }
        assert!(!node_failures(&Text::from(valid.as_str()), &root, 2)?.complete);
        Ok(())
    }
}
