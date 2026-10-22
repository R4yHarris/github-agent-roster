use std::collections::BTreeSet;

use super::*;

pub fn counterfactual_names(file: &str, source: &Text) -> Result<Vec<Text>, Error> {
    if file.ends_with(".rs") {
        let source = source.to_utf8()?;
        return Ok(regex(
            r"(?s)#\[\s*(?:(?:tokio|async_std)::)?test(?:\s*\([^]]*\))?\s*\]\s*(?:#\[[^\]]*\]\s*)*(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)",
        )?
        .captures_iter(&source)
        .filter_map(|capture| capture.get(1))
        .map(|name| Text::from(name.as_str()))
        .collect());
    }
    let mut names = Vec::new();
    let units = &source.0;
    let mut index = 0;
    while index < units.len() {
        if index > 0
            && units
                .get(index - 1)
                .is_some_and(|unit| matches!(unit, 48..=57 | 65..=90 | 97..=122 | 95))
        {
            index += 1;
            continue;
        }
        let Some(token) = ["test(", "test.only(", "it(", "it.only("]
            .iter()
            .find(|token| units[index..].starts_with(&Text::from(**token).0))
        else {
            index += 1;
            continue;
        };
        let mut cursor = index + token.len();
        while units
            .get(cursor)
            .copied()
            .is_some_and(crate::text::is_space)
        {
            cursor += 1;
        }
        let Some(quote) = units
            .get(cursor)
            .copied()
            .filter(|unit| [34, 39].contains(unit))
        else {
            index += 1;
            continue;
        };
        cursor += 1;
        let mut name = Text::default();
        let mut closed = false;
        while let Some(unit) = units.get(cursor).copied() {
            if unit == quote {
                closed = true;
                break;
            }
            if unit == 10 {
                break;
            }
            if unit == 92 {
                let Some(escaped) = units
                    .get(cursor + 1)
                    .copied()
                    .filter(|unit| ![10, 13, 0x2028, 0x2029].contains(unit))
                else {
                    break;
                };
                name.0.push(escaped);
                cursor += 2;
            } else {
                name.0.push(unit);
                cursor += 1;
            }
        }
        if closed {
            names.push(name);
        }
        index += 1;
    }
    Ok(names)
}

const HARNESS: &[&str] = &[
    "node:assert",
    "node:assert/strict",
    "assert",
    "assert/strict",
    "node:test",
];
const KEYWORDS: &[&str] = &[
    "if", "for", "while", "switch", "catch", "function", "return", "typeof", "new", "await",
    "test", "it", "describe", "assert", "JSON", "Object", "Array", "String", "Number", "Date",
    "Math", "Promise",
];
const BUILTINS: &[&str] = &[
    "includes",
    "indexOf",
    "lastIndexOf",
    "startsWith",
    "endsWith",
    "match",
    "matchAll",
    "test",
    "exec",
    "search",
    "replace",
    "replaceAll",
    "split",
    "slice",
    "substring",
    "trim",
    "toString",
    "toLowerCase",
    "toUpperCase",
    "some",
    "every",
    "map",
    "filter",
    "find",
    "findIndex",
    "forEach",
    "reduce",
    "join",
    "at",
    "concat",
    "flat",
    "flatMap",
    "keys",
    "values",
    "entries",
    "has",
    "get",
    "stringify",
    "parse",
    "valueOf",
];

fn ascii(source: &Text) -> String {
    source
        .0
        .iter()
        .map(|unit| {
            if *unit <= 127 {
                *unit as u8 as char
            } else if crate::text::is_space(*unit) {
                ' '
            } else {
                '~'
            }
        })
        .collect()
}

fn word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}
fn boundary(source: &str, start: usize, dot: bool) -> bool {
    start == 0
        || source
            .as_bytes()
            .get(start - 1)
            .is_none_or(|byte| !word(*byte) && (!dot || *byte != b'.'))
}
fn named(source: &str, name: &str) -> bool {
    source.match_indices(name).any(|(start, _)| {
        boundary(source, start, false)
            && source
                .as_bytes()
                .get(start + name.len())
                .is_none_or(|byte| !word(*byte))
    })
}
fn contains(source: &Text, value: &Text) -> bool {
    value.0.is_empty()
        || source
            .0
            .windows(value.0.len())
            .any(|window| window == value.0)
}
fn text_slice(source: &Text, start: usize, end: usize) -> Text {
    Text(
        source
            .0
            .get(start..end.min(source.0.len()))
            .unwrap_or(&[])
            .to_vec(),
    )
}
fn code_slice(source: &str, start: usize, end: usize) -> &str {
    source.get(start..end.min(source.len())).unwrap_or("")
}

