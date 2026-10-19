use crate::catalog::{Catalog, Scalar};
use crate::json::Value;
use crate::text::{Text, ascii};
use crate::{Error, MAX_SAFE_INTEGER};

pub fn members(value: &Value) -> Option<&[(Text, Value)]> {
    match value {
        Value::Object(entries) => Some(entries),
        _ => None,
    }
}

pub fn items(value: &Value) -> Option<&[Value]> {
    match value {
        Value::Array(items) => Some(items),
        _ => None,
    }
}

pub fn property<'a>(value: Option<&'a Value>, name: &str) -> Result<Option<&'a Value>, Error> {
    match value {
        None | Some(Value::Null) => Err(Error::type_error(format!(
            "Cannot read properties of {} (reading '{name}')",
            if value.is_none() { "undefined" } else { "null" }
        ))),
        Some(value) => Ok(value.get(name)),
    }
}

pub fn spread(value: &Value) -> Value {
    match value {
        Value::Object(_) => value.clone(),
        Value::Array(values) => Value::Object(
            values
                .iter()
                .enumerate()
                .map(|(index, value)| (Text::from(index.to_string().as_str()), value.clone()))
                .collect(),
        ),
        Value::String(value) => Value::Object(
            value
                .0
                .iter()
                .enumerate()
                .map(|(index, unit)| {
                    (
                        Text::from(index.to_string().as_str()),
                        Value::String(Text(vec![*unit])),
                    )
                })
                .collect(),
        ),
        _ => Value::Object(Vec::new()),
    }
}

pub fn put(object: &mut Value, name: &str, value: Value) -> Result<(), Error> {
    let Value::Object(entries) = object else {
        return Err(Error::type_error("Native domain requires an object"));
    };
    if let Some((_, existing)) = entries.iter_mut().find(|(key, _)| ascii(&key.0, name)) {
        *existing = value;
    } else {
        entries.push((Text::from(name), value));
    }
    Ok(())
}

pub fn remove(object: &mut Value, name: &str) -> Result<(), Error> {
    let Value::Object(entries) = object else {
        return Err(Error::type_error("Native domain requires an object"));
    };
    entries.retain(|(key, _)| !ascii(&key.0, name));
    Ok(())
}

pub fn merge(base: &Value, overlay: &Value) -> Result<Value, Error> {
    let mut result = base.clone();
    for (name, value) in
        members(overlay).ok_or_else(|| Error::type_error("Native domain requires an object"))?
    {
        put(&mut result, &name.to_utf8()?, value.clone())?;
    }
    Ok(result)
}

pub fn is_text(value: Option<&Value>, expected: &str) -> bool {
    value
        .and_then(Value::as_string)
        .is_some_and(|text| ascii(&text.0, expected))
}

pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Boolean(value) => *value,
        Value::Number(value) => *value != 0.0 && !value.is_nan(),
        Value::String(value) => !value.0.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

pub fn string(value: &Value) -> Result<Text, Error> {
    Ok(match value {
        Value::Null => Text::from("null"),
        Value::Boolean(value) => Text::from(if *value { "true" } else { "false" }),
        Value::String(value) => value.clone(),
        Value::Number(value) if value.is_nan() => Text::from("NaN"),
        Value::Number(value) if *value == f64::INFINITY => Text::from("Infinity"),
        Value::Number(value) if *value == f64::NEG_INFINITY => Text::from("-Infinity"),
        Value::Number(_) => Text::from(value.json().as_str()),
        Value::Object(_) if value.get("toString").is_some() => {
            return Err(Error::type_error(
                "Cannot convert object to primitive value",
            ));
        }
        Value::Object(_) => Text::from("[object Object]"),
        Value::Array(values) => {
            let mut result = Text::default();
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    result.push_str(",");
                }
                if value != &Value::Null {
                    result.0.extend(string(value)?.0);
                }
            }
            result
        }
    })
}

pub fn scalar(value: &Value) -> Result<Text, Error> {
    match value {
        Value::String(value) => Ok(Text::from(value.json().as_str())),
        Value::Array(values) => {
            let mut result = Text::from("[");
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    result.push_str(", ");
                }
                if value != &Value::Null {
                    result.0.extend(string(value)?.0);
                }
            }
            result.push_str("]");
            Ok(result)
        }
        _ => string(value),
    }
}

pub fn safe_integer(value: Option<&Value>) -> Option<u64> {
    value.and_then(Value::as_number).and_then(|number| {
        if number.is_finite()
            && number.fract() == 0.0
            && (0.0..=MAX_SAFE_INTEGER as f64).contains(&number)
        {
            Some(number as u64)
        } else {
            None
        }
    })
}

pub fn catalog_values(catalog: Catalog) -> Vec<Value> {
    catalog
        .entries
        .into_iter()
        .map(|entry| {
            Value::Object(
                entry
                    .into_iter()
                    .map(|(name, value)| {
                        let value = match value {
                            Scalar::String(value) => Value::String(value),
                            Scalar::Integer(value) => Value::Number(value as f64),
                            Scalar::List(values) => {
                                Value::Array(values.into_iter().map(Value::String).collect())
                            }
                        };
                        (Text::from(name.as_str()), value)
                    })
                    .collect(),
            )
        })
        .collect()
}

pub fn format_catalog(root: &str, records: &[Value]) -> Result<Text, Error> {
    if records.is_empty() {
        return Ok(Text::from(format!("{root}: []\n").as_str()));
    }
    let mut result = Text::from(format!("{root}:\n").as_str());
    for record in records {
        let entries = members(record)
            .ok_or_else(|| Error::type_error("Native catalog record must be an object"))?;
        for (index, (name, value)) in entries.iter().enumerate() {
            result.push_str(if index == 0 { "  - " } else { "    " });
            result.0.extend_from_slice(&name.0);
            result.push_str(": ");
            result.0.extend(scalar(value)?.0);
            result.push_str("\n");
        }
    }
    Ok(result)
}
