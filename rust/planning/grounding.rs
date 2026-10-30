use std::path::Path;

use super::*;
use crate::paths;
use crate::state::secure;

#[derive(Debug, Clone, Default)]
pub struct Index {
    pub symbols: Vec<String>,
    pub exports_by_file: Vec<(String, Vec<String>)>,
    pub files: Vec<String>,
}

impl Index {
    pub fn from_value(input: &Value) -> Result<Self, Error> {
        let list = |field: &str| -> Result<Vec<String>, Error> {
            array(
                required(input, field)?,
                "Grounding index fields must be arrays",
            )?
            .iter()
            .map(|item| text(item, "Grounding index entries must be text"))
            .collect()
        };
        let mut exports_by_file = Vec::new();
        if let Some(entries) = input.get("exportsByFile").and_then(domain::members) {
            for (file, names) in entries {
                exports_by_file.push((
                    encode(file),
                    array(names, "Grounding exports must be arrays")?
                        .iter()
                        .map(|name| text(name, "Grounding export must be text"))
                        .collect::<Result<Vec<_>, _>>()?,
                ));
            }
        } else if let Some(entries) = input.get("exportsByFile").and_then(domain::items) {
            for entry in entries {
                let pair = array(entry, "Grounding export entry must be a pair")?;
                let file = pair
                    .first()
                    .ok_or_else(|| Error::type_error("Missing grounding export path"))?;
                let names = pair
                    .get(1)
                    .ok_or_else(|| Error::type_error("Missing grounding exports"))?;
                exports_by_file.push((
                    text(file, "Grounding path must be text")?,
                    array(names, "Grounding exports must be arrays")?
                        .iter()
                        .map(|name| text(name, "Grounding export must be text"))
                        .collect::<Result<Vec<_>, _>>()?,
                ));
            }
        }
        Ok(Self {
            symbols: list("symbols")?,
            exports_by_file,
            files: list("files")?,
        })
    }

    pub fn as_value(&self) -> Value {
        Value::object(vec![
            ("symbols", strings(&self.symbols)),
            ("files", strings(&self.files)),
            (
                "exportsByFile",
                Value::Object(
                    self.exports_by_file
                        .iter()
                        .map(|(file, names)| (decode(file), strings(names)))
                        .collect(),
                ),
            ),
        ])
    }

