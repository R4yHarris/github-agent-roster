use std::collections::BTreeSet;
use std::path::Path;

use super::*;
use crate::planning;

pub const RULE_LAYERS: &str = "1. Human policy and protected surfaces: enforced by tools and gates, not negotiable in prose.\n2. Org rules: the contracts SDK, identity, and trailers; applied by the harness at publication.\n3. Repo rules: the AGENTS.md coding sections below.\n4. Area rules: conventions derived from this repository (below) and notes in the modules you read.\n5. Task rules: TASK.md checks and allowed files.\n6. Seat principal: professional conduct for this seat.\nA higher layer wins on conflict; a lower layer may narrow, never widen, a higher one.";

fn slice(source: &str, units: usize) -> Text {
    Text(source.encode_utf16().take(units).collect())
}
fn bounded(source: Text, limit: usize) -> Text {
    if source.0.len() <= limit {
        return source;
    }
    let end = source
        .0
        .iter()
        .take(limit + 1)
        .rposition(|unit| *unit == 10)
        .unwrap_or_else(|| source.0.len().saturating_sub(1));
    let mut result = Text(source.0.iter().take(end).copied().collect());
    result.push_str("\n[Truncated]");
    result
}

fn sorted(names: Vec<String>) -> Result<Vec<String>, Error> {
    let mut keyed = names
        .into_iter()
        .map(|name| evidence::name_key(&name).map(|key| (key, name)))
        .collect::<Result<Vec<_>, _>>()?;
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(keyed.into_iter().map(|(_, name)| name).collect())
}

pub fn module_exports(root: &Path, modules: &[String], limit: usize) -> Result<Text, Error> {
    let js = regex(
        r"^export\s+(?:default\s+)?(?:async\s+)?(?:function\*?|class|const|let)\s+[A-Za-z_$][\w$]*",
    )?;
    let rust = regex(
        r"^pub(?:\([^)]*\))?\s+(?:async\s+)?(?:fn|struct|enum|trait|type|const|static)\s+[A-Za-z_][A-Za-z0-9_]*",
    )?;
    let mut blocks = Text::default();
    for module in modules {
        if planning::forbidden_read(module) {
            continue;
        }
        let Some(text) = evidence::read_product(root, module, 262_144)? else {
            continue;
        };
        let source = text.to_utf8()?.replace("\r\n", "\n");
        let lines = source.split('\n').collect::<Vec<_>>();
        let mut signatures = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            if !(js.is_match(line) || module.ends_with(".rs") && rust.is_match(line)) {
                continue;
            }
            let mut signature = (*line).to_owned();
            for next in lines.iter().skip(index + 1).take(5) {
                if signature.matches('(').count() <= signature.matches(')').count() {
                    break;
                }
                signature.push(' ');
                signature.push_str(js_trim(next));
            }
            let signature = regex(r"\s+")?.replace_all(&signature, " ");
            let signature = regex(r"\s*(?:=>\s*)?\{\s*\}?\s*$")?.replace(&signature, "");
            let signature = js_trim(&signature);
            let mut value = Text::from("- ");
            if signature.encode_utf16().count() > 200 {
                value.0.extend(slice(signature, 197).0);
                value.push_str("...");
            } else {
                value.push_str(signature);
            }
            signatures.push(value);
            if signatures.len() == 30 {
                break;
            }
        }
        if signatures.is_empty() {
            continue;
        }
        if !blocks.0.is_empty() {
            blocks.push_str("\n\n");
        }
        blocks.push_str(&format!("`{module}`"));
        for signature in signatures {
            blocks.0.push(10);
            blocks.0.extend(signature.0);
        }
    }
    Ok(bounded(blocks, limit))
}

