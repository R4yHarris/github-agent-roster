use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::domain::{self, items, members, put};
use crate::fleet::{self, TaskClass};
use crate::json::Value;
use crate::text::{Text, ascii, trim};
use crate::{Error, catalog, config, paths, settings};

const FIELDS: &[&str] = &[
    "profile_id",
    "model_id",
    "task_class",
    "suggested_difficulty",
    "context_max",
    "concurrency",
    "notes",
];
const REQUIRED: &[&str] = &[
    "suggested_difficulty",
    "context_max",
    "concurrency",
    "notes",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completeness {
    Full,
    Partial,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Capabilities {
    records: Vec<Value>,
}

impl Capabilities {
    pub fn records(&self) -> &[Value] {
        &self.records
    }

    pub fn as_value(&self) -> Value {
        Value::object(vec![("capabilities", Value::Array(self.records.clone()))])
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Selector {
    Profile(Text),
    Model(Text),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    selector: Selector,
    task_class: TaskClass,
}

fn invalid_record() -> Error {
    Error::type_error(
        "Capability prior needs one profile_id or model_id, a task_class, and documented fields",
    )
}

fn key(record: &Value) -> Result<Key, Error> {
    let selector = if let Some(value) = record.get("profile_id").and_then(Value::as_string) {
        Selector::Profile(value.clone())
    } else {
        Selector::Model(
            record
                .get("model_id")
                .and_then(Value::as_string)
                .cloned()
                .ok_or_else(invalid_record)?,
        )
    };
    Ok(Key {
        selector,
        task_class: TaskClass::parse(record.get("task_class")).ok_or_else(invalid_record)?,
    })
}

fn has_ipv4(units: &[u16]) -> bool {
    for start in 0..units.len() {
        let mut position = start;
        let mut groups = 0;
        while groups < 3 {
            let digits = units
                .get(position..)
                .into_iter()
                .flatten()
                .take(3)
                .take_while(|unit| (48..=57).contains(*unit))
                .count();
            if digits == 0 || units.get(position + digits) != Some(&46) {
                break;
            }
            position += digits + 1;
            groups += 1;
        }
        if groups == 3
            && units
                .get(position)
                .is_some_and(|unit| (48..=57).contains(unit))
        {
            return true;
        }
    }
    false
}

fn has_ipv6(units: &[u16]) -> bool {
    units.iter().enumerate().any(|(index, unit)| {
        if *unit != 58 {
            return false;
        }
        let digits = units
            .get(index + 1..)
            .into_iter()
            .flatten()
            .take(4)
            .take_while(|unit| matches!(unit, 48..=57 | 65..=70 | 97..=102))
            .count();
        units.get(index + 1 + digits) == Some(&58)
    })
}

fn validate_record(record: &Value, completeness: Completeness) -> Result<Value, Error> {
    if members(record).is_none()
        || members(record).is_some_and(|entries| {
            entries
                .iter()
                .any(|(name, _)| !FIELDS.iter().any(|field| ascii(&name.0, field)))
        })
        || record.get("profile_id").is_some() == record.get("model_id").is_some()
        || TaskClass::parse(record.get("task_class")).is_none()
        || completeness == Completeness::Full
            && REQUIRED.iter().any(|name| record.get(name).is_none())
    {
        return Err(invalid_record());
    }
    if record.get("profile_id").is_some() {
        fleet::validate_id(record.get("profile_id"))?;
    } else {
        let model = fleet::actual_model(record.get("model_id"))?;
        if ascii(&model.0, "builtin-stub") {
            return Err(Error::type_error(
                "Capability model_id must name an actual model",
            ));
        }
    }
    if record.get("suggested_difficulty").is_some()
        && domain::safe_integer(record.get("suggested_difficulty"))
            .is_none_or(|number| !(1..=5).contains(&number))
    {
        return Err(Error::type_error(
            "Capability suggested_difficulty must be an integer from 1 to 5",
        ));
    }
    for field in ["context_max", "concurrency"] {
        if record.get(field).is_some()
            && domain::safe_integer(record.get(field)).is_none_or(|number| number == 0)
        {
            return Err(Error::type_error(format!(
                "Capability {field} must be a positive safe integer"
            )));
        }
    }
    if record.get("notes").is_some()
        && record
            .get("notes")
            .and_then(Value::as_string)
            .is_none_or(|value| {
                trim(&value.0).is_empty()
                    || value.0 != trim(&value.0)
                    || value.0.len() > 240
                    || value.0.iter().any(|unit| matches!(unit, 0..=31 | 127))
                    || has_ipv4(&value.0)
                    || has_ipv6(&value.0)
            })
    {
        return Err(Error::type_error(
            "Capability notes must be short single-line text without IP addresses",
        ));
    }
    Ok(record.clone())
}

pub fn validate(catalog: &Value, completeness: Completeness) -> Result<Capabilities, Error> {
    let records = catalog
        .get("capabilities")
        .and_then(items)
        .filter(|_| members(catalog).is_some_and(|entries| entries.len() == 1))
        .ok_or_else(|| Error::type_error("Capabilities must contain only a capabilities list"))?;
    let records = records
        .iter()
        .map(|record| validate_record(record, completeness))
        .collect::<Result<Vec<_>, _>>()?;
    let mut seen = HashSet::new();
    for record in &records {
        if !seen.insert(key(record)?) {
            return Err(Error::type_error(
                "Capability selector and task_class pairs must be unique",
            ));
        }
    }
    Ok(Capabilities { records })
}

pub fn parse(source: &Text, completeness: Completeness) -> Result<Capabilities, Error> {
    let records = domain::catalog_values(catalog::parse(source, "capabilities", FIELDS)?);
    validate(
        &Value::object(vec![("capabilities", Value::Array(records))]),
        completeness,
    )
}

pub fn format(catalog: &Value, completeness: Completeness) -> Result<Text, Error> {
    domain::format_catalog("capabilities", &validate(catalog, completeness)?.records)
}

pub fn merge(baseline: &Value, overlay: &Value) -> Result<Capabilities, Error> {
    let mut baseline = validate(baseline, Completeness::Full)?;
    let overlay = validate(overlay, Completeness::Partial)?;
    let mut positions = baseline
        .records
        .iter()
        .enumerate()
        .map(|(position, record)| key(record).map(|key| (key, position)))
        .collect::<Result<HashMap<_, _>, _>>()?;
    for record in overlay.records {
        let selector = key(&record)?;
        let position = positions.get(&selector).copied();
        if let Some(position) = position {
            let existing = baseline.records.get(position).ok_or_else(invalid_record)?;
            let merged = validate_record(&domain::merge(existing, &record)?, Completeness::Full)?;
            let existing = baseline
                .records
                .get_mut(position)
                .ok_or_else(invalid_record)?;
            *existing = merged;
        } else {
            positions.insert(selector, baseline.records.len());
            baseline
                .records
                .push(validate_record(&record, Completeness::Full)?);
        }
    }
    validate(&baseline.as_value(), Completeness::Full)
}

pub fn builtin() -> Result<Capabilities, Error> {
    parse(
        &Text::from(include_str!("../examples/capabilities.yml")),
        Completeness::Full,
    )
}

pub fn model_prior(
    model: &Value,
    task_class: Option<&Value>,
    catalog: Option<&Value>,
) -> Result<Value, Error> {
    let default_task = Value::string("feat");
    let task_class = task_class.unwrap_or(&default_task);
    if TaskClass::parse(Some(task_class)).is_none() {
        return Err(Error::type_error("Model prior requires a known task class"));
    }
    let builtin = builtin()?;
    let selected = catalog
        .filter(|catalog| **catalog != Value::Null)
        .cloned()
        .unwrap_or_else(|| builtin.as_value());
    let selected = validate(&selected, Completeness::Full)?;
    for record in selected.records {
        if record.get("model_id") == Some(model) && record.get("task_class") == Some(task_class) {
            let difficulty = domain::safe_integer(record.get("suggested_difficulty"))
                .ok_or_else(invalid_record)?;
            let mut result = record;
            put(
                &mut result,
                "strength",
                Value::string(if difficulty >= 4 {
                    "strong"
                } else if difficulty == 3 {
                    "standard"
                } else {
                    "limited"
                }),
            )?;
            return Ok(result);
        }
    }
    Ok(Value::object(vec![
        ("strength", Value::string("unknown")),
        ("suggested_difficulty", Value::Number(2.0)),
    ]))
}

pub fn load(repo_root: &Path, installation_root: &Path) -> Result<Capabilities, Error> {
    let file = installation_root.join("examples").join("capabilities.yml");
    paths::ensure_local_path(&file, installation_root)?;
    let baseline = parse(&config::read_source(&file)?, Completeness::Full)?;
    match config::read_private_source(repo_root, settings::Name::Capabilities)? {
        Some(source) => merge(
            &baseline.as_value(),
            &parse(&source, Completeness::Partial)?.as_value(),
        ),
        None => Ok(baseline),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::is_text;

    #[test]
    fn catalog_order_and_partial_overlays_round_trip() -> Result<(), Error> {
        let baseline = builtin()?;
        assert_eq!(baseline.records().len(), 10);
        assert_eq!(
            parse(
                &format(&baseline.as_value(), Completeness::Full)?,
                Completeness::Full,
            )?,
            baseline
        );
        let overlay = parse(
            &Text::from(
                "capabilities:\n  - profile_id: default\n    task_class: fix\n    suggested_difficulty: 4\n",
            ),
            Completeness::Partial,
        )?;
        let merged = merge(&baseline.as_value(), &overlay.as_value())?;
        assert_eq!(merged.records().len(), baseline.records().len());
        let changed = merged
            .records()
            .iter()
            .find(|record| {
                is_text(record.get("profile_id"), "default")
                    && is_text(record.get("task_class"), "fix")
            })
            .ok_or_else(invalid_record)?;
        assert_eq!(
            changed.get("suggested_difficulty"),
            Some(&Value::Number(4.0))
        );
        assert_eq!(changed.get("context_max"), Some(&Value::Number(8192.0)));
        Ok(())
    }

    #[test]
    fn unknown_priors_remain_unknown_and_do_not_register_models() -> Result<(), Error> {
        let prior = model_prior(&Value::string("unrated"), None, None)?;
        assert_eq!(
            prior,
            Value::object(vec![
                ("strength", Value::string("unknown")),
                ("suggested_difficulty", Value::Number(2.0)),
            ])
        );
        let strong = model_prior(&Value::string("deepseek-v4.1-flash"), None, None)?;
        assert!(is_text(strong.get("strength"), "strong"));
        assert_eq!(strong.get("context_max"), Some(&Value::Number(1048576.0)));
        Ok(())
    }

    #[test]
    fn ip_note_matching_has_the_original_unanchored_boundaries() {
        for note in [
            "Private host 192.0.2.10",
            "IPv6 host [2001:db8::1]",
            "x9999.0.0.12345x",
            "colons :a:",
        ] {
            let units = Text::from(note);
            assert!(has_ipv4(&units.0) || has_ipv6(&units.0), "{note}");
        }
        for note in ["Version 1.2.3", "one:colon", "colons :abcde:", "not:gh:hex"] {
            let units = Text::from(note);
            assert!(!has_ipv4(&units.0) && !has_ipv6(&units.0), "{note}");
        }
    }
}