    fn exports(&self, file: &str) -> &[String] {
        self.exports_by_file
            .iter()
            .find(|(name, _)| name == file)
            .map_or(&[], |(_, names)| names)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Context {
    pub index: Index,
    pub related: Vec<(String, Vec<String>)>,
    pub definitions: Text,
    pub test_names: Vec<String>,
}

fn code_file(file: &str) -> Result<bool, Error> {
    matches(r"\.(?:[cm]?js|[cm]?ts|jsx|tsx|rs)$", file)
}

pub fn is_code_slice(files: &[String]) -> Result<bool, Error> {
    let code = regex(r"\.(?:[cm]?js|[cm]?ts|jsx|tsx|rs)$")?;
    Ok(files.iter().any(|file| code.is_match(file)))
}

fn exported_names(source: &str) -> Result<Vec<String>, Error> {
    let mut items: Vec<String> = regex(r"(?m)^export\s+(?:default\s+)?(?:async\s+)?(?:function\*?|const|let|class)\s+([A-Za-z_$][\w$]*)")?
        .captures_iter(source)
        .filter_map(|captures| captures.get(1).map(|entry| entry.as_str().to_owned()))
        .collect();
    items.extend(
        regex(r"(?m)^\s*pub(?:\([^)]*\))?\s+(?:async\s+)?(?:fn|struct|enum|trait|type|const|static|mod)\s+([A-Za-z_][A-Za-z0-9_]*)")?
            .captures_iter(source)
            .filter_map(|captures| captures.get(1).map(|entry| entry.as_str().to_owned())),
    );
    Ok(distinct(items))
}

pub fn index_sources(files: &[String], sources: &[(String, Text)]) -> Result<Index, Error> {
    let mut result = Index {
        files: files.to_owned(),
        ..Index::default()
    };
    let token = regex(r"[A-Za-z_$][\w$]*")?;
    let commonjs = regex(r"\bexports\.([A-Za-z_$][\w$]*)\s*=")?;
    let code = regex(r"\.(?:[cm]?js|[cm]?ts|jsx|tsx|rs)$")?;
    let export_list = regex(r"(?m)^export\s*\{([^}]*)\}")?;
    let identifier = regex(r"^[A-Za-z_$][\w$]*$")?;
    let alias = regex(r"\s+as\s+")?;
    for file in files
        .iter()
        .filter(|file| !file.starts_with("vendor/") && !forbidden_read(file))
        .filter(|file| code.is_match(file))
        .take(1500)
    {
        let Some((_, source)) = sources.iter().find(|(name, _)| name == file) else {
            continue;
        };
        if source.utf8_len() > 512 * 1024 {
            continue;
        }
        let source = encode(source);
        for token in token.find_iter(&source) {
            let name = token.as_str().to_owned();
            if !result.symbols.contains(&name) {
                result.symbols.push(name);
            }
        }
        let mut names = exported_names(&source)?;
        names.extend(
            commonjs
                .captures_iter(&source)
                .filter_map(|captures| captures.get(1).map(|entry| entry.as_str().to_owned())),
        );
        for captures in export_list.captures_iter(&source) {
            for part in captures
                .get(1)
                .map_or("", |entry| entry.as_str())
                .split(',')
            {
                let name = alias.split(trim(part)).last().unwrap_or("");
                if identifier.is_match(name) {
                    names.push(name.to_owned());
                }
            }
        }
        result.exports_by_file.push((file.clone(), distinct(names)));
    }
    Ok(result)
}

fn stems(source: &str) -> Result<Vec<String>, Error> {
    let split = regex(r"([a-z0-9])([A-Z])")?
        .replace_all(source, "$1 $2")
        .to_lowercase();
    let stop = [
        "the", "and", "for", "with", "from", "into", "that", "this", "when", "than", "then",
        "each", "every", "must", "never", "only", "without", "under", "over", "their", "there",
        "issue", "issues", "feature", "add", "adds", "make", "keep", "use", "uses", "used", "not",
        "are", "can", "should", "will", "any", "all", "new", "test", "tests", "file", "files",
        "docs", "src", "lib", "mjs",
    ];
    Ok(distinct(
        regex("[^a-z0-9]+")?
            .split(&split)
            .filter(|word| word.len() >= 4 && !stop.contains(word))
            .map(|word| slice(word, 5))
            .collect(),
    ))
}

pub fn related_exports(
    sources: &[(String, Text)],
    files: &[String],
    ask: &str,
    limit: usize,
    exports_per_file: usize,
) -> Result<Vec<(String, Vec<String>)>, Error> {
    let terms = stems(ask)?;
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let pattern = regex(r"^src/.+\.(?:mjs|cjs|js|ts)$")?;
    let mut scored = Vec::new();
    for file in files.iter().filter(|file| pattern.is_match(file)).take(400) {
        let Some((_, source)) = sources.iter().find(|(name, _)| name == file) else {
            continue;
        };
        if source.0.len() > 256 * 1024 {
            continue;
        }
        let names = exported_names(&encode(source))?;
        if names.is_empty() {
            continue;
        }
        let file_hits = stems(file)?
            .iter()
            .filter(|stem| terms.contains(stem))
            .count();
        let mut ranked = names
            .iter()
            .map(|name| {
                Ok((
                    name.clone(),
                    stems(name)?
                        .iter()
                        .filter(|stem| terms.contains(stem))
                        .count(),
                ))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let score = file_hits * 2 + ranked.iter().map(|(_, hits)| (*hits).min(2)).sum::<usize>();
        if score < 2 {
            continue;
        }
        ranked.retain(|(_, hits)| *hits > 0);
        ranked.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        let exports = if ranked.is_empty() {
            names.into_iter().take(exports_per_file).collect()
        } else {
            ranked
                .into_iter()
                .take(exports_per_file)
                .map(|(name, _)| name)
                .collect()
        };
        scored.push((file.clone(), exports, score));
    }
    scored.sort_by(|left, right| right.2.cmp(&left.2).then_with(|| left.0.cmp(&right.0)));
    Ok(scored
        .into_iter()
        .take(limit)
        .map(|(file, exports, _)| (file, exports))
        .collect())
}

pub fn read_context(root: &Path, files: &[String], ask: &str) -> Result<Context, Error> {
    if files.len() > 1500 {
        return Err(Error::type_error(
            "Planning repository inventory exceeds 1500 files",
        ));
    }
    let mut sources = Vec::new();
    let mut test_names = Vec::new();
    let test_pattern = regex(r#"\b(?:test|it)\s*\(\s*['"]([^'"\n]+)['"]"#)?;
    for file in files {
        if forbidden_read(file)
            || file
                .split('/')
                .any(|part| part.eq_ignore_ascii_case("vendor"))
        {
            continue;
        }
        if !code_file(file)?
            && !file.starts_with("tests/")
            && !file.contains(".test.")
            && !file.contains(".spec.")
            && !matches(r"\.(?:json|bicep|ya?ml|toml|html|css|py|sh|ps1)$", file)?
        {
            continue;
        }
        let path = root.join(file);
        paths::ensure_local_path(&path, root)?;
        let status = std::fs::symlink_metadata(&path)
            .map_err(|error| secure::io_error("Inspect planning grounding file", &path, error))?;
        if !status.is_file() || secure::linked(&status) || status.len() > 512 * 1024 {
            continue;
        }
        let Some(snapshot) = secure::read(&path, 512 * 1024)? else {
            continue;
        };
        if paths::file_information(&path)?.links != 1 {
            return Err(secure::path_error(&path, "has multiple hard links"));
        }
        let source = String::from_utf8(snapshot.bytes)
            .map_err(|_| Error::type_error(format!("Planning input {file} is not UTF-8")))?;
        if file.starts_with("tests/") || file.contains(".test.") || file.contains(".spec.") {
            test_names.extend(
                test_pattern
                    .captures_iter(&source)
                    .filter_map(|captures| captures.get(1).map(|entry| entry.as_str().to_owned())),
            );
        }
        sources.push((file.clone(), Text::from(source.as_str())));
    }
    let readable = files
        .iter()
        .filter(|file| {
            !forbidden_read(file)
                && !file
                    .split('/')
                    .any(|part| part.eq_ignore_ascii_case("vendor"))
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut index = index_sources(&readable, &sources)?;
    let extra_token = regex(r"[A-Za-z_$][\w$]*")?;
    let non_code = regex(r"\.(?:json|bicep|ya?ml|toml|html|css|py|sh|ps1)$")?;
    for (file, source) in &sources {
        if !non_code.is_match(file) || source.utf8_len() > 512 * 1024 {
            continue;
        }
        let text_src = encode(source);
        for matched in extra_token.find_iter(&text_src) {
            let sym = matched.as_str().to_owned();
            if !index.symbols.contains(&sym) {
                index.symbols.push(sym);
            }
        }
    }
    let related = related_exports(&sources, &readable, ask, 6, 4)?;
    let mut blocks = Vec::new();
    let mut used = 0;
    for (file, names) in &related {
        let Some((_, source)) = sources.iter().find(|(name, _)| name == file) else {
            continue;
        };
        let source = encode(source).replace("\r\n", "\n");
        for name in names {
            let declaration = regex(&format!(
                r"^export\s+(?:default\s+)?(?:async\s+)?(?:function\*?|const|let|class)\s+{}\b",
                regex_lite::escape(name)
            ))?;
            let lines = source.split('\n').collect::<Vec<_>>();
            let Some(start) = lines.iter().position(|line| declaration.is_match(line)) else {
                continue;
            };
            let mut body = Vec::new();
            let mut depth = 0_i64;
            let mut opened = false;
            for line in lines.iter().skip(start).take(40) {
                body.push((*line).to_owned());
                let stripped = strip_strings(line);
                for c in stripped.chars() {
                    if c == '{' {
                        depth += 1;
                        opened = true;
                    } else if c == '}' {
                        depth -= 1;
                    }
                }
                if if opened {
                    depth <= 0
                } else {
                    trim_end(line).ends_with(';')
                } {
                    break;
                }
            }
            if body.len() > 14 {
                let last = body
                    .last()
                    .cloned()
                    .ok_or_else(|| Error::type_error("Missing definition line"))?;
                body.truncate(12);
                body.push("  // ...".to_owned());
                body.push(last);
            }
            let block = format!("`{file}` `{name}`\n{}", body.join("\n"));
            let length = block.chars().count();
            if used + length > 4000 {
                return Ok(Context {
                    index,
                    related,
                    definitions: decode(&blocks.join("\n\n")),
                    test_names,
                });
            }
            used += length + 2;
            blocks.push(block);
        }
    }
    Ok(Context {
        index,
        related,
        definitions: decode(&blocks.join("\n\n")),
        test_names,
    })
}

fn strip_strings(source: &str) -> String {
    let mut result = String::new();
    let mut quote = None;
    let mut escaped = false;
    for c in source.chars() {
        if quote.is_some() {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if Some(c) == quote {
                quote = None;
            }
        } else if matches!(c, '"' | '\'' | '`') {
            quote = Some(c);
        } else {
            result.push(c);
        }
    }
    result
}

fn design_list<'a>(
    input: Option<&'a Value>,
    label: &str,
    maximum: usize,
) -> Result<&'a [Value], Error> {
    let Some(input) = input else {
        return Ok(&[]);
    };
    domain::items(input)
        .filter(|items| items.len() <= maximum)
        .ok_or_else(|| {
            Error::type_error(format!(
                "design.{label} must be a list of at most {maximum} entries"
            ))
        })
}

fn identifier(input: &Value, label: &str) -> Result<String, Error> {
    let name = encode(&task::one_line(input, label)?);
    let name = name.strip_prefix('`').unwrap_or(&name);
    let name = name.strip_suffix('`').unwrap_or(name);
    let name = name.split('(').next().unwrap_or("");
    if !matches(r"^[A-Za-z_$][\w$]*$", name)? {
        return Err(text_error(
            "TypeError",
            &format!("{label} must be an identifier, got {name}"),
        ));
    }
    Ok(name.to_owned())
}

pub fn normalize_design(input: Option<&Value>) -> Result<Option<Value>, Error> {
    let Some(input) = input.filter(|input| **input != Value::Null) else {
        return Ok(None);
    };
    fields(
        input,
        &[],
        &[
            "extend",
            "new_exports",
            "outline",
            "edge_cases",
            "out_of_scope",
        ],
        "design must be an object with extend, new_exports, outline, edge_cases, out_of_scope",
    )?;
    let file = |input: Option<&Value>, label| -> Result<Value, Error> {
        let input = input.ok_or_else(|| {
            Error::type_error(format!(
                "{label} must be one nonempty line (at most 240 characters)"
            ))
        })?;
        let file = encode(&task::one_line(input, label)?);
        Ok(value(
            file.strip_prefix('`')
                .unwrap_or(&file)
                .strip_suffix('`')
                .unwrap_or(file.strip_prefix('`').unwrap_or(&file)),
        ))
    };
    let mut extend = Vec::new();
    for entry in design_list(input.get("extend"), "extend", 8)? {
        if domain::members(entry).is_none() {
            return Err(Error::type_error(
                "design.extend entries need file and exports",
            ));
        }
        extend.push(Value::object(vec![
            ("file", file(entry.get("file"), "design.extend file")?),
            (
                "exports",
                Value::Array(
                    design_list(entry.get("exports"), "extend exports", 12)?
                        .iter()
                        .map(|item| {
                            identifier(item, "design.extend export").map(|name| value(&name))
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                ),
            ),
        ]));
    }
    let mut new_exports = Vec::new();
    for entry in design_list(input.get("new_exports"), "new_exports", 12)? {
        if domain::members(entry).is_none() {
            return Err(Error::type_error(
                "design.new_exports entries need file and name",
            ));
        }
        new_exports.push(Value::object(vec![
            ("file", file(entry.get("file"), "design.new_exports file")?),
            (
                "name",
                value(&identifier(
                    entry.get("name").unwrap_or(&Value::Null),
                    "design.new_exports name",
                )?),
            ),
        ]));
    }
    let mut result = Value::object(vec![
        ("extend", Value::Array(extend)),
        ("new_exports", Value::Array(new_exports)),
    ]);
    for (field, limit) in [("outline", 10), ("edge_cases", 8), ("out_of_scope", 6)] {
        domain::put(
            &mut result,
            field,
            Value::Array(
                design_list(input.get(field), field, limit)?
                    .iter()
                    .map(|item| task::one_line(item, field).map(Value::String))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        )?;
    }
    Ok(Some(result))
}

fn members<'a>(input: Option<&'a Value>, name: &str) -> &'a [Value] {
    input
        .and_then(|input| input.get(name))
        .and_then(domain::items)
        .unwrap_or(&[])
}

fn name_field(input: &Value, name: &str) -> Result<String, Error> {
    text(required(input, name)?, "Design fields must be text")
}

fn runtime_global(name: &str) -> bool {
    const GLOBALS: &str = "AbortController AbortSignal AggregateError Array ArrayBuffer BTreeMap BTreeSet BigInt BigInt64Array BigUint64Array Boolean Box BroadcastChannel Buffer ByteLengthQueuingStrategy CompressionStream CountQueuingStrategy CryptoKey CustomEvent DataView Date DecompressionStream Error EvalError Event EventTarget FinalizationRegistry Float32Array Float64Array FormData Future HashMap HashSet Headers HTMLElement Int16Array Int32Array Int8Array JSON KeyboardEvent Map Math MessageChannel MessageEvent MessagePort MouseEvent Mutex NaN Number Object Option Path PathBuf PerformanceEntry PerformanceMark PerformanceMeasure PerformanceObserver PerformanceObserverEntryList PerformanceResourceTiming Pin Promise Proxy RangeError ReadableByteStreamController ReadableStream ReadableStreamBYOBReader ReadableStreamBYOBRequest ReadableStreamDefaultController ReadableStreamDefaultReader ReferenceError Reflect RegExp Request Response Result RwLock Set SharedArrayBuffer String SubtleCrypto Symbol SyntaxError TextDecoder TextDecoderStream TextEncoder TextEncoderStream TransformStream TransformStreamDefaultController TypeError URL URLSearchParams Uint16Array Uint32Array Uint8Array Uint8ClampedArray Vec WeakMap WeakRef WeakSet WebAssembly WritableStream WritableStreamDefaultController WritableStreamDefaultWriter __defineGetter__ __defineSetter__ __lookupGetter__ __lookupSetter__ after afterEach assert before beforeEach clearImmediate clearInterval clearTimeout console decodeURI decodeURIComponent describe document encodeURI encodeURIComponent fetch globalThis hasOwnProperty isFinite isNaN isPrototypeOf it localStorage navigator parseFloat parseInt process propertyIsEnumerable queueMicrotask sessionStorage setImmediate setInterval setTimeout structuredClone test toLocaleString toString valueOf window";
    GLOBALS
        .split_ascii_whitespace()
        .any(|global| global == name)
}

fn runtime_member(namespace: &str, name: &str) -> bool {
    let common = match namespace {
        "fs" => {
            "access accessSync appendFile appendFileSync chmod chmodSync chown chownSync close closeSync constants copyFile copyFileSync cp cpSync createReadStream createWriteStream exists existsSync fstat fstatSync fsync fsyncSync lstat lstatSync mkdir mkdirSync mkdtemp mkdtempSync open openSync promises read readFile readFileSync readdir readdirSync readlink readlinkSync realpath realpathSync rename renameSync rm rmSync rmdir rmdirSync stat statSync symlink symlinkSync truncate truncateSync unlink unlinkSync watch watchFile write writeFile writeFileSync"
        }
        "path" => {
            "basename delimiter dirname extname format isAbsolute join matchesGlob normalize parse posix relative resolve sep toNamespacedPath win32"
        }
        "os" => {
            "EOL arch availableParallelism constants cpus devNull endianness freemem getPriority homedir hostname loadavg machine networkInterfaces platform release setPriority tmpdir totalmem type uptime userInfo version"
        }
        "child_process" => "ChildProcess exec execFile execFileSync execSync fork spawn spawnSync",
        "assert" => {
            "deepEqual deepStrictEqual doesNotMatch doesNotReject doesNotThrow equal fail ifError match notDeepEqual notDeepStrictEqual notEqual notStrictEqual ok rejects strict strictEqual throws"
        }
        "Object" => {
            "assign create defineProperties defineProperty entries freeze fromEntries getOwnPropertyDescriptor getOwnPropertyDescriptors getOwnPropertyNames getOwnPropertySymbols getPrototypeOf groupBy hasOwn is isExtensible isFrozen isSealed keys preventExtensions seal setPrototypeOf values"
        }
        "Array" => "from fromAsync isArray of",
        "JSON" => "parse rawJSON isRawJSON stringify",
        "Number" => {
            "EPSILON MAX_SAFE_INTEGER MAX_VALUE MIN_SAFE_INTEGER MIN_VALUE NaN NEGATIVE_INFINITY POSITIVE_INFINITY isFinite isInteger isNaN isSafeInteger parseFloat parseInt"
        }
        "Math" => {
            "abs acos acosh asin asinh atan atan2 atanh cbrt ceil clz32 cos cosh exp expm1 floor fround hypot imul log log10 log1p log2 max min pow random round sign sin sinh sqrt tan tanh trunc"
        }
        "Promise" => "all allSettled any race reject resolve withResolvers",
        "Buffer" => {
            "alloc allocUnsafe allocUnsafeSlow byteLength compare concat copyBytesFrom from isBuffer isEncoding poolSize"
        }
        "process" => {
            "arch argv argv0 chdir config cpuUsage cwd emitWarning env execArgv execPath exit exitCode features getegid geteuid getgid getgroups getuid hrtime kill memoryUsage nextTick pid platform ppid release report resourceUsage send stderr stdin stdout title umask uptime version versions"
        }
        "url" => {
            "URL URLSearchParams domainToASCII domainToUnicode fileURLToPath format parse pathToFileURL resolve urlToHttpOptions"
        }
        "crypto" => {
            "Cipheriv Decipheriv Hash Hmac KeyObject Sign Verify X509Certificate checkPrime checkPrimeSync createCipheriv createDecipheriv createDiffieHellman createDiffieHellmanGroup createECDH createHash createHmac createPrivateKey createPublicKey createSecretKey createSign createVerify diffieHellman generateKey generateKeyPair generateKeyPairSync generateKeySync generatePrime generatePrimeSync getCipherInfo getCiphers getCurves getDiffieHellman getFips getHashes hkdf hkdfSync pbkdf2 pbkdf2Sync privateDecrypt privateEncrypt publicDecrypt publicEncrypt randomBytes randomFill randomFillSync randomInt randomUUID scrypt scryptSync secureHeapUsed setEngine setFips sign subtle timingSafeEqual verify webcrypto"
        }
        "console" => {
            "assert clear count countReset debug dir dirxml error group groupCollapsed groupEnd info log table time timeEnd timeLog timeStamp trace warn"
        }
        "document" => {
            "activeElement addEventListener body cookie createDocumentFragment createElement createTextNode documentElement getElementById getElementsByClassName getElementsByName getElementsByTagName head location querySelector querySelectorAll readyState removeEventListener title"
        }
        "window" => {
            "addEventListener clearInterval clearTimeout close confirm dispatchEvent fetch focus getComputedStyle history innerHeight innerWidth localStorage location matchMedia navigator open parent postMessage match print prompt removeEventListener requestAnimationFrame scroll scrollBy scrollTo sessionStorage setInterval setTimeout top"
        }
        _ => return false,
    };
    common.split(' ').any(|entry| entry == name)
}

pub fn errors(
    checks: &[String],
    design: Option<&Value>,
    files: &[String],
    ask: &str,
    index: Option<&Index>,
) -> Result<Vec<String>, Error> {
    let Some(index) = index else {
        return Ok(Vec::new());
    };
    let mut errors = Vec::new();
    let declared = members(design, "new_exports")
        .iter()
        .map(|item| name_field(item, "name"))
        .collect::<Result<Vec<_>, _>>()?;
    for item in members(design, "extend") {
        let file = name_field(item, "file")?;
        if !index.files.contains(&file) {
            errors.push(format!(
                "design.extend names `{file}`, which is not a tracked file"
            ));
            continue;
        }
        for export in members(Some(item), "exports") {
            let name = text(export, "Design export must be text")?;
            let exported = index.exports(&file);
            if if exported.is_empty() {
                !index.symbols.contains(&name)
            } else {
                !exported.contains(&name)
            } {
                errors.push(format!(
                    "design.extend cites `{name}`, which `{file}` does not export"
                ));
            }
        }
    }
    for item in members(design, "new_exports") {
        let file = name_field(item, "file")?;
        let name = name_field(item, "name")?;
        if !is_allowed(&file, files) {
            errors.push(format!(
                "design.new_exports puts `{name}` in `{file}`, outside files_allowed"
            ));
        }
        if let Some((owner, _)) = index
            .exports_by_file
            .iter()
            .find(|(other, names)| other != &file && names.contains(&name))
        {
            errors.push(format!("design.new_exports `{name}` already exists in `{owner}`; extend that module instead"));
        }
    }
    let mut source = checks.to_owned();
    source.extend(
        members(design, "outline")
            .iter()
            .chain(members(design, "edge_cases"))
            .map(|item| text(item, "Design text must be text"))
            .collect::<Result<Vec<_>, _>>()?,
    );
    let source = source.join("\n");
    let mut names = Vec::new();
    let mut paths = Vec::new();
    let mut runtime_names = Vec::new();
    let mut qualified_names = Vec::new();
    let id = regex(r"^[A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)*$")?;
    let code_like = regex(r"[a-z0-9][A-Z]|^[a-z]+(?:_[a-z0-9]+)+$")?;
    let upper = regex(r"^[A-Z0-9_]+$")?;
    let path = regex(r"^[\w.@-]+(?:/[\w.@-]+)+\.\w+$")?;
    let call = regex(r"^([A-Za-z_$][\w$.]*)\s*\(")?;
    for capture in regex(r"`([^`\n]+)`")?.captures_iter(&source) {
        let span = trim(capture.get(1).map_or("", |entry| entry.as_str()));
        if path.is_match(span) {
            paths.push(span.to_owned());
            continue;
        }
        let name = call
            .captures(span)
            .and_then(|captures| captures.get(1).map(|entry| entry.as_str()))
            .unwrap_or(span);
        if !id.is_match(name) {
            continue;
        }
        let parts = name.split('.').collect::<Vec<_>>();
        if let [namespace, member] = parts.as_slice() {
            if runtime_member(namespace, member) {
                runtime_names.push((*member).to_owned());
                continue;
            }
        }
        for part in &parts {
            if code_like.is_match(part) && !upper.is_match(part) {
                names.push((*part).to_owned());
                if parts.len() > 1 {
                    qualified_names.push((*part).to_owned());
                }
            }
        }
    }
    for name in distinct(names) {
        if index.symbols.contains(&name)
            || declared.contains(&name)
            || ask.contains(&name)
            || runtime_global(&name)
            || runtime_names.contains(&name) && !qualified_names.contains(&name)
        {
            continue;
        }
        errors.push(format!("`{name}` does not exist in the repository; cite a real export or declare it in design.new_exports (in TASK.md: a `## Design` section with `New exports:` then `- \\`{name}\\` in \\`<file>\\``)"));
    }
    for file in distinct(paths) {
        if !index.files.contains(&file) && !is_allowed(&file, files) && !ask.contains(&file) {
            errors.push(format!(
                "`{file}` is neither a tracked file nor in files_allowed"
            ));
        }
    }
    Ok(errors)
}

pub fn parse_design(task: &Value) -> Result<Option<Value>, Error> {
    let parts = task::sections(task)?;
    let Some(section) = parts.section("design") else {
        return Ok(None);
    };
    let mut result = Value::object(vec![
        ("extend", Value::Array(Vec::new())),
        ("new_exports", Value::Array(Vec::new())),
        ("outline", Value::Array(Vec::new())),
        ("edge_cases", Value::Array(Vec::new())),
        ("out_of_scope", Value::Array(Vec::new())),
    ]);
    let mut key = None;
    let marker = regex(r"^(?:[-*+]|\d+[.)])\s+")?;
    let backtick = regex(r"`([^`]+)`")?;
    for line in section
        .content
        .split('\n')
        .map(trim)
        .filter(|line| !line.is_empty())
    {
        let label = match line.strip_suffix(':').map(str::to_lowercase).as_deref() {
            Some("extend") => Some("extend"),
            Some("new exports") => Some("new_exports"),
            Some("outline") => Some("outline"),
            Some("edge cases") => Some("edge_cases"),
            Some("out of scope") => Some("out_of_scope"),
            _ => None,
        };
        if let Some(label) = label {
            key = Some(label);
            continue;
        }
        let item = marker.replace(line, "").into_owned();
        let entry = match key {
            Some("extend") => regex(r"^`([^`]+)`(?::\s*(.*))?$")?
                .captures(&item)
                .map(|captures| {
                    Value::object(vec![
                        (
                            "file",
                            value(captures.get(1).map_or("", |entry| entry.as_str())),
                        ),
                        (
                            "exports",
                            strings(
                                &backtick
                                    .captures_iter(
                                        captures.get(2).map_or("", |entry| entry.as_str()),
                                    )
                                    .filter_map(|captures| {
                                        captures.get(1).map(|entry| entry.as_str().to_owned())
                                    })
                                    .collect::<Vec<_>>(),
                            ),
                        ),
                    ])
                }),
            Some("new_exports") => {
                regex(r"^`([^`]+)`\s+in\s+`([^`]+)`")?
                    .captures(&item)
                    .map(|captures| {
                        Value::object(vec![
                            (
                                "name",
                                value(captures.get(1).map_or("", |entry| entry.as_str())),
                            ),
                            (
                                "file",
                                value(captures.get(2).map_or("", |entry| entry.as_str())),
                            ),
                        ])
                    })
            }
            Some(_) => Some(value(&item)),
            None => None,
        };
        if let (Some(key), Some(entry)) = (key, entry) {
            let mut items = result
                .get(key)
                .and_then(domain::items)
                .unwrap_or(&[])
                .to_vec();
            items.push(entry);
            domain::put(&mut result, key, Value::Array(items))?;
        }
    }
    Ok(Some(result))
}

pub fn render_design(design: &Value) -> Result<Text, Error> {
    let mut rows = vec!["## Design".to_owned(), String::new()];
    if design.get("derived") == Some(&Value::Boolean(true)) {
        rows.extend([
            "Source: derived by the harness from the repository; the planner gave none.".to_owned(),
            String::new(),
        ]);
    }
    if let Some(rejected) = design.get("rejected").and_then(Value::as_string) {
        rows.extend([
            format!("The planner's design was rejected: {}.", encode(rejected)),
            String::new(),
        ]);
    }
    if !members(Some(design), "extend").is_empty() {
        rows.push("Extend:".to_owned());
        for item in members(Some(design), "extend") {
            let exports = members(Some(item), "exports")
                .iter()
                .map(|item| {
                    text(item, "Design export must be text").map(|name| format!("`{name}`"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            rows.push(
                format!(
                    "`{}`{}",
                    name_field(item, "file")?,
                    if exports.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", exports.join(", "))
                    }
                )
                .replacen('`', "- `", 1),
            );
        }
    }
    if !members(Some(design), "new_exports").is_empty() {
        rows.push("New exports:".to_owned());
        for item in members(Some(design), "new_exports") {
            rows.push(format!(
                "- `{}` in `{}`",
                name_field(item, "name")?,
                name_field(item, "file")?
            ));
        }
    }
    for (name, label) in [
        ("outline", "Outline:"),
        ("edge_cases", "Edge cases:"),
        ("out_of_scope", "Out of scope:"),
    ] {
        let items = members(Some(design), name);
        if !items.is_empty() {
            rows.push(label.to_owned());
            for (index, item) in items.iter().enumerate() {
                rows.push(format!(
                    "{} {}",
                    if name == "outline" {
                        format!("{}.", index + 1)
                    } else {
                        "-".to_owned()
                    },
                    text(item, "Design item must be text")?
                ));
            }
        }
    }
    Ok(decode(&format!("{}\n", rows.join("\n"))))
}

pub fn with_design(task: &Value, design: &Value) -> Result<Text, Error> {
    if task::sections(task)?.section("design").is_some() {
        return task
            .as_string()
            .cloned()
            .ok_or_else(|| Error::type_error("TASK.md must be text"));
    }
    let source = text(task, "TASK.md must be text")?;
    let block = encode(&render_design(design)?);
    let heading = regex(r"(?im)^## (?:Files allowed|Allowed Files)\s*$")?;
    let result = if let Some(found) = heading.find(&source) {
        format!(
            "{}{block}\n{}",
            &source[..found.start()],
            &source[found.start()..]
        )
    } else {
        format!("{}\n\n{block}", trim_end(&source))
    };
    Ok(decode(&result))
}

pub fn derive_design(
    files: &[String],
    index: &Index,
    related: &[(String, Vec<String>)],
) -> Result<Value, Error> {
    let mut extend = Vec::new();
    for file in files {
        if index.files.contains(file) && code_file(file)? {
            extend.push((
                file.clone(),
                index
                    .exports(file)
                    .iter()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>(),
            ));
        }
    }
    for (file, names) in related {
        if extend.len() >= 6 {
            break;
        }
        if !extend.iter().any(|(name, _)| name == file) {
            extend.push((file.clone(), names.iter().take(6).cloned().collect()));
        }
    }
    Ok(Value::object(vec![
        ("extend", Value::Array(extend.iter().map(|(file, names)| Value::object(vec![("file", value(file)), ("exports", strings(names))])).collect())),
        ("new_exports", Value::Array(Vec::new())),
        ("outline", strings(&["Read every Extend module first; reuse its exports instead of adding a parallel module.".to_owned(), "Write or extend a test for each acceptance check, then change only Files allowed.".to_owned()])),
        ("edge_cases", Value::Array(Vec::new())), ("out_of_scope", strings(&["Files outside Files allowed; new runtime dependencies".to_owned()])),
        ("derived", Value::Boolean(true)),
    ]))
}

pub fn ensure_design(task: &Value, context: &Context, ask: &str) -> Result<Text, Error> {
    let files = task::files_allowed(task)?;
    if !is_code_slice(&files)? {
        return task
            .as_string()
            .cloned()
            .ok_or_else(|| Error::type_error("TASK.md must be text"));
    }
    let design = parse_design(task)?;
    let errors = errors(&[], design.as_ref(), &files, ask, Some(&context.index))?;
    if design.is_some() && errors.is_empty() {
        return task
            .as_string()
            .cloned()
            .ok_or_else(|| Error::type_error("TASK.md must be text"));
    }
    let mut source = text(task, "TASK.md must be text")?;
    if let Some(section) = task::sections(task)?.section("design") {
        source = source.replacen(&section.source, "", 1);
    }
    let mut derived = derive_design(&files, &context.index, &context.related)?;
    if let Some(error) = errors.first() {
        domain::put(&mut derived, "rejected", value(error))?;
    }
    with_design(&value(&source), &derived)
}