pub fn public_seams(root: &Path, files: &[String], limit: usize) -> Result<Text, Error> {
    let pattern = regex(r#"from\s+['"](\.[^'"]+)['"]"#)?;
    let extension = regex(r"\.[cm]?js$")?;
    let mut modules = BTreeSet::new();
    for file in files {
        if file.contains('*') || !extension.is_match(file) {
            continue;
        }
        let Some(source) = evidence::read_product(root, file, 262_144)? else {
            continue;
        };
        for capture in pattern.captures_iter(&source.to_utf8()?) {
            let Some(relative) = capture.get(1) else {
                continue;
            };
            let parent = root
                .join(file)
                .parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| Error::type_error("Module import requires a parent"))?;
            let target = crate::paths::resolve(&parent, Path::new(relative.as_str()))?;
            let Ok(target) = target.strip_prefix(root) else {
                continue;
            };
            let mut target = target
                .to_str()
                .ok_or_else(|| Error::type_error("Public seam path must be Unicode"))?
                .replace('\\', "/");
            if !extension.is_match(&target) {
                target.push_str(".mjs");
            }
            if !planning::forbidden_read(&target) && !files.contains(&target) {
                modules.insert(target);
            }
        }
    }
    let stem = |file: &str| {
        regex(r"\.(test|spec)?\.?[cm]?js$").map(|pattern| {
            pattern
                .replace(file.rsplit('/').next().unwrap_or(""), "")
                .into_owned()
        })
    };
    let stems = files
        .iter()
        .map(|file| stem(file))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let names = sorted(modules.into_iter().collect())?;
    let mut preferred = Vec::new();
    let mut others = Vec::new();
    for name in names {
        let base = extension
            .replace(name.rsplit('/').next().unwrap_or(""), "")
            .into_owned();
        if stems.contains(&base) {
            preferred.push(name);
        } else {
            others.push(name);
        }
    }
    preferred.extend(others);
    module_exports(root, &preferred, limit)
}

pub fn source_files(
    root: &Path,
    directory: &str,
    limit: usize,
    skip_build: bool,
) -> Result<Vec<String>, Error> {
    let mut queue = vec![directory.to_owned()];
    let mut result = Vec::new();
    let mut visited = 0;
    while let Some(file) = queue.pop() {
        visited += 1;
        if visited > 8192 || file.split('/').count() > 64 {
            return Err(denied(
                "Enrichment traversal exceeds its bounded entry count or depth",
            ));
        }
        if planning::forbidden_read(&file) || policy::managed(&file) {
            continue;
        }
        let target = if file.is_empty() {
            root.to_owned()
        } else {
            root.join(&file)
        };
        crate::state::secure::check_components(&target)?;
        let metadata = match std::fs::symlink_metadata(&target) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(crate::state::secure::io_error(
                    "Inspect enrichment entry",
                    &target,
                    error,
                ));
            }
        };
        if crate::state::secure::linked(&metadata) {
            continue;
        }
        if metadata.is_file() {
            result.push(file);
            if result.len() > limit {
                return Err(denied(
                    "Enrichment file list exceeds its bound; no filenames were silently discarded",
                ));
            }
            continue;
        }
        if !metadata.is_dir() {
            continue;
        }
        let name = file.rsplit('/').next().unwrap_or("");
        if skip_build
            && !name.is_empty()
            && (name.starts_with('.')
                || [
                    "node_modules",
                    "vendor",
                    "dist",
                    "build",
                    "coverage",
                    "out",
                    "target",
                ]
                .contains(&name))
        {
            continue;
        }
        let entries = match std::fs::read_dir(&target) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(crate::state::secure::io_error(
                    "Read enrichment directory",
                    &target,
                    error,
                ));
            }
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| {
                crate::state::secure::io_error("Read enrichment entry", &target, error)
            })?;
            names.push(
                entry
                    .file_name()
                    .into_string()
                    .map_err(|_| Error::type_error("Enrichment filenames must be Unicode"))?,
            );
        }
        queue.extend(sorted(names)?.into_iter().rev().map(|name| {
            if file.is_empty() {
                name
            } else {
                format!("{file}/{name}")
            }
        }));
    }
    Ok(result)
}

