use std::collections::BTreeMap;
use std::path::Path;

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Engine {
    Node,
    Cargo,
}

pub(crate) struct Target {
    pub(crate) file: String,
    pub(crate) names: Vec<Text>,
    pub(crate) engine: Engine,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    Pass,
    Fail,
    Skipped,
    Missing,
}

impl Verdict {
    fn label(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Skipped => "skipped",
            Self::Missing => "not run",
        }
    }
}

pub struct Report {
    stamp: backend::Stamp,
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
    pub async fn require_current(&self, boundary: &tools::Boundary) -> Result<(), Error> {
        self.stamp
            .require_current(backend::Owner::Execution(boundary))
            .await
    }
}

pub(crate) fn verdict(run: &backend::Run, name: &Text, engine: Engine) -> Result<Verdict, Error> {
    if run.timed_out || run.overflow || run.exit.is_none() {
        return Ok(Verdict::Missing);
    }
    let output = run.text()?.to_utf8()?;
    if engine == Engine::Cargo
        && (output.contains("could not compile") || output.contains("error[E"))
    {
        return Ok(Verdict::Missing);
    }
    let pattern = if engine == Engine::Node {
        regex(r"(?m)^\s*(not ok|ok) \d+ - (.*?)(?: # (SKIP|TODO)\b.*)?$")?
    } else {
        regex(r"(?m)^test (.+?) \.\.\. (ok|FAILED|ignored)\s*$")?
    };
    let expected = name.to_utf8()?;
    let mut found = Vec::new();
    for capture in pattern.captures_iter(&output) {
        let (actual, result) = if engine == Engine::Node {
            let actual = capture
                .get(2)
                .map_or("", |value| value.as_str())
                .replace("\\#", "#")
                .replace("\\\\", "\\");
            let result = if capture.get(3).is_some() {
                Verdict::Skipped
            } else if capture.get(1).is_some_and(|value| value.as_str() == "ok") {
                Verdict::Pass
            } else {
                Verdict::Fail
            };
            (actual, result)
        } else {
            let actual = capture.get(1).map_or("", |value| value.as_str()).to_owned();
            let result = match capture.get(2).map_or("", |value| value.as_str()) {
                "ok" => Verdict::Pass,
                "FAILED" => Verdict::Fail,
                _ => Verdict::Skipped,
            };
            (actual, result)
        };
        if actual == expected
            || engine == Engine::Cargo && actual.ends_with(&format!("::{expected}"))
        {
            found.push(result);
        }
    }
    if found.len() != 1 {
        return Ok(Verdict::Missing);
    }
    let result = found
        .first()
        .copied()
        .ok_or_else(|| denied("Named test observation disappeared"))?;
    if result == Verdict::Fail && run.exit.is_none_or(|exit| exit <= 0 || exit > 255) {
        return Ok(Verdict::Missing);
    }
    if result == Verdict::Pass && !run.success() {
        return Ok(Verdict::Missing);
    }
    Ok(result)
}

