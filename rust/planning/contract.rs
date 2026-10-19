use super::*;

fn optional_text(input: &Value, field: &str) -> Result<Option<Text>, Error> {
    input
        .get(field)
        .map(|value| {
            value
                .as_string()
                .cloned()
                .ok_or_else(|| Error::type_error(format!("{field} must be text")))
        })
        .transpose()
}

fn string_list(input: Option<&Value>) -> Result<Vec<String>, Error> {
    input
        .map(|input| {
            array(input, "Planning paths must be an array")?
                .iter()
                .map(|item| text(item, "Planning paths must be text"))
                .collect()
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

fn authority(input: &Value) -> Result<task::Authority, Error> {
    Ok(task::Authority {
        expected_ask: optional_text(input, "expectedAsk")?,
        issue_title: optional_text(input, "issueTitle")?,
        issue_body: optional_text(input, "issueBody")?,
    })
}

fn context(input: &Value) -> Result<Option<grounding::Context>, Error> {
    input
        .get("index")
        .map(|index| {
            Ok(grounding::Context {
                index: grounding::Index::from_value(index)?,
                related: Vec::new(),
                definitions: optional_text(input, "definitions")?.unwrap_or_default(),
                test_names: string_list(input.get("testNames"))?,
            })
        })
        .transpose()
}

pub fn evaluate(input: &Value) -> Result<Value, Error> {
    let operation = text(
        required(input, "operation")?,
        "Planning operation must be text",
    )?;
    let empty = Value::object(Vec::new());
    match operation.as_str() {
        "one-line" => Ok(Value::String(task::one_line(
            required(input, "value")?,
            "Value",
        )?)),
        "acceptance" => Ok(Value::String(task::acceptance_check(
            required(input, "value")?,
            domain::safe_integer(input.get("index")).unwrap_or(0) as usize,
        )?)),
        "allowed-file" => Ok(value(&task::allowed_file(required(input, "value")?)?)),
        "forbidden-read" => Ok(Value::Boolean(forbidden_read(&text(
            required(input, "value")?,
            "Path must be text",
        )?))),
        "forbidden-write" => Ok(Value::Boolean(forbidden_write(&text(
            required(input, "value")?,
            "Path must be text",
        )?))),
        "is-allowed" => Ok(Value::Boolean(is_allowed(
            &text(required(input, "file")?, "Path must be text")?,
            &string_list(input.get("files"))?,
        ))),
        "without-references" => Ok(value(&intake::without_reference_urls(&text(
            required(input, "value")?,
            "Reference text must be text",
        )?)?)),
        "clean-ask" => Ok(value(&intake::clean_ask(required(input, "value")?)?)),
        "directory" => Ok(task::output_directory(&text(
            required(input, "value")?,
            "Ask must be text",
        )?)?
        .map_or(Value::Null, |directory| value(&directory))),
        "directory-document" => Ok(Value::Boolean(task::directory_document(&text(
            required(input, "value")?,
            "Ask must be text",
        )?)?)),
        "requests-executable" => Ok(Value::Boolean(task::requests_executable(&text(
            required(input, "value")?,
            "Ask must be text",
        )?)?)),
        "directory-files" => {
            task::validate_directory_files(
                &string_list(input.get("files"))?,
                &text(required(input, "directory")?, "Directory must be text")?,
            )?;
            Ok(Value::Null)
        }
        "directory-documents" => {
            task::validate_directory_documents(
                &text(required(input, "ask")?, "Ask must be text")?,
                &string_list(input.get("files"))?,
                &string_list(input.get("checks"))?,
            )?;
            Ok(Value::Null)
        }
        "directory-executable" => {
            let class = optional_text(input, "taskClass")?.as_ref().map(encode);
            task::validate_directory_executable(
                &text(required(input, "ask")?, "Ask must be text")?,
                &string_list(input.get("files"))?,
                class.as_deref(),
            )?;
            Ok(Value::Null)
        }
        "normalize-ask" => Ok(value(&task::normalize_ask(&text(
            required(input, "value")?,
            "Ask comparison requires text",
        )?)?)),
        "task-sections" => Ok(task::sections(required(input, "value")?)?.as_value()),
        "task-parse" => Ok(task::parse(required(input, "value")?, &authority(input)?)?.as_value()),
        "task-files" => Ok(strings(&task::files_allowed(required(input, "value")?)?)),
        "application-files" => Ok(strings(&task::application_files(required(
            input, "value",
        )?)?)),
        "ensure-ask" | "ensure-title" | "ensure-files" | "ensure-checks" => {
            let source = required(input, "value")?
                .as_string()
                .ok_or_else(|| Error::type_error("TASK.md must be text"))?;
            let title = optional_text(input, "title")?;
            let ask = optional_text(input, "ask")?
                .as_ref()
                .map(encode)
                .unwrap_or_default();
            Ok(Value::String(match operation.as_str() {
                "ensure-ask" => task::ensure_original_ask(source, title.as_ref())?,
                "ensure-title" => task::ensure_title(source, title.as_ref())?,
                "ensure-files" => task::ensure_files(source, &ask)?,
                _ => task::ensure_checks(source, &ask)?,
            }))
        }
        "named-files" => Ok(strings(&task::named_files(&text(
            required(input, "value")?,
            "Ask must be text",
        )?)?)),
        "requirements" => Ok(intake::requirements(
            required(input, "value")?,
            input.get("allowMissing") == Some(&Value::Boolean(true)),
        )?
        .as_value()),
        "classify" => intake::classify(
            required(input, "value")?,
            optional_text(input, "title")?.as_ref(),
            &string_list(input.get("filesAllowed"))?,
        ),
        "ambiguous" => intake::ambiguous(required(input, "value")?),
        "clarification" => intake::clarification(required(input, "value")?),
        "skill-names" => Ok(strings(&skills::task_names(required(input, "value")?)?)),
        "estimate" => Ok(estimate::estimate(
            input.get("metadata").unwrap_or(&empty),
            input
                .get("evaluations")
                .map(|value| array(value, "Evaluations must be an array"))
                .transpose()?
                .unwrap_or(&[]),
            &optional_text(input, "defaultModel")?
                .as_ref()
                .map(encode)
                .unwrap_or_default(),
        )?
        .as_value()),
        "needs-reestimate" => Ok(Value::Boolean(estimate::needs_reestimate(
            input.get("estimate"),
            required(input, "currentFiles")?,
        )?)),
        "read-metadata" => Ok(estimate::read_task_metadata(required(input, "value")?)?.as_value()),
        "update-metadata" => Ok(Value::String(estimate::update_task_metadata(
            required(input, "value")?,
            required(input, "metadata")?,
        )?)),
        "estimate-artifact" => {
            let source = required(input, "value")?;
            let metadata = estimate::read_task_metadata(source)?;
            let estimated = estimate::estimate(
                &metadata.as_value(),
                input
                    .get("evaluations")
                    .map(|value| array(value, "Evaluations must be arrays"))
                    .transpose()?
                    .unwrap_or(&[]),
                &optional_text(input, "defaultModel")?
                    .as_ref()
                    .map(encode)
                    .unwrap_or_default(),
            )?;
            let estimated = estimate::with_recommendation(estimated, input.get("recommendation"))?;
            Ok(Value::object(vec![
                (
                    "task",
                    Value::String(estimate::update_task_metadata(
                        source,
                        &estimated.as_value(),
                    )?),
                ),
                ("metadata", estimated.as_value()),
                ("estimate", Value::String(estimated.render())),
            ]))
        }
        "recipe" => Ok(Value::String(artifacts::runtime_recipe(&text(
            required(input, "reference")?,
            "Reference must be text",
        )?)?)),
        "render-ask" => Ok(Value::String(artifacts::render_ask(
            required(input, "value")?
                .as_string()
                .ok_or_else(|| Error::type_error("Ask must be text"))?,
        ))),
        "render-assignment" => Ok(Value::String(artifacts::render_assignment(required(
            input, "value",
        )?)?)),
        "feedback" => {
            let snapshot = feedback::Snapshot {
                evaluations: input
                    .get("evaluations")
                    .map(|value| {
                        array(value, "Evaluations must be arrays").map(|items| items.to_vec())
                    })
                    .transpose()?
                    .unwrap_or_default(),
                records: input
                    .get("records")
                    .map(|value| {
                        array(value, "Metrics records must be arrays").map(|items| items.to_vec())
                    })
                    .transpose()?
                    .unwrap_or_default(),
                estimation_history: Vec::new(),
            };
            let mut redaction = crate::state::redaction::Config::default();
            if let Some(environment) = input.get("env") {
                redaction.environment = environment.clone();
            }
            feedback::apply(
                required(input, "value")?
                    .as_string()
                    .ok_or_else(|| Error::type_error("TASK.md must be text"))?,
                &snapshot,
                required(input, "config")?,
                input.get("env").unwrap_or(&empty),
                &redaction,
            )
        }
        "recommend" => {
            let difficulty = match input.get("difficulty") {
                None | Some(Value::Null) => None,
                Some(value) => Some(
                    domain::safe_integer(Some(value))
                        .filter(|value| (1..=5).contains(value))
                        .ok_or_else(|| {
                            Error::type_error("difficulty must be an integer from 1 to 5")
                        })? as u8,
                ),
            };
            Ok(feedback::recommend(
                array(
                    required(input, "records")?,
                    "Metrics records must be arrays",
                )?,
                &text(required(input, "taskClass")?, "Task class must be text")?,
                difficulty,
            )?
            .unwrap_or(Value::Null))
        }
        "build" | "stub" => {
            let reference = optional_text(input, "reference")?
                .as_ref()
                .map(encode)
                .unwrap_or_else(|| "local:draft".to_owned());
            let plan = if operation == "stub" {
                artifacts::stub(
                    required(input, "ask")?,
                    &reference,
                    input.get("title"),
                    input.get("metadata"),
                )?
            } else {
                let context = context(input)?;
                artifacts::build(
                    required(input, "ask")?,
                    artifacts::BuildOptions {
                        reference: Some(&reference),
                        title: input.get("title"),
                        checks: input.get("checks"),
                        files: input.get("files"),
                        metadata: input.get("metadata"),
                        grounding: context.as_ref(),
                        design: input.get("design"),
                        ..artifacts::BuildOptions::default()
                    },
                )?
            };
            Ok(plan.as_value())
        }
        "from-task" => {
            let ask = required(input, "ask")?
                .as_string()
                .ok_or_else(|| Error::type_error("Ask must be text"))?;
            artifacts::from_task(
                required(input, "value")?,
                ask,
                optional_text(input, "issueTitle")?.as_ref(),
                optional_text(input, "issueBody")?.as_ref(),
            )
        }
        "leading-json" => artifacts::leading_json(
            required(input, "value")?
                .as_string()
                .ok_or_else(|| Error::type_error("JSON source must be text"))?,
        ),
        "normalize-design" => {
            Ok(grounding::normalize_design(input.get("value"))?.unwrap_or(Value::Null))
        }
        "parse-design" => {
            Ok(grounding::parse_design(required(input, "value")?)?.unwrap_or(Value::Null))
        }
        "render-design" => Ok(Value::String(grounding::render_design(required(
            input, "value",
        )?)?)),
        "with-design" => Ok(Value::String(grounding::with_design(
            required(input, "value")?,
            required(input, "design")?,
        )?)),
        "grounding-errors" => {
            let index = input
                .get("index")
                .map(grounding::Index::from_value)
                .transpose()?;
            Ok(strings(&grounding::errors(
                &string_list(input.get("checks"))?,
                input.get("design").filter(|value| **value != Value::Null),
                &string_list(input.get("files"))?,
                &optional_text(input, "ask")?
                    .as_ref()
                    .map(encode)
                    .unwrap_or_default(),
                index.as_ref(),
            )?))
        }
        "symbol-index" | "related-exports" => {
            let files = string_list(input.get("files"))?;
            let sources = input
                .get("sources")
                .and_then(domain::members)
                .ok_or_else(|| Error::type_error("Planning source fixture requires a source map"))?
                .iter()
                .map(|(file, source)| {
                    Ok((
                        encode(file),
                        source
                            .as_string()
                            .cloned()
                            .ok_or_else(|| Error::type_error("Source fixture must be text"))?,
                    ))
                })
                .collect::<Result<Vec<_>, Error>>()?;
            if operation == "symbol-index" {
                return Ok(grounding::index_sources(&files, &sources)?.as_value());
            }
            let related = grounding::related_exports(
                &sources,
                &files,
                &optional_text(input, "ask")?
                    .as_ref()
                    .map(encode)
                    .unwrap_or_default(),
                domain::safe_integer(input.get("limit")).unwrap_or(12) as usize,
                domain::safe_integer(input.get("exportsPerFile")).unwrap_or(16) as usize,
            )?;
            Ok(Value::Array(
                related
                    .iter()
                    .map(|(file, exports)| {
                        Value::object(vec![("file", value(file)), ("exports", strings(exports))])
                    })
                    .collect(),
            ))
        }
        "derive-design" => {
            let index = grounding::Index::from_value(required(input, "index")?)?;
            grounding::derive_design(&string_list(input.get("files"))?, &index, &[])
        }
        "ensure-design" => {
            let context = context(input)?
                .ok_or_else(|| Error::type_error("Design derivation requires an index"))?;
            Ok(Value::String(grounding::ensure_design(
                required(input, "value")?,
                &context,
                &optional_text(input, "ask")?
                    .as_ref()
                    .map(encode)
                    .unwrap_or_default(),
            )?))
        }
        "critic-defects" => {
            let index = input
                .get("index")
                .map(grounding::Index::from_value)
                .transpose()?;
            Ok(Value::Array(critic::deterministic(
                required(input, "value")?,
                index.as_ref(),
                &string_list(input.get("testNames"))?,
            )?))
        }
        "critic-report" => Ok(Value::Array(critic::validate_report(
            required(input, "value")?,
            domain::safe_integer(input.get("count")).unwrap_or(0) as usize,
        )?)),
        "plan-validate" => {
            let kind =
                intake::Kind::parse(&text(required(input, "kind")?, "Ask kind must be text")?)?;
            let repository = input
                .get("repositoryFiles")
                .map(|_| string_list(input.get("repositoryFiles")))
                .transpose()?;
            outline::validate(
                required(input, "value")?,
                kind,
                &string_list(input.get("filesAllowed"))?,
                repository.as_deref(),
                input.get("proposedScope") == Some(&Value::Boolean(true)),
            )
        }
        "plan-render" => Ok(Value::String(outline::render(
            required(input, "value")?,
            required(input, "ask")?,
            input.get("title"),
            intake::Kind::parse(&text(required(input, "kind")?, "Ask kind must be text")?)?,
            &text(required(input, "reference")?, "Reference must be text")?,
        )?)),
        "plan-parse" => outline::parse_document(required(input, "value")?),
        "receipt" => {
            let task = task::parse(required(input, "task")?, &task::Authority::default())?;
            receipt::validate(
                required(input, "value")?
                    .as_string()
                    .ok_or_else(|| Error::type_error("Receipt must be text"))?,
                &task,
            )?;
            Ok(Value::Null)
        }
        "tool-calls" => Ok(
            match tools::parse_calls(
                required(input, "value")?,
                domain::safe_integer(input.get("turn")).unwrap_or(1) as usize,
                &string_list(input.get("usedIds"))?,
            ) {
                Ok(calls) => calls.as_value(),
                Err(error) => Value::object(vec![("error", Value::string(&error.message))]),
            },
        ),
        "memory-text" => {
            let memory = array(
                required(input, "value")?,
                "Planner memory must contain single-line JSONL data",
            )?
            .iter()
            .map(|value| {
                value.as_string().cloned().ok_or_else(|| {
                    Error::type_error("Planner memory must contain single-line JSONL data")
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
            let mut redaction = crate::state::redaction::Config::default();
            if let Some(environment) = input.get("env") {
                redaction.environment = environment.clone();
            }
            Ok(Value::String(memory::planner_text(&memory, &redaction)?))
        }
        _ => Err(Error::type_error(
            "Unknown native planning contract operation",
        )),
    }
}