fn keys(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(domain::members)
        .map_or_else(Vec::new, |entries| {
            entries
                .iter()
                .filter_map(|(key, _)| key.to_utf8().ok())
                .collect()
        })
}

fn count_rank(values: &[String], lexical: bool) -> Result<Vec<(String, usize)>, Error> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for value in values {
        if let Some((_, count)) = counts.iter_mut().find(|(name, _)| name == value) {
            *count += 1;
        } else {
            counts.push((value.clone(), 1));
        }
    }
    if lexical {
        let mut keyed = counts
            .into_iter()
            .map(|(name, count)| evidence::name_key(&name).map(|key| (key, name, count)))
            .collect::<Result<Vec<_>, _>>()?;
        keyed.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
        return Ok(keyed
            .into_iter()
            .map(|(_, name, count)| (name, count))
            .collect());
    }
    counts.sort_by_key(|value| std::cmp::Reverse(value.1));
    Ok(counts)
}

fn case_of(name: &str) -> Result<Option<&'static str>, Error> {
    for (pattern, label) in [
        (r"^[a-z0-9]+(?:-[a-z0-9]+)+$", "kebab-case"),
        (r"^[a-z0-9]+(?:_[a-z0-9]+)+$", "snake_case"),
        (r"^[A-Z][a-zA-Z0-9]*$", "PascalCase"),
        (r"^[a-z][a-z0-9]*[A-Z][a-zA-Z0-9]*$", "camelCase"),
        (r"^[a-z0-9]+$", "lowercase"),
    ] {
        if regex(pattern)?.is_match(name) {
            return Ok(Some(label));
        }
    }
    Ok(None)
}

fn dominant(names: &[String], single: bool) -> Result<Value, Error> {
    let cases = names
        .iter()
        .map(|name| case_of(name))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let lowercase = cases.iter().any(|name| name == "lowercase");
    let counts = count_rank(
        &cases
            .into_iter()
            .filter(|case| case != "lowercase")
            .collect::<Vec<_>>(),
        true,
    )?;
    Ok(counts.first().map_or_else(
        || {
            if lowercase {
                Value::string("lowercase")
            } else {
                Value::Null
            }
        },
        |(name, _)| {
            Value::string(&if single && lowercase {
                format!("{name} (single words lowercase)")
            } else {
                name.clone()
            })
        },
    ))
}