pub fn is_test_file(file: &str) -> Result<bool, Error> {
    let normalized = file.replace('\\', "/");
    Ok(regex(
        r"\.(test|spec)\.[cm]?[jt]s$|(^|/)(tests?|__tests__)/.+\.[cm]?[jt]s$|(^|/)tests/.+\.rs$|(_test|_tests|tests)\.rs$",
    )?
    .is_match(&normalized))
}

fn interpolation_end(source: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0;
    for (index, byte) in source.iter().enumerate().skip(start) {
        match byte {
            b'`' => return None,
            b'{' => depth += 1,
            b'}' if depth == 0 => return Some(index),
            b'}' => depth -= 1,
            _ => {}
        }
    }
    None
}

fn mask_inner(source: &str, depth: usize) -> Result<String, Error> {
    if depth > 64 {
        return Err(Error::type_error(
            "Test substance template nesting exceeds 64",
        ));
    }
    let bytes = source.as_bytes();
    let mut output = Vec::new();
    let mut index = 0;
    let mut previous = None;
    while let Some(byte) = bytes.get(index).copied() {
        let next = bytes.get(index + 1).copied();
        if byte == b'/' && next == Some(b'/') {
            while bytes.get(index).is_some_and(|byte| *byte != b'\n') {
                output.push(b' ');
                index += 1;
            }
            continue;
        }
        if byte == b'/' && next == Some(b'*') {
            while bytes.get(index).is_some()
                && !(bytes.get(index) == Some(&b'*') && bytes.get(index + 1) == Some(&b'/'))
            {
                output.push(if bytes.get(index) == Some(&b'\n') {
                    b'\n'
                } else {
                    b' '
                });
                index += 1;
            }
            output.extend(*b"  ");
            index += 2;
            continue;
        }
        if matches!(byte, b'"' | b'\'' | b'`')
            || byte == b'/' && previous.is_none_or(|byte| b"(,=:[!&|?{};".contains(&byte))
        {
            let quote = byte;
            output.push(quote);
            index += 1;
            while bytes.get(index).is_some_and(|byte| *byte != quote) {
                if bytes.get(index) == Some(&b'\\') {
                    output.extend(*b"  ");
                    index += 2;
                    continue;
                }
                if quote != b'`' && bytes.get(index) == Some(&b'\n') {
                    break;
                }
                if quote == b'`'
                    && bytes.get(index) == Some(&b'$')
                    && bytes.get(index + 1) == Some(&b'{')
                {
                    if let Some(close) =
                        interpolation_end(bytes, index + 2).filter(|close| *close > 0)
                    {
                        output.extend(*b"${");
                        output.extend(
                            mask_inner(code_slice(source, index + 2, close), depth + 1)?.bytes(),
                        );
                        output.push(b'}');
                        index = close + 1;
                        continue;
                    }
                }
                output.push(if bytes.get(index) == Some(&b'\n') {
                    b'\n'
                } else {
                    b' '
                });
                index += 1;
            }
            if let Some(byte) = bytes.get(index) {
                output.push(*byte);
                index += 1;
            }
            previous = Some(quote);
            continue;
        }
        output.push(byte);
        if !byte.is_ascii_whitespace() {
            previous = Some(byte);
        }
        index += 1;
    }
    String::from_utf8(output).map_err(|_| Error::type_error("Masked source must remain ASCII"))
}

pub fn mask(source: &Text) -> Result<String, Error> {
    if source.0.len() > 262_144 {
        return Err(Error::type_error(
            "Test substance input exceeds 256 Ki UTF-16 units",
        ));
    }
    mask_inner(&ascii(source), 0)
}

