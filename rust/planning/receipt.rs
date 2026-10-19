use super::*;

fn scalar(source: &str) -> Result<Value, Error> {
    if source.starts_with('"') {
        let value = crate::json::parse(&decode(source))?;
        if !matches!(value, Value::String(_)) {
            return Err(Error::type_error("Planning receipt scalar must be text"));
        }
        return Ok(value);
    }
    if source.starts_with('\'') && source.ends_with('\'') {
        return Ok(value(
            &source
                .get(1..source.len().saturating_sub(1))
                .unwrap_or("")
                .replace("''", "'"),
        ));
    }
    if matches(r"^(?:0|[1-9]\d*)$", source)? {
        return Ok(Value::Number(source.parse::<f64>().map_err(|_| {
            Error::type_error("Invalid planning receipt scalar")
        })?));
    }
    if source.is_empty() || matches(r"[\x00-\x1f\x7f]|^[&*!{\[]", source)? {
        return Err(Error::type_error("Invalid planning receipt scalar"));
    }
    Ok(value(source))
}

pub fn validate(source: &Text, task: &task::Document) -> Result<(), Error> {
    let source = encode(source).replace("\r\n", "\n");
    let allowed = [
        "task",
        "title",
        "task_class",
        "difficulty",
        "estimate_min",
        "model",
        "ask",
        "acceptance_checks",
        "files_allowed",
        "notes",
        "steps",
    ];
    let lists = ["acceptance_checks", "files_allowed", "steps"];
    let mut receipt = Value::object(Vec::new());
    let mut current = String::new();
    let mut block = false;
    let header = regex(r"^([a-z_]+):\s*(.*)$")?;
    let bullet = regex(r"^\s{2,}-\s+(.+)$")?;
    for original in source.split('\n') {
        if trim(original).is_empty() || trim(original).starts_with('#') {
            continue;
        }
        if let Some(captures) = header.captures(original) {
            let key = captures.get(1).map_or("", |entry| entry.as_str());
            let raw = captures.get(2).map_or("", |entry| entry.as_str());
            if !allowed.contains(&key) || receipt.get(key).is_some() {
                return Err(Error::type_error("Invalid planning receipt field"));
            }
            current = key.to_owned();
            block = ["|", ">"].contains(&raw);
            let parsed = if lists.contains(&key) {
                if !raw.is_empty() {
                    return Err(Error::type_error(
                        "Planning receipt lists must use indented entries",
                    ));
                }
                Value::Array(Vec::new())
            } else if block {
                Value::string("")
            } else {
                scalar(raw)?
            };
            domain::put(&mut receipt, key, parsed)?;
            continue;
        }
        if let Some(item) = bullet
            .captures(original)
            .filter(|_| lists.contains(&current.as_str()))
        {
            let mut items = receipt
                .get(&current)
                .and_then(domain::items)
                .unwrap_or(&[])
                .to_vec();
            items.push(scalar(item.get(1).map_or("", |entry| entry.as_str()))?);
            domain::put(&mut receipt, &current, Value::Array(items))?;
        } else if block && matches(r"^\s{2,}\S", original)? {
            let previous = receipt
                .get(&current)
                .and_then(Value::as_string)
                .map(encode)
                .ok_or_else(|| Error::type_error("Invalid planning receipt YAML"))?;
            domain::put(
                &mut receipt,
                &current,
                value(&format!("{previous}{}\n", trim(original))),
            )?;
        } else {
            return Err(Error::type_error("Invalid planning receipt YAML"));
        }
    }
    let title = receipt
        .get("title")
        .or_else(|| receipt.get("task"))
        .and_then(Value::as_string)
        .map(encode);
    let checks = receipt.get("acceptance_checks").and_then(domain::items);
    if title
        .map(|title| task::normalize_ask(&title))
        .transpose()?
        .as_deref()
        != Some(&task::normalize_ask(&encode(&task.title))?)
        || checks.is_none_or(|checks| checks.is_empty())
        || receipt
            .get("files_allowed")
            .and_then(domain::items)
            .is_none()
    {
        return Err(Error::type_error(
            "Planning receipt must match the task title and contain checks and allowed files",
        ));
    }
    for (index, check) in checks.unwrap_or(&[]).iter().enumerate() {
        task::acceptance_check(check, index)?;
    }
    let files = task::application_files(required(&receipt, "files_allowed")?)?;
    if files.len() != task.files_allowed.len()
        || files.iter().any(|file| !task.files_allowed.contains(file))
    {
        return Err(Error::type_error(
            "Planning receipt application paths differ from TASK.md",
        ));
    }
    estimate::estimate(&receipt, &[], "")?;
    Ok(())
}