pub fn conventions(root: &Path) -> Result<Value, Error> {
    let files = source_files(root, "", 2000, true)?;
    let source_pattern = regex(r"\.(?:[cm]?[jt]sx?|py|go|rs|rb|java|cs)$")?;
    let test_pattern = regex(
        r"(?:^|/)(?:__tests__/|tests?/)|\.(?:test|spec)\.[cm]?[jt]sx?$|(?:^|/)test_[^/]+\.py$",
    )?;
    let sources = files
        .iter()
        .filter(|file| source_pattern.is_match(file) && !file.ends_with(".d.ts"))
        .cloned()
        .collect::<Vec<_>>();
    let tests = sources
        .iter()
        .filter(|file| test_pattern.is_match(file))
        .cloned()
        .collect::<Vec<_>>();
    let modules = sources
        .iter()
        .filter(|file| !test_pattern.is_match(file))
        .cloned()
        .collect::<Vec<_>>();
    let mut languages = Vec::new();
    for file in &sources {
        for (pattern, name) in [
            (r"\.[cm]?js$|\.jsx$", "JavaScript"),
            (r"\.tsx?$|\.[cm]ts$", "TypeScript"),
            (r"\.py$", "Python"),
            (r"\.go$", "Go"),
            (r"\.rs$", "Rust"),
            (r"\.rb$", "Ruby"),
            (r"\.java$", "Java"),
            (r"\.cs$", "C#"),
        ] {
            if regex(pattern)?.is_match(file) {
                languages.push(name.to_owned());
                break;
            }
        }
    }
    let languages = count_rank(&languages, false)?
        .into_iter()
        .map(|(name, _)| name)
        .collect::<Vec<_>>();
    let package = if files.iter().any(|file| file == "package.json") {
        evidence::read_product(root, "package.json", 262_144)?
            .and_then(|source| crate::json::parse(&source).ok())
    } else {
        None
    };
    let deps = keys(
        package
            .as_ref()
            .and_then(|package| package.get("dependencies")),
    );
    let dev = keys(
        package
            .as_ref()
            .and_then(|package| package.get("devDependencies")),
    );
    let extensions = modules
        .iter()
        .map(|file| format!(".{}", file.rsplit('.').next().unwrap_or("")))
        .collect::<Vec<_>>();
    let extension = count_rank(&extensions, false)?
        .first()
        .map(|(name, _)| name.clone());
    let mut texts = Vec::new();
    for file in sources.iter().take(600) {
        if let Some(source) = evidence::read_product(root, file, 262_144)? {
            texts.push((file.clone(), source.to_utf8()?.replace("\r\n", "\n")));
        }
    }
    let mut esm = 0;
    let mut cjs = 0;
    for (file, source) in &texts {
        if !regex(r"\.[cm]?[jt]sx?$")?.is_match(file) {
            continue;
        }
        if regex(r"(?m)^\s*(?:import\s[^(]|export\s)")?.is_match(source) {
            esm += 1;
        }
        if regex(r#"\brequire\(\s*['"]|module\.exports\b|\bexports\.\w+\s*="#)?.is_match(source) {
            cjs += 1;
        }
    }
    let module_system = if !languages
        .iter()
        .any(|language| ["JavaScript", "TypeScript"].contains(&language.as_str()))
    {
        Value::Null
    } else if package
        .as_ref()
        .is_some_and(|package| domain::is_text(package.get("type"), "module"))
        || extension.as_deref() == Some(".mjs")
        || esm > cjs
    {
        Value::string("ES modules (import/export)")
    } else if cjs > 0 {
        Value::string("CommonJS (require/module.exports)")
    } else {
        Value::Null
    };
    let manager = [
        ("package-lock.json", "npm"),
        ("pnpm-lock.yaml", "pnpm"),
        ("yarn.lock", "yarn"),
        ("bun.lockb", "bun"),
        ("poetry.lock", "poetry"),
        ("uv.lock", "uv"),
        ("Cargo.lock", "cargo"),
        ("go.sum", "go"),
    ]
    .iter()
    .find(|(file, _)| files.iter().any(|candidate| candidate == file))
    .map(|(_, name)| *name)
    .or(package.as_ref().map(|_| "npm (no lockfile)"));
    let script = package
        .as_ref()
        .and_then(|package| package.get("scripts"))
        .and_then(|scripts| scripts.get("test"))
        .map(domain::string)
        .transpose()?
        .map(|text| text.to_utf8())
        .transpose()?
        .unwrap_or_default();
    let runner = if regex(r"\bvitest\b")?.is_match(&script) || dev.iter().any(|dep| dep == "vitest")
    {
        Some("vitest")
    } else if regex(r"\bjest\b")?.is_match(&script)
        || dev.iter().any(|dep| dep == "jest")
        || files.iter().any(|file| file.starts_with("jest.config."))
    {
        Some("jest")
    } else if regex(r"\bmocha\b")?.is_match(&script) || dev.iter().any(|dep| dep == "mocha") {
        Some("mocha")
    } else if regex(r"node\s+--test\b")?.is_match(&script)
        || texts.iter().any(|(_, source)| {
            regex(r#"from\s+['"]node:test['"]"#).is_ok_and(|pattern| pattern.is_match(source))
        })
    {
        Some("node:test")
    } else if regex(r"\bpytest\b")?.is_match(&script)
        || files
            .iter()
            .any(|file| ["pytest.ini", "conftest.py"].contains(&file.as_str()))
    {
        Some("pytest")
    } else {
        None
    };
    let dirs = tests
        .iter()
        .map(|file| {
            file.split_once('/')
                .map_or_else(|| "(root)".to_owned(), |(first, _)| format!("{first}/"))
        })
        .collect::<Vec<_>>();
    let test_dir = count_rank(&dirs, false)?
        .first()
        .map(|(name, _)| name.clone());
    let suffix = regex(r"\.((?:test|spec)\.[cm]?[jt]sx?)$")?;
    let suffixes = tests
        .iter()
        .filter_map(|file| {
            suffix
                .captures(file)
                .and_then(|capture| capture.get(1).map(|value| value.as_str().to_owned()))
        })
        .collect::<Vec<_>>();
    let test_suffix = count_rank(&suffixes, false)?
        .first()
        .map(|(name, _)| name.clone());
    let sharded = tests
        .iter()
        .filter(|file| {
            regex(r"\.[^./]+\.(?:test|spec)\.[cm]?[jt]sx?$")
                .is_ok_and(|pattern| pattern.is_match(file.rsplit('/').next().unwrap_or("")))
        })
        .count();
    let stem = |file: &str| -> Result<String, Error> {
        Ok(regex(r"(?:\.(?:test|spec))?\.[^.]+$")?
            .replace(file.rsplit('/').next().unwrap_or(""), "")
            .split('.')
            .next()
            .unwrap_or("")
            .to_owned())
    };
    let naming = dominant(
        &modules
            .iter()
            .map(|file| stem(file))
            .collect::<Result<Vec<_>, _>>()?,
        true,
    )?;
    let mut exports = Vec::new();
    for (file, source) in &texts {
        if test_pattern.is_match(file) {
            continue;
        }
        for capture in regex(r"(?m)^export\s+(?:default\s+)?(?:async\s+)?(?:function\*?|const|let|class)\s+([A-Za-z_$][\w$]*)")?.captures_iter(source) { if let Some(name) = capture.get(1) { exports.push(name.as_str().to_owned()); } }
        for capture in regex(r"\bexports\.([A-Za-z_$][\w$]*)\s*=")?.captures_iter(source) {
            if let Some(name) = capture.get(1) {
                exports.push(name.as_str().to_owned());
            }
        }
        if let Some(capture) = regex(r"module\.exports\s*=\s*\{([^}]*)\}")?.captures(source) {
            for name in capture.get(1).map_or("", |value| value.as_str()).split(',') {
                let name = name.split(':').next().unwrap_or("").trim();
                if regex(r"^[A-Za-z_$][\w$]*$")?.is_match(name) {
                    exports.push(name.to_owned());
                }
            }
        }
    }
    exports.retain(|name| !regex(r"^[A-Z][A-Z0-9_]+$").is_ok_and(|pattern| pattern.is_match(name)));
    let export_naming = dominant(&exports, false)?;
    let mut lint = Vec::new();
    for file in files.iter().filter(|file| !file.contains('/')) {
        for (pattern, name) in [
            (
                r"^(?:\.eslintrc(?:\.\w+)?|eslint\.config\.[cm]?[jt]s)$",
                "ESLint",
            ),
            (
                r"^(?:\.prettierrc(?:\.\w+)?|prettier\.config\.[cm]?js)$",
                "Prettier",
            ),
            (r"^biome\.jsonc?$", "Biome"),
            (r"^\.editorconfig$", "EditorConfig"),
            (r"^(?:ruff|\.ruff)\.toml$", "Ruff"),
        ] {
            if regex(pattern)?.is_match(file) && !lint.iter().any(|value| value == name) {
                lint.push(name.to_owned());
            }
        }
    }
    for (field, name) in [("eslintConfig", "ESLint"), ("prettier", "Prettier")] {
        if package
            .as_ref()
            .and_then(|package| package.get(field))
            .is_some_and(domain::truthy)
            && !lint.iter().any(|value| value == name)
        {
            lint.push(name.to_owned());
        }
    }
    let largest = |files: &[String]| -> Result<Value, Error> {
        let mut ranked = Vec::new();
        for file in files {
            let lines = texts
                .iter()
                .find(|(name, _)| name == file)
                .map_or(0, |(_, source)| {
                    source.split('\n').count() - usize::from(source.ends_with('\n'))
                });
            ranked.push((lines, evidence::name_key(file)?, file));
        }
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        Ok(ranked.first().map_or(Value::Null, |(lines, _, file)| {
            Value::object(vec![
                ("file", Value::string(file)),
                ("lines", Value::Number(*lines as f64)),
            ])
        }))
    };
    let mut importers = Vec::new();
    for (file, source) in &texts {
        if test_pattern.is_match(file) {
            continue;
        }
        for capture in
            regex(r#"(?:\bfrom\s+|\brequire\(\s*|\bimport\(\s*)['"](\.{1,2}/[^'"]+)['"]"#)?
                .captures_iter(source)
        {
            let relative = capture.get(1).map_or("", |value| value.as_str());
            let parent = root
                .join(file)
                .parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| Error::type_error("Import requires a parent"))?;
            let target = crate::paths::resolve(&parent, Path::new(relative))?;
            let Ok(target) = target.strip_prefix(root) else {
                continue;
            };
            let base = target
                .to_str()
                .ok_or_else(|| Error::type_error("Import must be Unicode"))?
                .replace('\\', "/");
            if let Some(target) = [
                base.clone(),
                format!("{base}.js"),
                format!("{base}.mjs"),
                format!("{base}.cjs"),
                format!("{base}.ts"),
                format!("{base}/index.js"),
            ]
            .into_iter()
            .find(|target| modules.contains(target))
            {
                if &target != file {
                    importers.push(target);
                }
            }
        }
    }
    let core = count_rank(&importers, true)?
        .into_iter()
        .take(5)
        .map(|(file, count)| {
            Value::object(vec![
                ("file", Value::string(&file)),
                ("importers", Value::Number(count as f64)),
            ])
        })
        .collect();
    Ok(Value::object(vec![
        ("languages", strings(&languages)),
        ("moduleSystem", module_system),
        (
            "sourceExtension",
            extension.as_deref().map_or(Value::Null, Value::string),
        ),
        ("packageManager", manager.map_or(Value::Null, Value::string)),
        ("runtimeDependencies", Value::Number(deps.len() as f64)),
        ("testRunner", runner.map_or(Value::Null, Value::string)),
        (
            "testDir",
            test_dir.as_deref().map_or(Value::Null, Value::string),
        ),
        (
            "testSuffix",
            test_suffix.as_deref().map_or(Value::Null, Value::string),
        ),
        ("shardedTests", Value::Number(sharded as f64)),
        ("testCount", Value::Number(tests.len() as f64)),
        ("fileNaming", naming),
        ("exportNaming", export_naming),
        ("lint", strings(&lint)),
        ("largestModule", largest(&modules)?),
        ("largestTest", largest(&tests)?),
        ("core", Value::Array(core)),
    ]))
}

pub fn conventions_text(c: &Value) -> Result<Text, Error> {
    let text_field = |name: &str| {
        c.get(name)
            .and_then(Value::as_string)
            .map(Text::to_utf8)
            .transpose()
    };
    let list = |name: &str| -> Result<Vec<String>, Error> {
        c.get(name)
            .and_then(domain::items)
            .unwrap_or(&[])
            .iter()
            .map(|value| text(value, "Convention list requires text"))
            .collect()
    };
    let languages = list("languages")?;
    let mut rows = Vec::new();
    if !languages.is_empty() {
        rows.push(format!(
            "- Language: {}{}{}.",
            languages.join(", "),
            text_field("moduleSystem")?.map_or(String::new(), |value| format!("; {value}")),
            text_field("sourceExtension")?.map_or(String::new(), |value| format!(
                "; source files use `{value}`"
            ))
        ));
    }
    if let Some(manager) = text_field("packageManager")? {
        let count = domain::safe_integer(c.get("runtimeDependencies")).unwrap_or(0);
        rows.push(format!(
            "- Packages: {manager}; {}.",
            if count > 0 {
                format!("{count} runtime dependencies")
            } else {
                "no runtime dependencies (do not add one)".to_owned()
            }
        ));
    }
    let count = domain::safe_integer(c.get("testCount")).unwrap_or(0);
    let runner = text_field("testRunner")?;
    if runner.is_some() || count > 0 {
        let layout =
            text_field("testDir")?.map_or(String::new(), |value| format!(" under `{value}`"));
        let shape = text_field("testSuffix")?.map_or(String::new(), |suffix| {
            format!(
                ", named `<module>.{suffix}`{}",
                if domain::safe_integer(c.get("shardedTests")).unwrap_or(0) > 0 {
                    format!(" or sharded `<module>.<topic>.{suffix}`")
                } else {
                    String::new()
                }
            )
        });
        rows.push(format!(
            "- Tests: {}; {count} files{layout}{shape}. Use the same runner and layout.",
            runner.as_deref().unwrap_or("unknown runner")
        ));
    }
    let naming = text_field("fileNaming")?;
    let exports = text_field("exportNaming")?;
    if naming.is_some() || exports.is_some() {
        rows.push(format!(
            "- Naming: {}{}{}.",
            naming
                .as_ref()
                .map_or(String::new(), |value| format!("files {value}")),
            if naming.is_some() && exports.is_some() {
                "; "
            } else {
                ""
            },
            exports.map_or(String::new(), |value| format!("exports {value}"))
        ));
    }
    let lint = list("lint")?;
    rows.push(format!(
        "- Lint/format config: {}.",
        if lint.is_empty() {
            "none; match the surrounding style".to_owned()
        } else {
            lint.join(", ")
        }
    ));
    if let Some(largest) = c
        .get("largestModule")
        .filter(|value| domain::safe_integer(value.get("lines")).unwrap_or(0) > 0)
    {
        let file = text(required(largest, "file")?, "Largest module requires a file")?;
        let lines = domain::safe_integer(largest.get("lines")).unwrap_or(0);
        let largest_test = c
            .get("largestTest")
            .filter(|value| domain::safe_integer(value.get("lines")).unwrap_or(0) > 0)
            .map(|value| {
                Ok::<_, Error>(format!(
                    "; largest test `{}` is {} lines",
                    text(required(value, "file")?, "Largest test requires a file")?,
                    domain::safe_integer(value.get("lines")).unwrap_or(0)
                ))
            })
            .transpose()?
            .unwrap_or_default();
        rows.push(format!("- Size: largest module `{file}` is {lines} lines{largest_test}. Keep files below these; add a new focused module or test shard instead of growing one."));
    }
    if let Some(core) = c
        .get("core")
        .and_then(domain::items)
        .filter(|core| !core.is_empty())
    {
        let values = core
            .iter()
            .map(|value| {
                Ok(format!(
                    "`{}` ({})",
                    text(required(value, "file")?, "Core module requires a file")?,
                    domain::safe_integer(value.get("importers")).unwrap_or(0)
                ))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        rows.push(format!(
            "- Core modules (most imported; read before adding parallel code): {}.",
            values.join(", ")
        ));
    }
    Ok(Text::from(rows.join("\n").as_str()))
}

pub fn field_contracts(
    root: &Path,
    files: &[String],
    task: &Text,
    limit: usize,
) -> Result<Text, Error> {
    let sources = source_files(root, "src", 2000, false)?
        .into_iter()
        .filter(|file| regex(r"\.[cm]?js$").is_ok_and(|pattern| pattern.is_match(file)))
        .collect::<Vec<_>>();
    let mut texts = Vec::new();
    for file in sources {
        if let Some(source) = evidence::read_product(root, &file, 262_144)? {
            texts.push((file, source.to_utf8()?));
        }
    }
    let mut accessed = Vec::new();
    let pattern = regex(r"(?:\b[a-z][A-Za-z0-9]*|[)\]])\??\.([a-z][a-z0-9]*[A-Z][A-Za-z0-9]*)\b")?;
    for file in files {
        if file.contains('*')
            || !regex(r"\.[cm]?js$")?.is_match(file)
            || planning::forbidden_read(file)
        {
            continue;
        }
        let source = if let Some((_, source)) = texts.iter().find(|(name, _)| name == file) {
            Some(source.clone())
        } else {
            evidence::read_product(root, file, 262_144)?
                .map(|source| source.to_utf8())
                .transpose()?
        };
        for capture in pattern.captures_iter(source.as_deref().unwrap_or("")) {
            let Some(whole) = capture.get(0) else {
                continue;
            };
            if source
                .as_ref()
                .and_then(|source| source.get(whole.end()..))
                .is_some_and(|tail| tail.trim_start().starts_with('('))
            {
                continue;
            }
            if let Some(field) = capture.get(1) {
                if !accessed.iter().any(|value| value == field.as_str()) {
                    accessed.push(field.as_str().to_owned());
                }
            }
        }
    }
    let mut named = Vec::new();
    for capture in regex(r"\b([a-z][a-z0-9]*[A-Z][A-Za-z0-9]*)\b")?.captures_iter(&task.to_utf8()?)
    {
        if let Some(field) = capture.get(1) {
            if !named.iter().any(|value| value == field.as_str()) {
                named.push(field.as_str().to_owned());
            }
        }
    }
    let mut candidates = named.clone();
    for field in accessed {
        if !candidates.contains(&field) {
            candidates.push(field);
        }
    }
    let mut blocks = Vec::new();
    for field in candidates {
        let mut lines = Vec::new();
        for (file, source) in &texts {
            for (index, line) in source.replace("\r\n", "\n").split('\n').enumerate() {
                if !word_match(line, &field) || !regex(r"\bmust\b|\bthrow\b|TypeError|RangeError|\.test\(|\btypeof\b|\binstanceof\b|Number\.is|Array\.isArray")?.is_match(line) { continue; }
                let trimmed = js_trim(line);
                let mut body = Text::from(format!("- {file}:{}: ", index + 1).as_str());
                if trimmed.encode_utf16().count() > 160 {
                    body.0.extend(slice(trimmed, 157).0);
                    body.push_str("...");
                } else {
                    body.push_str(trimmed);
                }
                lines.push((regex(r"\bmust\b")?.is_match(line), body));
            }
        }
        if !lines.iter().any(|(must, _)| *must) {
            continue;
        }
        lines.sort_by_key(|value| std::cmp::Reverse(value.0));
        let mut body = Text::from(format!("`{field}`").as_str());
        for (_, line) in lines.into_iter().take(3) {
            body.0.push(10);
            body.0.extend(line.0);
        }
        blocks.push((named.contains(&field), body));
    }
    blocks.sort_by_key(|value| std::cmp::Reverse(value.0));
    let mut kept = Text::default();
    let mut used = 0;
    for (_, body) in blocks.into_iter().take(6) {
        if used + body.0.len() > limit {
            if kept.0.is_empty() {
                kept.0
                    .extend(body.0.into_iter().take(limit.saturating_sub(3)));
                kept.push_str("...");
            }
            break;
        }
        if !kept.0.is_empty() {
            kept.push_str("\n\n");
        }
        used += body.0.len() + 2;
        kept.0.extend(body.0);
    }
    Ok(kept)
}

fn word_match(source: &str, field: &str) -> bool {
    source.match_indices(field).any(|(start, _)| {
        (start == 0
            || source
                .as_bytes()
                .get(start - 1)
                .is_none_or(|byte| !byte.is_ascii_alphanumeric() && !b"_$".contains(byte)))
            && source
                .as_bytes()
                .get(start + field.len())
                .is_none_or(|byte| !byte.is_ascii_alphanumeric() && !b"_$".contains(byte))
    })
}