pub(crate) fn environment(home: &Path) -> BTreeMap<String, String> {
    let home = home.to_string_lossy().into_owned();
    BTreeMap::from([
        ("HOME".to_owned(), home.clone()),
        ("USERPROFILE".to_owned(), home.clone()),
        ("APPDATA".to_owned(), home.clone()),
        ("LOCALAPPDATA".to_owned(), home.clone()),
        ("XDG_CONFIG_HOME".to_owned(), home.clone()),
        ("XDG_DATA_HOME".to_owned(), home.clone()),
        ("ROSTER_STATE_HOME".to_owned(), home.clone()),
        ("TEMP".to_owned(), home.clone()),
        ("TMP".to_owned(), home.clone()),
        ("NO_COLOR".to_owned(), "1".to_owned()),
        ("CI".to_owned(), "1".to_owned()),
        ("ROSTER_SEAT".to_owned(), "coder".to_owned()),
        ("GIT_CONFIG_NOSYSTEM".to_owned(), "1".to_owned()),
        (
            "GIT_CONFIG_GLOBAL".to_owned(),
            if cfg!(windows) { "NUL" } else { "/dev/null" }.to_owned(),
        ),
        ("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned()),
        (
            "GH_CONFIG_DIR".to_owned(),
            Path::new(&home).join("gh").to_string_lossy().into_owned(),
        ),
    ])
}

pub(crate) async fn node_version(
    owner: backend::Owner<'_>,
    executable: &Path,
) -> Result<(), Error> {
    let _scope = owner.span(timing::Label::NodeVersion);
    let version = owner
        .run(
            owner.root(),
            executable,
            &["--version".to_owned()],
            &BTreeMap::new(),
            10_000,
            1024,
        )
        .await?;
    if !version.success()
        || !regex(r"^v20\.[0-9]+\.[0-9]+\s*$")?.is_match(&tools::bytes_text(&version.stdout)?)
    {
        return Err(denied(
            "Counterfactual Node checks require an actually observed approved Node 20",
        ));
    }
    Ok(())
}

pub(crate) async fn run_one(
    owner: backend::Owner<'_>,
    approval: &backend::Approval,
    scratch: &scratch::Scratch<'_>,
    directory: &Path,
    target: &Target,
    base: bool,
) -> Result<backend::Run, Error> {
    let _scope = owner.span(timing::Label::ConsumerRun);
    let home = scratch.directory(if base { "ib" } else { "ic" })?;
    let mut env = environment(&home);
    if target.engine == Engine::Node {
        let node = approval.node.as_deref().ok_or_else(|| {
            unsupported(
                "Selected consumer Node counterfactual has no explicit host executable approval",
            )
        })?;
        return owner
            .run(
                directory,
                node,
                &[
                    "--test".to_owned(),
                    "--test-reporter=tap".to_owned(),
                    format!("--test-timeout={}", approval.timeout()),
                    target.file.clone(),
                ],
                &env,
                approval.timeout(),
                evidence::MAX_DIFF_BYTES,
            )
            .await;
    }
    let cargo = owner
        .host()
        .cargo
        .as_ref()
        .ok_or_else(|| unsupported("Selected Rust counterfactual has no approved Cargo"))?;
    env.extend(
        cargo
            .environment
            .iter()
            .filter(|(name, _)| {
                ["CARGO_HOME", "RUSTUP_HOME", "RUSTFLAGS", "RUSTUP_TOOLCHAIN"]
                    .contains(&name.as_str())
            })
            .map(|(name, value)| (name.clone(), value.clone())),
    );
    env.insert("CARGO_NET_OFFLINE".to_owned(), "true".to_owned());
    env.insert("RUSTUP_AUTO_INSTALL".to_owned(), "0".to_owned());
    env.insert("CARGO_TERM_COLOR".to_owned(), "never".to_owned());
    env.insert(
        "CARGO_TARGET_DIR".to_owned(),
        scratch
            .directory(if base { "cb" } else { "cc" })?
            .to_string_lossy()
            .into_owned(),
    );
    let stem = target
        .file
        .strip_prefix("tests/")
        .and_then(|name| name.strip_suffix(".rs"))
        .ok_or_else(|| {
            unsupported("Rust integration counterfactual requires a direct tests/*.rs target")
        })?;
    owner
        .run(
            directory,
            &cargo.executable,
            &[
                "test".to_owned(),
                "--locked".to_owned(),
                "--offline".to_owned(),
                "--test".to_owned(),
                stem.to_owned(),
                "--".to_owned(),
                "--format=pretty".to_owned(),
            ],
            &env,
            approval.timeout(),
            evidence::MAX_DIFF_BYTES,
        )
        .await
}

fn value(status: &str, reasons: &[Text], tests: Vec<Value>, runs: Vec<Value>) -> Value {
    Value::object(vec![
        ("status", Value::string(status)),
        (
            "reasons",
            Value::Array(reasons.iter().cloned().map(Value::String).collect()),
        ),
        ("tests", Value::Array(tests)),
        ("runs", Value::Array(runs)),
    ])
}

fn observed(run: &backend::Run, name: &str, owner: backend::Owner<'_>) -> Result<Value, Error> {
    let output = crate::planning::memory::redact(&run.text()?, owner.redaction());
    Ok(Value::object(vec![
        ("revision", Value::string(name)),
        (
            "exit",
            run.exit
                .map_or(Value::Null, |exit| Value::Number(f64::from(exit))),
        ),
        ("timedOut", Value::Boolean(run.timed_out)),
        ("overflow", Value::Boolean(run.overflow)),
        (
            "output",
            Value::String(Text(output.0.into_iter().take(2000).collect())),
        ),
    ]))
}

pub async fn run(
    boundary: &tools::Boundary,
    approval: &backend::Approval,
    changes: &evidence::Changes,
    files: &[String],
) -> Result<Report, Error> {
    approval.validate()?;
    let owner = backend::Owner::Execution(boundary);
    changes.require_current(boundary)?;
    let stamp = backend::Stamp::capture(owner).await?;
    let mode = policy::tests_mode(boundary.task())?;
    if mode != "required" {
        return Ok(Report {
            stamp,
            value: value("exempt", &[], Vec::new(), Vec::new()),
            pass: true,
        });
    }
    let mut product = false;
    for file in files {
        product |= !substance::is_test_file(file)?
            && !file.starts_with("tests/")
            && regex(r"\.[cm]?[jt]sx?$|\.rs$|\.py$")?.is_match(file);
    }
    if !product {
        return Ok(Report {
            stamp,
            value: value("exempt", &[], Vec::new(), Vec::new()),
            pass: true,
        });
    }
    let mut targets = Vec::new();
    let mut reasons = Vec::new();
    for file in files {
        if !(substance::is_test_file(file)? || file.ends_with(".rs"))
            || crate::planning::forbidden_read(file)
        {
            continue;
        }
        let Some(source) = evidence::read_product(owner.root(), file, MAX_FILE_BYTES)? else {
            continue;
        };
        let previous = changes
            .head_product(boundary, file)
            .await?
            .unwrap_or_default();
        let old = substance::counterfactual_names(file, &previous)?;
        let mut names = Vec::new();
        for name in substance::counterfactual_names(file, &source)? {
            if !old.contains(&name) && !names.contains(&name) {
                names.push(name);
            }
        }
        if names.is_empty() {
            continue;
        }
        let engine = if file.ends_with(".rs") {
            Engine::Cargo
        } else {
            Engine::Node
        };
        if engine == Engine::Cargo && (!file.starts_with("tests/") || file.split('/').count() != 2)
        {
            reasons.push(Text::from(format!("Rust counterfactual for {file} mixes production/inline or non-direct test source; no candidate production was copied onto base.").as_str()));
        } else {
            targets.push(Target {
                file: file.clone(),
                names,
                engine,
            });
        }
    }
    if !reasons.is_empty() || targets.len() > 8 {
        if targets.len() > 8 {
            reasons.push(Text::from(
                "Counterfactual selection exceeds eight files; no required target was omitted.",
            ));
        }
        return Ok(Report {
            stamp,
            value: value("unavailable", &reasons, Vec::new(), Vec::new()),
            pass: false,
        });
    }
    if targets.is_empty() {
        return Ok(Report {
            stamp,
            value: value("none", &[], Vec::new(), Vec::new()),
            pass: true,
        });
    }
    if !approval.counterfactual {
        return Ok(Report {
            stamp,
            value: value(
                "unavailable",
                &[Text::from(
                    "Selected counterfactual execution requires explicit host approval.",
                )],
                Vec::new(),
                Vec::new(),
            ),
            pass: false,
        });
    }
    if targets.iter().any(|target| target.engine == Engine::Cargo)
        && changes
            .head_product(boundary, "Cargo.toml")
            .await?
            .is_none()
    {
        return Ok(Report {
            stamp,
            value: value(
                "unavailable",
                &[Text::from(
                    "Captured base has no Cargo manifest; native production is not reconstructed from candidate or legacy Node.",
                )],
                Vec::new(),
                Vec::new(),
            ),
            pass: false,
        });
    }
    if targets.iter().any(|target| target.engine == Engine::Node) {
        node_version(
            owner,
            approval.node.as_deref().ok_or_else(|| {
                unsupported("Selected Node counterfactual needs host-approved Node 20")
            })?,
        )
        .await?;
    }
    let scratch = scratch::Scratch::create(owner).await?;
    let outcome = async {
        let head = text(required(&changes.validation, "head")?, "Captured base revision is missing")?;
        let base = scratch.base(&head, approval).await?;
        scratch.overlay_tests(&base, files)?;
        let mut tests = Vec::new();
        let mut runs = Vec::new();
        let mut unavailable = false;
        let mut failed = false;
        for target in &targets {
            let before = evidence::capture(&base, None, &evidence::Projection::default())?;
            let before_git = backend::git_signature(owner, &base).await?;
            let at_base = run_one(owner, approval, &scratch, &base, target, true).await?;
            let after = evidence::capture(&base, None, &evidence::Projection::default())?;
            if !before.same_source(&after) || before_git != backend::git_signature(owner, &base).await? {
                return Err(denied("Counterfactual test mutated base files; it is not trusted red evidence"));
            }
            let at_candidate = run_one(owner, approval, &scratch, owner.root(), target, false).await?;
            runs.push(observed(&at_base, "base", owner)?);
            runs.push(observed(&at_candidate, "candidate", owner)?);
            let mut target_red_count = 0_usize;
            for name in &target.names {
                let base = verdict(&at_base, name, target.engine)?;
                let candidate = verdict(&at_candidate, name, target.engine)?;
                unavailable |= base == Verdict::Missing || candidate == Verdict::Missing;
                if base == Verdict::Fail && candidate == Verdict::Pass {
                    target_red_count += 1;
                }
                let single_not_red = target.names.len() == 1 && base != Verdict::Fail;
                failed |= candidate != Verdict::Pass || single_not_red;
                tests.push(Value::object(vec![
                    ("file", Value::string(&target.file)), ("name", Value::String(name.clone())),
                    ("base", Value::string(base.label())), ("candidate", Value::string(candidate.label())),
                ]));
                if candidate != Verdict::Pass || single_not_red {
                    reasons.push(Text::from(format!("Counterfactual {} {:?}: base {}, candidate {}; compiler/setup failure or missing named output is not a red test.", target.file, name.to_utf8()?, base.label(), candidate.label()).as_str()));
                }
            }
            if target_red_count == 0 && !unavailable {
                failed = true;
                if reasons.is_empty() {
                    if let Some(first_name) = target.names.first() {
                        reasons.push(Text::from(format!("Counterfactual {} {:?}: base pass, candidate pass; compiler/setup failure or missing named output is not a red test.", target.file, first_name.to_utf8()?).as_str()));
                    }
                }
            }
        }
        Ok((value(if unavailable {"unavailable"} else if failed {"fail"} else {"checked"}, &reasons, tests, runs), !failed && !unavailable))
    }.await;
    let (value, pass) = backend::finish_owned(owner, outcome, scratch.close().await)?;
    stamp.require_current(owner).await?;
    Ok(Report { stamp, value, pass })
}

pub fn extract_inline_rust_test_module(source: &str) -> Result<Option<(String, String)>, Error> {
    if source.len() > MAX_FILE_BYTES as usize {
        return Err(Error::type_error(
            "Inline Rust counterfactual source exceeds maximum file size",
        ));
    }
    let pattern = regex(
        r"(?m)^\s*#\[\s*cfg\s*\(\s*test\s*\)\s*\]\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+[A-Za-z_][A-Za-z0-9_]*\s*\{",
    )?;
    let mut matches = pattern.find_iter(source);
    let Some(first) = matches.next() else {
        if source.contains("#[cfg(test)]") {
            return Err(Error::type_error(
                "Inline #[cfg(test)] attribute must be attached to a single mod <name> { ... } block",
            ));
        }
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err(Error::type_error(
            "Inline Rust counterfactual source must not contain multiple #[cfg(test)] modules",
        ));
    }
    let prefix = &source[..first.start()];
    if prefix.contains("#[cfg(test)]") {
        return Err(Error::type_error(
            "Production prefix must not contain #[cfg(test)] attributes",
        ));
    }
    let mod_slice = &source[first.start()..];
    let bytes = mod_slice.as_bytes();
    let mut idx = 0;
    let mut depth = 0_usize;
    let mut started_brace = false;
    let mut close_offset = None;
    while idx < bytes.len() {
        let b = bytes[idx];
        if b == b'/' && idx + 1 < bytes.len() && bytes[idx + 1] == b'/' {
            idx += 2;
            while idx < bytes.len() && bytes[idx] != b'\n' {
                idx += 1;
            }
            continue;
        }
        if b == b'/' && idx + 1 < bytes.len() && bytes[idx + 1] == b'*' {
            idx += 2;
            while idx + 1 < bytes.len() && !(bytes[idx] == b'*' && bytes[idx + 1] == b'/') {
                idx += 1;
            }
            idx = (idx + 2).min(bytes.len());
            continue;
        }
        if (b == b'r' || (b == b'b' && idx + 1 < bytes.len() && bytes[idx + 1] == b'r'))
            && idx + 1 < bytes.len()
        {
            let mut r_idx = if b == b'b' { idx + 2 } else { idx + 1 };
            let mut hashes = 0_usize;
            while r_idx < bytes.len() && bytes[r_idx] == b'#' {
                hashes += 1;
                r_idx += 1;
            }
            if r_idx < bytes.len() && bytes[r_idx] == b'"' {
                idx = r_idx + 1;
                while idx < bytes.len() {
                    if bytes[idx] == b'"'
                        && idx + hashes < bytes.len()
                        && bytes[idx + 1..=idx + hashes].iter().all(|c| *c == b'#')
                    {
                        idx += hashes + 1;
                        break;
                    }
                    idx += 1;
                }
                continue;
            }
        }
        if b == b'\'' {
            if idx + 2 < bytes.len() && bytes[idx + 1] == b'\\' {
                let mut c_idx = idx + 2;
                while c_idx < bytes.len() && bytes[c_idx] != b'\'' && bytes[c_idx] != b'\n' {
                    c_idx += 1;
                }
                if c_idx < bytes.len() && bytes[c_idx] == b'\'' {
                    idx = c_idx + 1;
                    continue;
                }
            } else if idx + 2 < bytes.len() && bytes[idx + 2] == b'\'' {
                idx += 3;
                continue;
            }
        }
        if b == b'"' {
            idx += 1;
            while idx < bytes.len() {
                if bytes[idx] == b'\\' {
                    idx = (idx + 2).min(bytes.len());
                    continue;
                }
                if bytes[idx] == b'"' {
                    idx += 1;
                    break;
                }
                idx += 1;
            }
            continue;
        }
        if b == b'{' {
            depth += 1;
            started_brace = true;
        } else if b == b'}' {
            if depth == 0 {
                return Err(Error::type_error(
                    "Unbalanced closing brace in #[cfg(test)] module",
                ));
            }
            depth -= 1;
            if started_brace && depth == 0 {
                close_offset = Some(idx + 1);
                break;
            }
        }
        idx += 1;
    }
    let Some(end_pos) = close_offset else {
        return Err(Error::type_error(
            "Unterminated #[cfg(test)] module in Rust source",
        ));
    };
    let trailing = &mod_slice[end_pos..];
    if !trailing.trim().is_empty() {
        return Err(Error::type_error(
            "Inline #[cfg(test)] module must be the final item in the file with no trailing production code",
        ));
    }
    let test_block = mod_slice[..end_pos].to_owned();
    if !test_block.contains("#[test]") && !test_block.contains("#[tokio::test") {
        return Err(Error::type_error(
            "Inline #[cfg(test)] module must contain at least one #[test] or #[tokio::test] function",
        ));
    }
    Ok(Some((prefix.to_owned(), test_block)))
}

pub fn graft_inline_rust_tests_onto_base(
    base_source: &str,
    candidate_source: &str,
) -> Result<String, Error> {
    let base_prod = match extract_inline_rust_test_module(base_source)? {
        Some((prod, _)) => prod,
        None => base_source.to_owned(),
    };
    let Some((_, candidate_test_block)) = extract_inline_rust_test_module(candidate_source)? else {
        return Err(Error::type_error(
            "Candidate Rust source has no extractable inline #[cfg(test)] module",
        ));
    };
    Ok(format!(
        "{}\n\n{}\n",
        base_prod.trim_end(),
        candidate_test_block.trim()
    ))
}

#[cfg(test)]
mod inline_tests {
    use super::*;

    #[test]
    fn inline_rust_test_extraction_grafts_only_test_module_onto_untouched_base_production()
    -> Result<(), Error> {
        let base = "pub fn compute(x: u32) -> u32 {\n    x + 1\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n    #[test]\n    fn base_case() {\n        assert_eq!(compute(1), 2);\n    }\n}\n";
        let candidate = "pub fn compute(x: u32) -> u32 {\n    x + 10\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n    #[test]\n    fn new_increment_behavior() {\n        assert_eq!(compute(1), 11);\n    }\n}\n";
        let grafted = graft_inline_rust_tests_onto_base(base, candidate)?;
        assert!(grafted.contains("x + 1"));
        assert!(!grafted.contains("x + 10"));
        assert!(grafted.contains("fn new_increment_behavior()"));
        assert!(!grafted.contains("fn base_case()"));

        let trailing_prod = format!("{candidate}\npub fn sneaked_in() -> u32 {{ 99 }}\n");
        assert!(extract_inline_rust_test_module(&trailing_prod).is_err());
        Ok(())
    }
}