fn close(source: &str, open: usize) -> Option<usize> {
    let mut stack = Vec::new();
    for (index, byte) in source.bytes().enumerate().skip(open) {
        match byte {
            b'(' => stack.push(b')'),
            b'[' => stack.push(b']'),
            b'{' => stack.push(b'}'),
            b')' | b']' | b'}' => {
                if stack.pop() != Some(byte) {
                    return None;
                }
                if stack.is_empty() {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn statement_end(source: &str, start: usize) -> usize {
    let mut depth: i32 = 0;
    for (index, byte) in source.bytes().enumerate().skip(start) {
        if b"([{".contains(&byte) {
            depth += 1;
        } else if b")]}".contains(&byte) {
            depth -= 1;
            if depth < 0 {
                return index;
            }
        } else if depth == 0 && matches!(byte, b';' | b'\n') {
            if byte == b'\n'
                && source
                    .get(index + 1..)
                    .map(str::trim_start)
                    .and_then(|source| source.bytes().next())
                    .is_some_and(|byte| b".?:+-*/|&,=)]}".contains(&byte))
            {
                continue;
            }
            return index;
        }
    }
    source.len()
}

#[derive(Clone)]
struct Import {
    name: String,
    specifier: String,
    imported: Option<String>,
    harness: bool,
    app: bool,
}

fn imports(source: &Text) -> Result<Vec<Import>, Error> {
    let source = ascii(source);
    let mut imports = Vec::new();
    for capture in
        regex(r#"(?m)^\s*import\s+([\s\S]*?)\s+from\s+['"]([^'"]+)['"]"#)?.captures_iter(&source)
    {
        let clause = capture.get(1).map_or("", |value| value.as_str());
        let specifier = capture.get(2).map_or("", |value| value.as_str());
        let harness = HARNESS.contains(&specifier);
        let app = !harness
            && (["node:child_process", "child_process"].contains(&specifier)
                || !specifier.starts_with("node:")
                    && !regex(r"^(fs|path|os|url|util|crypto)(/|$)")?.is_match(specifier));
        let mut names = Vec::new();
        if let Some(capture) = regex(r"\*\s+as\s+([A-Za-z_$][\w$]*)")?.captures(clause) {
            if let Some(name) = capture.get(1) {
                names.push((name.as_str().to_owned(), None));
            }
        }
        if let Some(capture) = regex(r"\{([^}]*)\}")?.captures(clause) {
            for part in capture.get(1).map_or("", |value| value.as_str()).split(',') {
                let pieces = regex(r"\s+as\s+")?.split(part.trim()).collect::<Vec<_>>();
                let local = pieces.last().copied().unwrap_or("").trim();
                if regex(r"^[A-Za-z_$][\w$]*$")?.is_match(local) {
                    names.push((
                        local.to_owned(),
                        pieces.first().map(|value| value.trim().to_owned()),
                    ));
                }
            }
        }
        if !clause.trim().starts_with(['{', '*']) {
            if let Some(capture) = regex(r"^([A-Za-z_$][\w$]*)")?.captures(clause.trim()) {
                if let Some(name) = capture.get(1) {
                    names.push((name.as_str().to_owned(), None));
                }
            }
        }
        for (name, imported) in names {
            imports.push(Import {
                name,
                imported,
                specifier: specifier.to_owned(),
                harness,
                app,
            });
        }
    }
    for capture in regex(
        r"\b(?:const|let|var)\s+(\{[^}]*\}|[A-Za-z_$][\w$]*)\s*=\s*await\s+import\(([^\n;]*)\)",
    )?
    .captures_iter(&source)
    {
        let binding = capture.get(1).map_or("", |value| value.as_str());
        let argument = capture.get(2).map_or("", |value| value.as_str()).trim();
        let literal = argument
            .strip_prefix(['"', '\''])
            .and_then(|value| value.strip_suffix(['"', '\'']));
        if literal.is_some_and(|literal| {
            HARNESS.contains(&literal)
                || literal.starts_with("node:")
                || ["fs", "path", "os", "url", "util", "crypto"]
                    .iter()
                    .any(|name| literal == *name || literal.starts_with(&format!("{name}/")))
        }) {
            continue;
        }
        let names = if let Some(binding) = binding
            .strip_prefix('{')
            .and_then(|binding| binding.strip_suffix('}'))
        {
            binding
                .split(',')
                .map(|part| part.rsplit(':').next().unwrap_or("").trim().to_owned())
                .collect::<Vec<_>>()
        } else {
            vec![binding.to_owned()]
        };
        for name in names {
            if regex(r"^[A-Za-z_$][\w$]*$")?.is_match(&name) {
                imports.push(Import {
                    name,
                    imported: None,
                    specifier: literal.unwrap_or(argument).to_owned(),
                    harness: false,
                    app: true,
                });
            }
        }
    }
    Ok(imports)
}

#[derive(Clone)]
struct Call {
    open: usize,
    close: usize,
}

fn calls(
    source: &str,
    names: &BTreeSet<String>,
    instances: &BTreeSet<String>,
) -> Result<Vec<Call>, Error> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let pattern = format!(
        r"(?:new\s+)?({})((?:\s*\.\s*[A-Za-z_$][\w$]*)*)\s*\(",
        names
            .iter()
            .map(|name| regex_lite::escape(name))
            .collect::<Vec<_>>()
            .join("|")
    );
    let mut result = Vec::new();
    for capture in regex(&pattern)?.captures_iter(source) {
        let Some(whole) = capture.get(0) else {
            continue;
        };
        if !boundary(source, whole.start(), true) {
            continue;
        }
        let name = capture.get(1).map_or("", |value| value.as_str());
        let members = capture.get(2).map_or("", |value| value.as_str());
        let member = regex(r"([A-Za-z_$][\w$]*)\s*$")?
            .captures(members)
            .and_then(|capture| capture.get(1).map(|value| value.as_str().to_owned()));
        if instances.contains(name)
            && member
                .as_ref()
                .is_none_or(|member| BUILTINS.contains(&member.as_str()))
        {
            continue;
        }
        let open = whole.end().saturating_sub(1);
        result.push(Call {
            open,
            close: close(source, open).unwrap_or(source.len()),
        });
    }
    Ok(result)
}

fn helpers(source: &Text, masked: &str, app: &BTreeSet<String>) -> Result<BTreeSet<String>, Error> {
    let promisifiers = imports(source)?
        .into_iter()
        .filter(|import| {
            ["node:util", "util"].contains(&import.specifier.as_str())
                && import.imported.as_deref() == Some("promisify")
        })
        .map(|import| import.name)
        .collect::<BTreeSet<_>>();
    let mut aliases = Vec::new();
    for capture in regex(r"(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*([A-Za-z_$][\w$]*)\s*\(\s*([A-Za-z_$][\w$]*)\s*\)\s*;")?.captures_iter(masked) {
        if capture.get(2).is_some_and(|name| promisifiers.contains(name.as_str())) {
            aliases.push((capture.get(1).map_or("", |value| value.as_str()).to_owned(), capture.get(3).map_or("", |value| value.as_str()).to_owned()));
        }
    }
    let pattern = r"(?:^|[^\w$.])(?:async\s+)?function\s*\*?\s*([A-Za-z_$][\w$]*)\s*\(|(?:^|[^\w$.])(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(?:async\s+)?(?:function\b|\([^)]*\)\s*=>|[A-Za-z_$][\w$]*\s*=>)";
    let mut definitions = Vec::new();
    for capture in regex(pattern)?.captures_iter(masked) {
        let whole = capture
            .get(0)
            .ok_or_else(|| Error::type_error("Missing helper match"))?;
        let name = capture
            .get(1)
            .or_else(|| capture.get(2))
            .map_or("", |value| value.as_str());
        let start = whole.start();
        let end = if capture.get(1).is_some() {
            masked
                .get(start..)
                .and_then(|rest| rest.find(')'))
                .map(|end| start + end)
                .and_then(|end| {
                    masked
                        .get(end..)
                        .and_then(|rest| rest.find('{'))
                        .map(|brace| end + brace)
                })
                .and_then(|brace| close(masked, brace))
                .unwrap_or(masked.len())
        } else {
            let body_start = if whole.as_str().ends_with("=>") {
                whole.end()
            } else {
                masked
                    .get(start..)
                    .and_then(|rest| rest.find('='))
                    .map_or(masked.len(), |equal| start + equal + 1)
            };
            let first = masked
                .get(body_start..)
                .and_then(|rest| rest.bytes().position(|byte| !byte.is_ascii_whitespace()))
                .map_or(masked.len(), |offset| body_start + offset);
            statement_end(masked, first)
        };
        let body = code_slice(masked, start, end.saturating_add(1));
        let body = body
            .find(name)
            .and_then(|index| body.get(index + name.len()..))
            .unwrap_or(body);
        definitions.push((name.to_owned(), body));
    }
    let mut helpers = BTreeSet::new();
    loop {
        let before = helpers.len();
        let sinks = app.union(&helpers).cloned().collect::<BTreeSet<_>>();
        for (name, wrapped) in &aliases {
            if sinks.contains(wrapped) {
                helpers.insert(name.clone());
            }
        }
        for (name, body) in &definitions {
            if !app.contains(name) && !calls(body, &sinks, &BTreeSet::new())?.is_empty() {
                helpers.insert(name.clone());
            }
        }
        if helpers.len() == before {
            break;
        }
    }
    Ok(helpers)
}

#[derive(Clone)]
struct Declaration {
    names: Vec<String>,
    start: usize,
    end: usize,
}

fn declarations(source: &str) -> Result<Vec<Declaration>, Error> {
    let mut result = Vec::new();
    for capture in regex(r"(?:const|let|var)\s+([A-Za-z_$][\w$]*|\{[^}]*\}|\[[^\]]*\])\s*=")?
        .captures_iter(source)
    {
        let whole = capture
            .get(0)
            .ok_or_else(|| Error::type_error("Missing declaration match"))?;
        let names = regex(r"[A-Za-z_$][\w$]*")?
            .find_iter(capture.get(1).map_or("", |value| value.as_str()))
            .filter(|name| !KEYWORDS.contains(&name.as_str()))
            .map(|name| name.as_str().to_owned())
            .collect();
        result.push(Declaration {
            names,
            start: whole.end(),
            end: statement_end(source, whole.end()),
        });
    }
    Ok(result)
}

fn instances(source: &str, sinks: &BTreeSet<String>) -> Result<BTreeSet<String>, Error> {
    let declarations = declarations(source)?;
    let mut instances = BTreeSet::new();
    loop {
        let before = instances.len();
        let current = sinks.union(&instances).cloned().collect::<BTreeSet<_>>();
        for declaration in &declarations {
            if declaration.names.iter().all(|name| current.contains(name)) {
                continue;
            }
            if !calls(
                code_slice(source, declaration.start, declaration.end.saturating_add(1)),
                &current,
                &instances,
            )?
            .is_empty()
            {
                instances.extend(declaration.names.iter().cloned());
            }
        }
        if instances.len() == before {
            break;
        }
    }
    Ok(instances)
}

struct Block {
    title: Text,
    start: usize,
    end: usize,
    literal_start: usize,
}

fn blocks(raw: &Text, masked: &str) -> Result<Vec<Block>, Error> {
    let mut result = Vec::new();
    for whole in regex(r"(test|it)(?:\.only|\.skip|\.todo)?\s*\(")?.find_iter(masked) {
        if !boundary(masked, whole.start(), true) {
            continue;
        }
        let open = whole.end() - 1;
        let Some(end) = close(masked, open) else {
            continue;
        };
        let start = masked
            .get(open + 1..)
            .and_then(|rest| rest.bytes().position(|byte| !byte.is_ascii_whitespace()))
            .map_or(end, |index| open + 1 + index);
        let quote = raw
            .0
            .get(start)
            .copied()
            .filter(|unit| [34, 39, 96].contains(unit));
        let title = if let Some(quote) = quote {
            raw.0
                .get(start + 1..end)
                .and_then(|rest| rest.iter().position(|unit| *unit == quote))
                .filter(|length| (1..=200).contains(length))
                .map(|length| text_slice(raw, start + 1, start + 1 + length))
                .unwrap_or_else(|| Text::from("(untitled)"))
        } else {
            Text::from("(untitled)")
        };
        result.push(Block {
            title,
            start: open,
            end,
            literal_start: start,
        });
    }
    Ok(result)
}

#[derive(Clone)]
struct Literal {
    value: Text,
    start: usize,
    end: usize,
}

fn literals(raw: &Text, masked: &str) -> Vec<Literal> {
    let mut result = Vec::new();
    let mut index = 0;
    while let Some(byte) = masked.as_bytes().get(index) {
        if !b"\"'`".contains(byte) {
            index += 1;
            continue;
        }
        let Some(length) = masked
            .as_bytes()
            .get(index + 1..)
            .and_then(|rest| rest.iter().position(|candidate| candidate == byte))
        else {
            break;
        };
        let end = index + 1 + length;
        let value = text_slice(raw, index + 1, end);
        if !contains(&value, &Text::from("${")) {
            result.push(Literal {
                value,
                start: index,
                end,
            });
        }
        index = end + 1;
    }
    result
}

fn absence(source: &str) -> Result<bool, Error> {
    Ok(regex(r"doesNotMatch\s*\(|notEqual\s*\(|notStrictEqual\s*\(|!\s*[\w$.()\[\]'`-]*\.includes\s*\(|\.includes\s*\([^\n]*\)\s*,\s*false\b|\.includes\s*\([^\n]*\)\s*===?\s*false\b|\.indexOf\s*\([^\n]*\)\s*===?\s*-1\b|\.(?:not\.)?(?:toContain|toMatch)\b")?.is_match(source))
}

fn seeded(raw: &Text, masked: &str) -> Result<Vec<(Text, Option<String>)>, Error> {
    let titles = blocks(raw, masked)?
        .into_iter()
        .map(|block| block.literal_start)
        .collect::<BTreeSet<_>>();
    let literals = literals(raw, masked)
        .into_iter()
        .filter(|literal| literal.value.0.len() >= 6 && !titles.contains(&literal.start))
        .collect::<Vec<_>>();
    let mut bound = Vec::new();
    for capture in
        regex(r#"(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(['"`])"#)?.captures_iter(masked)
    {
        let Some(whole) = capture.get(0) else {
            continue;
        };
        if let Some(literal) = literals
            .iter()
            .find(|literal| literal.start == whole.end() - 1)
        {
            let tail = code_slice(masked, literal.end + 1, literal.end + 3);
            if tail.is_empty() || regex(r"^\s*[;\n]")?.is_match(tail) {
                bound.push((
                    capture.get(1).map_or("", |value| value.as_str()).to_owned(),
                    literal.value.clone(),
                ));
            }
        }
    }
    let mut sentinels: Vec<(Text, Option<String>)> = Vec::new();
    let mut offset = 0;
    for line in masked.split('\n') {
        let start = offset;
        offset += line.len() + 1;
        if !absence(line)? {
            continue;
        }
        let end = start + line.len();
        for (name, value) in &bound {
            let reached = line.match_indices(name).any(|(index, _)| {
                boundary(line, index, false)
                    && (index < 2
                        || line.as_bytes().get(index - 1) != Some(&b'.')
                        || line.as_bytes().get(index - 2) == Some(&b'.'))
                    && line
                        .as_bytes()
                        .get(index + name.len())
                        .is_none_or(|byte| !word(*byte))
            });
            if reached {
                if let Some(entry) = sentinels.iter_mut().find(|(sentinel, _)| sentinel == value) {
                    entry.1 = Some(name.clone());
                } else {
                    sentinels.push((value.clone(), Some(name.clone())));
                }
            }
        }
        let mut candidates = literals
            .iter()
            .filter(|literal| literal.start >= start && literal.start < end)
            .map(|literal| (literal.value.clone(), true))
            .collect::<Vec<_>>();
        let raw_line = ascii(&text_slice(raw, start, end));
        for capture in regex(r"/((?:\\.|[^/\n\\])+)/[a-z]*")?.captures_iter(&raw_line) {
            let body = capture.get(1).map_or("", |value| value.as_str());
            let body = regex(r"\\(.)")?.replace_all(body, "$1");
            if body.len() >= 6 {
                candidates.push((Text::from(body.as_ref()), false));
            }
        }
        for (candidate, exact) in candidates {
            if let Some(literal) = literals.iter().find(|literal| {
                (literal.start < start || literal.start >= end)
                    && if exact {
                        literal.value == candidate
                    } else {
                        contains(&literal.value, &candidate)
                    }
            }) {
                if !sentinels.iter().any(|(value, _)| *value == literal.value) {
                    sentinels.push((literal.value.clone(), None));
                }
            }
        }
    }
    Ok(sentinels)
}

fn tainted(
    source: &str,
    raw: &Text,
    value: &Text,
    sinks: &BTreeSet<String>,
    instances: &BTreeSet<String>,
) -> Result<bool, Error> {
    let ranges = literals(raw, source)
        .into_iter()
        .filter(|literal| {
            contains(&literal.value, value)
                || literal.value.0.len() >= 6 && contains(value, &literal.value)
        })
        .collect::<Vec<_>>();
    let declarations = declarations(source)?;
    let mut names = BTreeSet::new();
    let has_taint = |start: usize, end: usize, names: &BTreeSet<String>| {
        ranges
            .iter()
            .any(|range| range.start >= start && range.start < end)
            || names.iter().any(|name| {
                code_slice(source, start, end)
                    .match_indices(name)
                    .any(|(index, _)| {
                        let slice = code_slice(source, start, end);
                        boundary(slice, index, false)
                            && (index == 0
                                || !b"'\"".contains(slice.as_bytes().get(index - 1).unwrap_or(&0)))
                            && (index < 2
                                || slice.as_bytes().get(index - 1) != Some(&b'.')
                                || slice.as_bytes().get(index - 2) == Some(&b'.'))
                            && slice
                                .as_bytes()
                                .get(index + name.len())
                                .is_none_or(|byte| !word(*byte))
                    })
            })
    };
    loop {
        let before = names.len();
        for declaration in &declarations {
            if !declaration.names.iter().all(|name| names.contains(name))
                && has_taint(declaration.start, declaration.end, &names)
            {
                names.extend(declaration.names.iter().cloned());
            }
        }
        if before == names.len() {
            break;
        }
    }
    for whole in regex(r"process\.env(?:\.[A-Za-z_$][\w$]*|\[[^\]]+\])\s*=")?.find_iter(source) {
        if source.as_bytes().get(whole.end()) != Some(&b'=')
            && has_taint(whole.end(), statement_end(source, whole.end()), &names)
        {
            return Ok(true);
        }
    }
    Ok(calls(source, sinks, instances)?
        .iter()
        .any(|call| has_taint(call.open, call.close, &names)))
}

pub fn adds_test_evidence(added: &Text) -> Result<bool, Error> {
    if crate::text::trim(&added.0).is_empty() {
        return Ok(false);
    }
    let masked = mask(added)?;
    if !blocks(added, &masked)?.is_empty() {
        return Ok(true);
    }
    if regex(r"#\[\s*(?:(?:tokio|async_std)::)?test\b|\bassert(?:_eq|_ne|_matches)?!\s*\(")?
        .is_match(&masked)
    {
        return Ok(true);
    }
    Ok(
        regex(r"(?:assert(?:\.[A-Za-z]+)?|expect|t\.assert\.[A-Za-z]+)\s*\(")?
            .find_iter(&masked)
            .any(|whole| boundary(&masked, whole.start(), false)),
    )
}

pub fn adds_only_imports(added: &Text) -> Result<bool, Error> {
    if crate::text::trim(&added.0).is_empty() {
        return Ok(false);
    }
    let source = ascii(added);
    let source = regex(r"(?s)/\*.*?\*/")?.replace_all(&source, "");
    let source = regex(r"(?m)^\s*//.*$")?.replace_all(&source, "");
    let source = regex(r#"\bimport\s+(?:[\w$*{}\s,]+?\s+from\s+)?['"][^'"\n]+['"]\s*;?"#)?
        .replace_all(&source, "");
    let source =
        regex(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?use\s+[^;]+;\s*$")?.replace_all(&source, "");
    let source = regex(r"(?m)^\s*;\s*$")?.replace_all(&source, "");
    Ok(source.trim().is_empty())
}

pub fn analyze(file: &Text, source: &Text, added: &Text) -> Result<Vec<Text>, Error> {
    if crate::text::trim(&added.0).is_empty() {
        return Ok(Vec::new());
    }
    let imports = imports(source)?;
    let app = imports
        .iter()
        .filter(|import| import.app)
        .map(|import| import.name.clone())
        .collect::<BTreeSet<_>>();
    let sinks = imports
        .iter()
        .filter(|import| !import.harness)
        .map(|import| import.name.clone())
        .collect::<BTreeSet<_>>();
    let masked = mask(source)?;
    let helpers = helpers(source, &masked, &app)?;
    let app_names = app.union(&helpers).cloned().collect::<BTreeSet<_>>();
    let instances = instances(&masked, &app_names)?;
    let app_sinks = app_names
        .union(&instances)
        .cloned()
        .collect::<BTreeSet<_>>();
    let all_sinks = sinks
        .union(&helpers)
        .cloned()
        .chain(instances.iter().cloned())
        .collect::<BTreeSet<_>>();
    let masked_added = mask(added)?;
    let mut reasons = Vec::new();
    if !app.is_empty() {
        for block in blocks(added, &masked_added)? {
            let body = code_slice(&masked_added, block.start, block.end + 1);
            let constant = app.iter().any(|name| {
                regex(r"^[A-Z][A-Z0-9_]*$").is_ok_and(|pattern| pattern.is_match(name))
                    && named(body, name)
            });
            let reads = regex(r"(?:readFile|readFileSync|readdir|readdirSync)\s*\(")?
                .find_iter(body)
                .any(|whole| boundary(body, whole.start(), false));
            let raw_body = ascii(&text_slice(added, block.start, block.end + 1));
            let raw_source = ascii(source);
            let path = regex(r#"['"`](?:\.{1,2}/)+(?:src|lib|bin|scripts)(?:/[^'"`\n]*)?['"`]"#)?;
            let scans = reads
                && (path.is_match(&raw_body)
                    || path.is_match(&raw_source) && raw_source.contains("import.meta.url"));
            if calls(body, &app_sinks, &instances)?.is_empty() && !constant && !scans {
                let mut reason = Text::from("Test substance: new test \"");
                reason.0.extend(block.title.0.into_iter().take(80));
                reason.push_str("\" in ");
                reason.0.extend(&file.0);
                reason.push_str(" never calls imported app code; its assertions only inspect values the test built, so they cannot catch a regression.");
                reasons.push(reason);
            }
        }
    }
    for (value, name) in seeded(added, &masked_added)? {
        if tainted(&masked_added, added, &value, &all_sinks, &instances)? {
            continue;
        }
        let mut reason = Text::from("Test substance: sentinel ");
        if let Some(name) = name {
            reason.push_str(&name);
        } else {
            reason.0.push(39);
            reason.0.extend(value.0.into_iter().take(40));
            reason.0.push(39);
        }
        reason.push_str(" in ");
        reason.0.extend(&file.0);
        reason.push_str(" is asserted absent but never passed to app code (arguments, config, or process.env); the assertion cannot fail.");
        reasons.push(reason);
    }
    Ok(reasons)
}

pub fn sentinel_values(added: &Text) -> Result<Vec<Text>, Error> {
    if crate::text::trim(&added.0).is_empty() {
        return Ok(Vec::new());
    }
    Ok(seeded(added, &mask(added)?)?
        .into_iter()
        .filter(|(value, _)| {
            value.0.len() >= 8
                && value
                    .0
                    .iter()
                    .any(|unit| matches!(unit, 65..=90 | 97..=122))
        })
        .map(|(value, _)| value)
        .collect())
}

pub fn sentinel_special_cases(app: &[(Text, Text)], sentinels: &[Text]) -> Vec<Text> {
    let mut reasons = Vec::new();
    for (file, source) in app {
        if let Some(hit) = sentinels.iter().find(|value| contains(source, value)) {
            let mut reason = Text::from("Test substance: ");
            reason.0.extend(&file.0);
            reason.push_str(" special-cases the test sentinel '");
            reason.0.extend(hit.0.iter().take(40));
            reason.push_str("'; app code must not match test values. Remove it and make the test feed the sentinel through a realistic input (an env secret, an api_key assignment, or a credential-shaped context) that the general behavior handles.");
            reasons.push(reason);
        }
    }
    reasons
}

pub fn added_lines(diff: &Text) -> Result<Vec<(String, Text)>, Error> {
    let source = diff.to_utf8()?;
    let mut files: Vec<(String, Vec<Text>)> = Vec::new();
    let mut current = None;
    for line in source.split('\n') {
        if let Some(capture) = regex(r"^diff --git a/(.+) b/(.+)$")?.captures(line) {
            let name = capture.get(2).map_or("", |value| value.as_str()).to_owned();
            if let Some(index) = files.iter().position(|(file, _)| file == &name) {
                files.remove(index);
            }
            files.push((name, Vec::new()));
            current = Some(files.len() - 1);
            continue;
        }
        let Some(index) = current else { continue };
        let entry = files
            .get_mut(index)
            .ok_or_else(|| Error::type_error("Diff file index is invalid"))?;
        if line.starts_with("@@") {
            entry.1.push(Text::from(";"));
        } else if line.starts_with('+') && !line.starts_with("+++") {
            entry.1.push(Text::from(line.get(1..).unwrap_or("")));
        }
    }
    Ok(files
        .into_iter()
        .map(|(file, lines)| {
            let mut source = Text::default();
            for (index, line) in lines.into_iter().enumerate() {
                if index != 0 {
                    source.0.push(10);
                }
                source.0.extend(line.0);
            }
            (file, source)
        })
        .collect())
}
