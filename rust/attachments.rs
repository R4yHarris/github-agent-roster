use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;

use crate::json::Value;
use crate::state::secure;
use crate::{Error, config};

pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "tif"];
pub const MAX_IMAGE_ATTACHMENT_BYTES: u64 = 10 * 1024 * 1024;
pub const MAX_TEXT_ATTACHMENT_BYTES: u64 = 512 * 1024;
pub const PASTE_COLLAPSE_LINE_THRESHOLD: usize = 5;
pub const PASTE_COLLAPSE_CHAR_THRESHOLD: usize = 1500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    Image,
    File,
    Directory,
}

impl AttachmentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::File => "file",
            Self::Directory => "directory",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineMention {
    pub raw_token: String,
    pub path: PathBuf,
    pub line_range: Option<(usize, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionAttachment {
    pub id: usize,
    pub name: String,
    pub path: PathBuf,
    pub kind: AttachmentKind,
    pub size_bytes: u64,
    pub mime_type: String,
    pub data_uri: Option<String>,
    pub text_excerpt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDropMatch {
    pub path: PathBuf,
    pub is_image: bool,
    pub remainder: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardCapture {
    Image(PathBuf),
    File(PathBuf),
    Text {
        path: PathBuf,
        content: String,
        lines: usize,
    },
    Empty,
}

pub fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            IMAGE_EXTENSIONS
                .iter()
                .any(|candidate| ext.eq_ignore_ascii_case(candidate))
        })
}

pub fn image_mime_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        Some("tiff" | "tif") => "image/tiff",
        _ => "application/octet-stream",
    }
}

pub fn resolve_attachment_candidate(raw: &str, cwd: &Path, env: &Value) -> Option<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 2048 {
        return None;
    }
    let unquoted = if (trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2)
        || (trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2)
    {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };
    let without_scheme = if let Some(rest) = unquoted.strip_prefix("file:///") {
        if cfg!(windows) && rest.as_bytes().get(1) == Some(&b':') {
            rest.to_owned()
        } else {
            format!("/{rest}")
        }
    } else if let Some(rest) = unquoted.strip_prefix("file://") {
        rest.to_owned()
    } else {
        unquoted.to_owned()
    };

    let home = config::environment_value(env, if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .ok()
        .flatten()
        .and_then(|t| t.to_utf8().ok())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
        });

    let mut candidates = Vec::new();
    if let Some(sub) = without_scheme
        .strip_prefix("~/")
        .or_else(|| without_scheme.strip_prefix(r"~\"))
    {
        if let Some(ref h) = home {
            candidates.push(h.join(sub));
        }
        if cfg!(windows) {
            let wsl_home = Path::new(r"\\wsl.localhost\Ubuntu\home");
            if let Ok(rd) = fs::read_dir(wsl_home) {
                for entry in rd.flatten() {
                    candidates.push(entry.path().join(sub.replace('/', "\\")));
                }
            }
        }
    } else {
        let p = PathBuf::from(&without_scheme);
        if p.is_absolute() {
            candidates.push(p);
        } else {
            candidates.push(cwd.join(&p));
        }
        if cfg!(windows) && without_scheme.starts_with("/home/") {
            let wsl_p = PathBuf::from(format!(
                r"\\wsl.localhost\Ubuntu\{}",
                without_scheme.trim_start_matches('/').replace('/', "\\")
            ));
            candidates.push(wsl_p);
        }
    }

    for cand in candidates {
        if let Ok(meta) = fs::symlink_metadata(&cand) {
            if !secure::linked(&meta) && (meta.is_file() || meta.is_dir()) {
                let fname = cand
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if meta.is_dir() && matches!(fname.as_str(), ".git" | "node_modules" | "target") {
                    continue;
                }
                return Some(fs::canonicalize(&cand).unwrap_or(cand));
            }
        }
    }
    None
}

fn starts_like_file_drop(input: &str) -> bool {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return false;
    }
    let quoted = trimmed.starts_with('"') || trimmed.starts_with('\'');
    let unquoted = if quoted { &trimmed[1..] } else { trimmed };
    if unquoted.starts_with("./")
        || unquoted.starts_with("../")
        || unquoted.starts_with(r".\")
        || unquoted.starts_with(r"..\")
        || unquoted.starts_with("~/")
        || unquoted.starts_with(r"~\")
        || unquoted.starts_with(r"\\wsl.localhost\")
        || (!quoted && unquoted.starts_with("file://"))
    {
        return true;
    }
    let bytes = unquoted.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
    {
        return true;
    }
    if unquoted.starts_with('/') {
        let first_token = unquoted.split_whitespace().next().unwrap_or("");
        if first_token.len() > 1 && first_token[1..].contains('/') {
            return true;
        }
        if Path::new(first_token).extension().is_some() {
            return true;
        }
    }
    false
}

pub fn detect_file_drop(user_input: &str, cwd: &Path, env: &Value) -> Option<FileDropMatch> {
    let stripped = user_input.trim();
    if !starts_like_file_drop(stripped) {
        return None;
    }
    if let Some(direct) = resolve_attachment_candidate(stripped, cwd, env) {
        let is_image = is_image_path(&direct);
        return Some(FileDropMatch {
            path: direct,
            is_image,
            remainder: String::new(),
        });
    }
    let first_char = stripped.chars().next()?;
    if matches!(first_char, '"' | '\'') {
        let rest = &stripped[first_char.len_utf8()..];
        if let Some(close_idx) = rest.find(first_char) {
            let quoted_path = &rest[..close_idx];
            let remainder = rest[close_idx + first_char.len_utf8()..].trim().to_owned();
            if let Some(resolved) = resolve_attachment_candidate(quoted_path, cwd, env) {
                let is_image = is_image_path(&resolved);
                return Some(FileDropMatch {
                    path: resolved,
                    is_image,
                    remainder,
                });
            }
        }
        return None;
    }
    if let Some((first_tok, rem)) = stripped.split_once(char::is_whitespace) {
        if let Some(resolved) = resolve_attachment_candidate(first_tok, cwd, env) {
            let is_image = is_image_path(&resolved);
            return Some(FileDropMatch {
                path: resolved,
                is_image,
                remainder: rem.trim().to_owned(),
            });
        }
    }
    let space_positions: Vec<usize> = stripped
        .char_indices()
        .filter_map(|(idx, ch)| (ch == ' ').then_some(idx))
        .collect();
    for pos in space_positions.into_iter().rev() {
        let prefix = stripped[..pos].trim_end();
        if let Some(resolved) = resolve_attachment_candidate(prefix, cwd, env) {
            let is_image = is_image_path(&resolved);
            let remainder = stripped[pos + 1..].trim().to_owned();
            return Some(FileDropMatch {
                path: resolved,
                is_image,
                remainder,
            });
        }
    }
    None
}

pub fn build_session_attachment(id: usize, path: &Path) -> Result<SessionAttachment, Error> {
    build_session_attachment_with_range(id, path, None)
}

pub fn build_session_attachment_with_range(
    id: usize,
    path: &Path,
    line_range: Option<(usize, usize)>,
) -> Result<SessionAttachment, Error> {
    let meta = fs::symlink_metadata(path).map_err(|e| {
        Error::new(
            "Error",
            format!("Cannot inspect attachment {}: {e}", path.display()),
        )
    })?;
    if (!meta.is_file() && !meta.is_dir()) || secure::linked(&meta) {
        return Err(Error::type_error(
            "Attachment must be a regular unlinked file or directory",
        ));
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("attachment")
        .to_owned();
    let fname_lower = name.to_lowercase();
    if fname_lower == ".env"
        || (fname_lower.starts_with(".env.") && fname_lower != ".env.example")
        || fname_lower.ends_with(".pem")
        || fname_lower.ends_with(".key")
        || fname_lower.ends_with(".pfx")
    {
        return Err(Error::type_error(
            "Refusing to attach secret or private-key file",
        ));
    }

    if meta.is_dir() {
        let mut entries_list = Vec::new();
        let mut total_bytes = 0_u64;
        let mut file_count = 0_usize;
        let mut dir_count = 0_usize;
        if let Ok(rd) = fs::read_dir(path) {
            let mut top_entries: Vec<_> = rd.flatten().take(120).collect();
            top_entries.sort_by_key(|e| e.file_name());
            for entry in top_entries {
                let ename = entry.file_name().to_string_lossy().to_string();
                if matches!(
                    ename.as_str(),
                    ".git" | "node_modules" | "target" | ".worktrees"
                ) {
                    continue;
                }
                let Ok(emeta) = fs::symlink_metadata(entry.path()) else {
                    continue;
                };
                if secure::linked(&emeta) {
                    continue;
                }
                if emeta.is_dir() {
                    dir_count += 1;
                    entries_list.push(format!("[dir]  {ename}/"));
                    if let Ok(sub_rd) = fs::read_dir(entry.path()) {
                        let mut sub_entries: Vec<_> = sub_rd.flatten().take(20).collect();
                        sub_entries.sort_by_key(|e| e.file_name());
                        for sub in sub_entries {
                            let sname = sub.file_name().to_string_lossy().to_string();
                            if let Ok(smeta) = fs::symlink_metadata(sub.path()) {
                                if !secure::linked(&smeta) {
                                    if smeta.is_dir() {
                                        entries_list.push(format!("  [dir]  {ename}/{sname}/"));
                                    } else if smeta.is_file() {
                                        file_count += 1;
                                        total_bytes = total_bytes.saturating_add(smeta.len());
                                        entries_list.push(format!(
                                            "  [file] {ename}/{sname} ({} B)",
                                            smeta.len()
                                        ));
                                    }
                                }
                            }
                        }
                    }
                } else if emeta.is_file() {
                    file_count += 1;
                    total_bytes = total_bytes.saturating_add(emeta.len());
                    entries_list.push(format!("[file] {ename} ({} B)", emeta.len()));
                }
            }
        }
        let mut summary = format!(
            "[Directory Attachment #{id}: `{name}/` ({file_count} files, {dir_count} subdirs, {total_bytes} B) at `{}`]\n{}",
            path.display(),
            entries_list.join("\n")
        );
        for key_doc in ["README.md", "Cargo.toml", "package.json", "AGENTS.md"] {
            let doc_p = path.join(key_doc);
            if let Ok(dmeta) = fs::symlink_metadata(&doc_p) {
                if dmeta.is_file() && !secure::linked(&dmeta) && dmeta.len() <= 65_536 {
                    if let Ok(raw) = fs::read_to_string(&doc_p) {
                        let snip: String = raw
                            .lines()
                            .take(20)
                            .collect::<Vec<_>>()
                            .join("\n")
                            .chars()
                            .take(1200)
                            .collect();
                        summary.push_str(&format!("\n\n--- {key_doc} ---\n{snip}"));
                    }
                }
            }
        }
        return Ok(SessionAttachment {
            id,
            name: format!("{name}/"),
            path: path.to_path_buf(),
            kind: AttachmentKind::Directory,
            size_bytes: total_bytes,
            mime_type: "inode/directory".to_owned(),
            data_uri: None,
            text_excerpt: Some(summary),
        });
    }

    let size_bytes = meta.len();
    if is_image_path(path) {
        if size_bytes > MAX_IMAGE_ATTACHMENT_BYTES {
            return Err(Error::type_error("Image attachment exceeds 10 MiB limit"));
        }
        let bytes = fs::read(path).map_err(|e| {
            Error::new(
                "Error",
                format!("Cannot read image {}: {e}", path.display()),
            )
        })?;
        let mime = image_mime_type(path).to_owned();
        let b64 = BASE64_STANDARD.encode(&bytes);
        let data_uri = format!("data:{mime};base64,{b64}");
        let desc = format!(
            "[Attachment #{id}: image `{name}` ({size_bytes} bytes, {mime}) at `{}`]",
            path.display()
        );
        Ok(SessionAttachment {
            id,
            name,
            path: path.to_path_buf(),
            kind: AttachmentKind::Image,
            size_bytes,
            mime_type: mime,
            data_uri: Some(data_uri),
            text_excerpt: Some(desc),
        })
    } else {
        if size_bytes > MAX_TEXT_ATTACHMENT_BYTES {
            return Err(Error::type_error(
                "Text/file attachment exceeds 512 KiB limit",
            ));
        }
        let bytes = fs::read(path).map_err(|e| {
            Error::new("Error", format!("Cannot read file {}: {e}", path.display()))
        })?;
        let text = String::from_utf8_lossy(&bytes);
        let excerpt: String = if let Some((start, end)) = line_range {
            let s = start.max(1);
            let e = end.max(s);
            text.lines()
                .enumerate()
                .skip(s - 1)
                .take(e - s + 1)
                .map(|(i, l)| format!("{}: {l}", i + 1))
                .collect::<Vec<_>>()
                .join("\n")
                .chars()
                .take(16_000)
                .collect()
        } else {
            text.chars().take(16_000).collect()
        };
        let display_name = match line_range {
            Some((s, e)) => format!("{name}:{s}-{e}"),
            None => name,
        };
        Ok(SessionAttachment {
            id,
            name: display_name,
            path: path.to_path_buf(),
            kind: AttachmentKind::File,
            size_bytes,
            mime_type: "text/plain".to_owned(),
            data_uri: None,
            text_excerpt: Some(excerpt),
        })
    }
}

pub fn format_attachment_badge(att: &SessionAttachment) -> String {
    let kib = (att.size_bytes as f64) / 1024.0;
    format!(
        "[Attachment #{} ({}) {} ({:.1} KiB) -> {}]",
        att.id,
        att.kind.as_str(),
        att.name,
        kib,
        att.path.display()
    )
}

pub fn format_attachment_list(attachments: &[SessionAttachment]) -> String {
    if attachments.is_empty() {
        return "No active session attachments.\n".to_owned();
    }
    let mut out = String::new();
    for att in attachments {
        out.push_str(&format!(
            "#{} | {} | {} | {} B | {}\n",
            att.id,
            att.kind.as_str(),
            att.name,
            att.size_bytes,
            att.path.display()
        ));
    }
    out
}

pub fn summarize_attachment_inventory(attachments: &[SessionAttachment]) -> String {
    if attachments.is_empty() {
        return "0 attachments (0 B)".to_owned();
    }
    let total = attachments.len();
    let images = attachments
        .iter()
        .filter(|att| att.kind == AttachmentKind::Image)
        .count();
    let files = total - images;
    let bytes: u64 = attachments.iter().map(|att| att.size_bytes).sum();
    format!("{total} attachments ({images} images, {files} files, {bytes} B)")
}

pub fn match_referenced_attachments<'a>(
    user_text: &str,
    attachments: &'a [SessionAttachment],
) -> Vec<&'a SessionAttachment> {
    if attachments.is_empty() {
        return Vec::new();
    }
    let lower = user_text.to_lowercase();
    let mut matched = Vec::new();
    for att in attachments {
        let id_tag = format!("#{}", att.id);
        let att_tag = format!("attachment {}", att.id);
        let name_lower = att.name.to_lowercase();
        let path_lower = att.path.display().to_string().to_lowercase();
        if lower.contains(&id_tag)
            || lower.contains(&att_tag)
            || lower.contains(&name_lower)
            || lower.contains(&path_lower)
        {
            matched.push(att);
        }
    }
    if matched.is_empty() {
        let generic_ref = regex_lite::Regex::new(
            r"(?i)\b(?:pasted|attached|attachment|screenshot|image|diagram|photo|picture|earlier\s+file|that\s+file|the\s+file)\b",
        )
        .is_ok_and(|re| re.is_match(&lower));
        if generic_ref {
            if let Some(last) = attachments.last() {
                matched.push(last);
            }
        }
    }
    matched
}

pub fn strip_bracketed_paste_wrappers(text: &str) -> String {
    text.replace("\x1b[200~", "")
        .replace("\x1b[201~", "")
        .replace("[200~", "")
        .replace("[201~", "")
}

pub fn collapse_large_paste(
    text: &str,
    paste_id: usize,
    pastes_dir: &Path,
) -> Option<(String, PathBuf)> {
    let normalized = strip_bracketed_paste_wrappers(text)
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let line_count = normalized.lines().count().max(1);
    if line_count < PASTE_COLLAPSE_LINE_THRESHOLD
        && normalized.len() < PASTE_COLLAPSE_CHAR_THRESHOLD
    {
        return None;
    }
    let _ = fs::create_dir_all(pastes_dir);
    let file_path = pastes_dir.join(format!("paste_{paste_id}.txt"));
    fs::write(&file_path, normalized.as_bytes()).ok()?;
    let token = format!(
        "[Pasted text #{paste_id}: {line_count} lines -> {}]",
        file_path.display()
    );
    Some((token, file_path))
}

pub fn expand_paste_references(text: &str) -> String {
    let Ok(re) =
        regex_lite::Regex::new(r"\[Pasted text #\d+:\s*\d+\s*lines\s*(?:->|→)\s*([^\]]+)\]")
    else {
        return text.to_owned();
    };
    let mut result = String::with_capacity(text.len());
    let mut last_end = 0_usize;
    for caps in re.captures_iter(text) {
        let Some(full) = caps.get(0) else {
            continue;
        };
        result.push_str(&text[last_end..full.start()]);
        let raw_path = caps.get(1).map_or("", |m| m.as_str()).trim();
        let p = Path::new(raw_path);
        if let Ok(meta) = fs::symlink_metadata(p) {
            if meta.is_file() && !secure::linked(&meta) && meta.len() <= MAX_TEXT_ATTACHMENT_BYTES {
                if let Ok(content) = fs::read_to_string(p) {
                    result.push_str(&content);
                    last_end = full.end();
                    continue;
                }
            }
        }
        result.push_str(full.as_str());
        last_end = full.end();
    }
    result.push_str(&text[last_end..]);
    result
}

fn is_wsl_env() -> bool {
    if cfg!(windows) {
        return false;
    }
    fs::read_to_string("/proc/sys/kernel/osrelease").is_ok_and(|s| {
        let l = s.to_lowercase();
        l.contains("microsoft") || l.contains("wsl")
    })
}

pub fn capture_clipboard(
    attachments_dir: &Path,
    pastes_dir: &Path,
    next_id: usize,
) -> Result<ClipboardCapture, Error> {
    let _ = fs::create_dir_all(attachments_dir);
    let _ = fs::create_dir_all(pastes_dir);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let img_dest = attachments_dir.join(format!("clip_{ts}_{next_id}.png"));

    if cfg!(windows) || is_wsl_env() {
        let ps_exe = if cfg!(windows) {
            "powershell"
        } else {
            "powershell.exe"
        };
        let script = r#"
Add-Type -AssemblyName System.Windows.Forms
if ([System.Windows.Forms.Clipboard]::ContainsImage()) {
    $img = [System.Windows.Forms.Clipboard]::GetImage()
    if ($null -ne $img) {
        $ms = New-Object System.IO.MemoryStream
        $img.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
        Write-Output ("IMAGE_B64:" + [System.Convert]::ToBase64String($ms.ToArray()))
        exit 0
    }
}
if ([System.Windows.Forms.Clipboard]::ContainsFileDropList()) {
    $files = [System.Windows.Forms.Clipboard]::GetFileDropList()
    if ($files.Count -gt 0) {
        Write-Output ("FILE_DROP:" + $files[0])
        exit 0
    }
}
$txt = Get-Clipboard -Raw -ErrorAction SilentlyContinue
if ($null -ne $txt -and $txt.Length -gt 0) {
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($txt)
    Write-Output ("TEXT_B64:" + [System.Convert]::ToBase64String($bytes))
    exit 0
}
Write-Output "EMPTY"
"#;
        if let Ok(out) = StdCommand::new(ps_exe)
            .args(["-NoProfile", "-NonInteractive", "-Sta", "-Command", script])
            .stdin(Stdio::null())
            .output()
        {
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let line = stdout.trim();
                if let Some(b64) = line.strip_prefix("IMAGE_B64:") {
                    if let Ok(bytes) = BASE64_STANDARD.decode(b64.trim()) {
                        if !bytes.is_empty() && fs::write(&img_dest, &bytes).is_ok() {
                            return Ok(ClipboardCapture::Image(img_dest));
                        }
                    }
                } else if let Some(fpath) = line.strip_prefix("FILE_DROP:") {
                    let p = PathBuf::from(fpath.trim());
                    if p.is_file() {
                        if is_image_path(&p) {
                            return Ok(ClipboardCapture::Image(p));
                        }
                        return Ok(ClipboardCapture::File(p));
                    }
                } else if let Some(b64) = line.strip_prefix("TEXT_B64:") {
                    if let Ok(bytes) = BASE64_STANDARD.decode(b64.trim()) {
                        if let Ok(text) = String::from_utf8(bytes) {
                            let clean = text.replace("\r\n", "\n");
                            let lines = clean.lines().count().max(1);
                            let paste_path = pastes_dir.join(format!("paste_{ts}_{next_id}.txt"));
                            let _ = fs::write(&paste_path, clean.as_bytes());
                            return Ok(ClipboardCapture::Text {
                                path: paste_path,
                                content: clean,
                                lines,
                            });
                        }
                    }
                }
            }
        }
    }

    if cfg!(target_os = "macos") {
        if StdCommand::new("pngpaste")
            .arg(&img_dest)
            .stdin(Stdio::null())
            .output()
            .is_ok_and(|o| o.status.success() && img_dest.is_file())
        {
            return Ok(ClipboardCapture::Image(img_dest));
        }
        if let Ok(out) = StdCommand::new("pbpaste").stdin(Stdio::null()).output() {
            if out.status.success() && !out.stdout.is_empty() {
                let text = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
                let lines = text.lines().count().max(1);
                let paste_path = pastes_dir.join(format!("paste_{ts}_{next_id}.txt"));
                let _ = fs::write(&paste_path, text.as_bytes());
                return Ok(ClipboardCapture::Text {
                    path: paste_path,
                    content: text,
                    lines,
                });
            }
        }
    }

    if cfg!(unix) {
        if let Ok(out) = StdCommand::new("wl-paste")
            .args(["--type", "image/png"])
            .stdin(Stdio::null())
            .output()
        {
            if out.status.success()
                && !out.stdout.is_empty()
                && fs::write(&img_dest, &out.stdout).is_ok()
            {
                return Ok(ClipboardCapture::Image(img_dest));
            }
        }
        if let Ok(out) = StdCommand::new("xclip")
            .args(["-selection", "clipboard", "-t", "image/png", "-o"])
            .stdin(Stdio::null())
            .output()
        {
            if out.status.success()
                && !out.stdout.is_empty()
                && fs::write(&img_dest, &out.stdout).is_ok()
            {
                return Ok(ClipboardCapture::Image(img_dest));
            }
        }
        for cmd in [
            vec!["wl-paste", "--no-newline"],
            vec!["xclip", "-selection", "clipboard", "-o"],
        ] {
            if let Ok(out) = StdCommand::new(cmd[0])
                .args(&cmd[1..])
                .stdin(Stdio::null())
                .output()
            {
                if out.status.success() && !out.stdout.is_empty() {
                    let text = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
                    let lines = text.lines().count().max(1);
                    let paste_path = pastes_dir.join(format!("paste_{ts}_{next_id}.txt"));
                    let _ = fs::write(&paste_path, text.as_bytes());
                    return Ok(ClipboardCapture::Text {
                        path: paste_path,
                        content: text,
                        lines,
                    });
                }
            }
        }
    }

    Ok(ClipboardCapture::Empty)
}

pub fn format_osc52_sequence(text: &str, tmux: bool) -> String {
    let b64 = BASE64_STANDARD.encode(text.as_bytes());
    let base_seq = format!("\x1b]52;c;{b64}\x07");
    if tmux {
        format!("\x1bPtmux;{}\x1b\\", base_seq.replace('\x1b', "\x1b\x1b"))
    } else {
        base_seq
    }
}

pub fn write_clipboard_text(text: &str) -> bool {
    let b64 = BASE64_STANDARD.encode(text.as_bytes());
    if cfg!(windows) || is_wsl_env() {
        let ps_exe = if cfg!(windows) {
            "powershell"
        } else {
            "powershell.exe"
        };
        let cmd = format!(
            "Set-Clipboard -Value ([System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String('{b64}')))"
        );
        if StdCommand::new(ps_exe)
            .args(["-NoProfile", "-NonInteractive", "-Command", &cmd])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
        {
            return true;
        }
    }
    if cfg!(target_os = "macos") {
        if let Ok(mut child) = StdCommand::new("pbcopy")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = std::io::Write::write_all(&mut stdin, text.as_bytes());
            }
            if child.wait().is_ok_and(|s| s.success()) {
                return true;
            }
        }
    }
    if cfg!(unix) {
        for prog in [
            vec!["wl-copy", "--type", "text/plain"],
            vec!["xclip", "-selection", "clipboard", "-in"],
            vec!["xsel", "--clipboard", "--input"],
        ] {
            if let Ok(mut child) = StdCommand::new(prog[0])
                .args(&prog[1..])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = std::io::Write::write_all(&mut stdin, text.as_bytes());
                }
                if child.wait().is_ok_and(|s| s.success()) {
                    return true;
                }
            }
        }
    }
    false
}

pub fn extract_last_fenced_code_block(text: &str) -> Option<String> {
    let mut blocks = Vec::new();
    let mut in_fence = false;
    let mut current = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            if in_fence {
                blocks.push(current.join("\n"));
                current.clear();
                in_fence = false;
            } else {
                in_fence = true;
                current.clear();
            }
        } else if in_fence {
            current.push(line);
        }
    }
    blocks.into_iter().rev().find(|b| !b.trim().is_empty())
}

pub fn extract_inline_at_mentions(text: &str, cwd: &Path, env: &Value) -> Vec<InlineMention> {
    let Ok(re) = regex_lite::Regex::new(r"(?:^|\s)@([A-Za-z0-9_./\\~:-]+)") else {
        return Vec::new();
    };
    let mut mentions = Vec::new();
    for caps in re.captures_iter(text) {
        let Some(m) = caps.get(1) else {
            continue;
        };
        let raw_tok = m
            .as_str()
            .trim_end_matches(['.', ',', ';', ')', ']', '?', '!']);
        if raw_tok.is_empty() {
            continue;
        }
        let (path_part, line_range) = if let Some((base_p, range_str)) = raw_tok.rsplit_once(':') {
            if !(base_p.is_empty()
                || base_p.len() == 1 && base_p.as_bytes()[0].is_ascii_alphabetic())
            {
                if let Some((start_s, end_s)) = range_str.split_once('-') {
                    if let (Ok(s), Ok(e)) = (start_s.parse::<usize>(), end_s.parse::<usize>()) {
                        (base_p, Some((s.max(1), e.max(s.max(1)))))
                    } else {
                        (raw_tok, None)
                    }
                } else if let Ok(single) = range_str.parse::<usize>() {
                    (base_p, Some((single.max(1), single.max(1) + 40)))
                } else {
                    (raw_tok, None)
                }
            } else {
                (raw_tok, None)
            }
        } else {
            (raw_tok, None)
        };
        if let Some(resolved) = resolve_attachment_candidate(path_part, cwd, env) {
            if !mentions.iter().any(|existing: &InlineMention| {
                existing.path == resolved && existing.line_range == line_range
            }) {
                mentions.push(InlineMention {
                    raw_token: format!("@{raw_tok}"),
                    path: resolved,
                    line_range,
                });
            }
        }
    }
    mentions
}

pub fn complete_workspace_path(partial: &str, cwd: &Path) -> Vec<String> {
    let norm = partial.replace('\\', "/");
    let (dir_rel, file_prefix) = match norm.rsplit_once('/') {
        Some((d, f)) => (format!("{d}/"), f),
        None => (String::new(), norm.as_str()),
    };
    let search_dir = if dir_rel.is_empty() {
        cwd.to_path_buf()
    } else {
        cwd.join(&dir_rel)
    };
    let Ok(rd) = fs::read_dir(&search_dir) else {
        return Vec::new();
    };
    let prefix_lower = file_prefix.to_lowercase();
    let mut matches = Vec::new();
    for entry in rd.flatten().take(200) {
        let name = entry.file_name().to_string_lossy().to_string();
        if matches!(
            name.as_str(),
            ".git" | "node_modules" | "target" | ".worktrees"
        ) || (name.starts_with('.') && !prefix_lower.starts_with('.'))
        {
            continue;
        }
        if name.to_lowercase().starts_with(&prefix_lower) {
            let is_dir = entry.metadata().is_ok_and(|m| m.is_dir());
            let suffix = if is_dir { "/" } else { "" };
            matches.push(format!("{dir_rel}{name}{suffix}"));
        }
    }
    matches.sort();
    matches
}

pub fn strip_leaked_terminal_sequences(text: &str) -> String {
    let mut cleaned = strip_bracketed_paste_wrappers(text);
    if let Ok(cpr_re) = regex_lite::Regex::new(r"(?:\x1b\[|\^\[)\d+;\d+R") {
        cleaned = cpr_re.replace_all(&cleaned, "").into_owned();
    }
    if let Ok(mouse_re) = regex_lite::Regex::new(r"(?:\x1b\[|\^\[)?<\d+;\d+;\d+[Mm]") {
        cleaned = mouse_re.replace_all(&cleaned, "").into_owned();
    }
    cleaned
}

pub fn is_whisper_hallucination(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }
    let lower = trimmed
        .trim_matches(['.', '!', '?', ',', ' ', '"', '\''])
        .to_lowercase();
    matches!(
        lower.as_str(),
        "thank you"
            | "thanks for watching"
            | "thank you for watching"
            | "you"
            | "bye"
            | "[blank_audio]"
            | "(blank audio)"
            | "subtitles by the amara.org community"
    )
}

pub fn transcribe_audio_file(audio_path: &Path) -> Result<String, Error> {
    let meta = fs::symlink_metadata(audio_path).map_err(|e| {
        Error::new(
            "Error",
            format!("Audio file not found ({}): {e}", audio_path.display()),
        )
    })?;
    if !meta.is_file() || secure::linked(&meta) {
        return Err(Error::type_error("Audio input must be a regular file"));
    }

    if cfg!(windows) || is_wsl_env() {
        let ps_exe = if cfg!(windows) {
            "powershell"
        } else {
            "powershell.exe"
        };
        let win_path = audio_path.display().to_string().replace('\'', "''");
        let script = format!(
            r#"
try {{
    Add-Type -AssemblyName System.Speech
    $rec = New-Object System.Speech.Recognition.SpeechRecognitionEngine
    $rec.LoadGrammar((New-Object System.Speech.Recognition.DictationGrammar))
    $rec.SetInputToWaveFile('{win_path}')
    $sb = New-Object System.Text.StringBuilder
    while ($true) {{
        $res = $rec.Recognize()
        if ($null -eq $res) {{ break }}
        if ($res.Text) {{ [void]$sb.Append($res.Text + ' ') }}
    }}
    Write-Output ($sb.ToString().Trim())
}} catch {{
    Write-Output ''
}}
"#
        );
        if let Ok(out) = StdCommand::new(ps_exe)
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .stdin(Stdio::null())
            .output()
        {
            if out.status.success() {
                let transcript = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                if !transcript.is_empty() && !is_whisper_hallucination(&transcript) {
                    return Ok(transcript);
                }
            }
        }
    }

    for whisper_bin in ["whisper", "faster-whisper"] {
        if let Ok(out) = StdCommand::new(whisper_bin)
            .args([
                audio_path.to_str().unwrap_or(""),
                "--model",
                "base",
                "--output_format",
                "txt",
            ])
            .stdin(Stdio::null())
            .output()
        {
            if out.status.success() {
                let transcript = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                if !transcript.is_empty() && !is_whisper_hallucination(&transcript) {
                    return Ok(transcript);
                }
            }
        }
    }

    Ok(format!(
        "[Audio clip attached: {} ({} bytes)]",
        audio_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("audio.wav"),
        meta.len()
    ))
}

pub fn capture_and_transcribe_voice(voice_dir: &Path, seconds: u64) -> Result<String, Error> {
    let dur = seconds.clamp(1, 60);
    let _ = fs::create_dir_all(voice_dir);
    if cfg!(windows) || is_wsl_env() {
        let ps_exe = if cfg!(windows) {
            "powershell"
        } else {
            "powershell.exe"
        };
        let script = format!(
            r#"
try {{
    Add-Type -AssemblyName System.Speech
    $rec = New-Object System.Speech.Recognition.SpeechRecognitionEngine
    $rec.LoadGrammar((New-Object System.Speech.Recognition.DictationGrammar))
    $rec.SetInputToDefaultAudioDevice()
    $res = $rec.Recognize([System.TimeSpan]::FromSeconds({dur}))
    if ($null -ne $res -and $res.Text) {{
        Write-Output ("TRANSCRIPT:" + $res.Text)
        exit 0
    }}
    Write-Output "NO_SPEECH"
}} catch {{
    Write-Output ("ERR:" + $_.Exception.Message)
}}
"#
        );
        if let Ok(out) = StdCommand::new(ps_exe)
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .stdin(Stdio::null())
            .output()
        {
            let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            if let Some(tr) = text.strip_prefix("TRANSCRIPT:") {
                let clean = tr.trim().to_owned();
                if !is_whisper_hallucination(&clean) {
                    return Ok(clean);
                }
            } else if let Some(err) = text.strip_prefix("ERR:") {
                return Err(Error::new(
                    "Error",
                    format!("Microphone capture unavailable: {}", err.trim()),
                ));
            }
        }
    }
    Err(Error::new(
        "Error",
        "No speech detected or no microphone input device available. You can also transcribe a recorded WAV file with `/voice file <PATH>`.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::files;

    #[test]
    fn attachment_and_paste_lifecycle_preserves_contracts() -> Result<(), Error> {
        let root = std::env::temp_dir().join(format!(
            "roster-attachments-test-{}-{}",
            std::process::id(),
            files::nonce()?
        ));
        files::create_private_directories(&root)?;
        let img_path = root.join("diagram.png");
        let png_bytes: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3, 4];
        fs::write(&img_path, png_bytes).map_err(|e| Error::new("TestError", e.to_string()))?;

        let doc_path = root.join("architecture notes.md");
        fs::write(&doc_path, "# Architecture\nLine 2\nLine 3\n")
            .map_err(|e| Error::new("TestError", e.to_string()))?;

        let env = Value::object(vec![
            ("HOME", Value::string(&root.display().to_string())),
            ("USERPROFILE", Value::string(&root.display().to_string())),
        ]);

        let drop_img = detect_file_drop(
            &format!("{} explain this architecture diagram", img_path.display()),
            &root,
            &env,
        )
        .ok_or_else(|| Error::new("TestError", "expected image drop match"))?;
        assert!(drop_img.is_image);
        assert_eq!(drop_img.remainder, "explain this architecture diagram");

        let drop_quoted = detect_file_drop(
            &format!("\"{}\" summarize this file", doc_path.display()),
            &root,
            &env,
        )
        .ok_or_else(|| Error::new("TestError", "expected quoted file drop match"))?;
        assert!(!drop_quoted.is_image);
        assert_eq!(drop_quoted.remainder, "summarize this file");

        let drop_spaced = detect_file_drop(
            &format!("{} compare with main", doc_path.display()),
            &root,
            &env,
        )
        .ok_or_else(|| Error::new("TestError", "expected spaced file drop match"))?;
        assert_eq!(drop_spaced.remainder, "compare with main");

        let att1 = build_session_attachment(1, &img_path)?;
        assert_eq!(att1.kind, AttachmentKind::Image);
        assert_eq!(att1.mime_type, "image/png");
        assert!(
            att1.data_uri
                .as_deref()
                .is_some_and(|u| u.starts_with("data:image/png;base64,"))
        );

        let att2 = build_session_attachment(2, &doc_path)?;
        assert_eq!(att2.kind, AttachmentKind::File);
        assert!(
            att2.text_excerpt
                .as_deref()
                .is_some_and(|t| t.contains("# Architecture"))
        );
        let expected_bytes = att1.size_bytes + att2.size_bytes;

        let all_atts = vec![att1, att2];
        let matched_by_id = match_referenced_attachments("Look at #1 and tell me", &all_atts);
        assert_eq!(matched_by_id.len(), 1);
        assert_eq!(matched_by_id[0].id, 1);

        let matched_generic =
            match_referenced_attachments("What is in the pasted file?", &all_atts);
        assert_eq!(matched_generic.len(), 1);
        assert_eq!(matched_generic[0].id, 2);

        assert_eq!(summarize_attachment_inventory(&[]), "0 attachments (0 B)");
        assert_eq!(
            summarize_attachment_inventory(&all_atts),
            format!("2 attachments (1 images, 1 files, {expected_bytes} B)")
        );
        let image_only = vec![all_atts[0].clone()];
        let expected_image_bytes = all_atts[0].size_bytes;
        assert_eq!(
            summarize_attachment_inventory(&image_only),
            format!("1 attachments (1 images, 0 files, {expected_image_bytes} B)")
        );
        let file_only = vec![all_atts[1].clone()];
        let expected_file_bytes = all_atts[1].size_bytes;
        assert_eq!(
            summarize_attachment_inventory(&file_only),
            format!("1 attachments (0 images, 1 files, {expected_file_bytes} B)")
        );

        let pastes_dir = root.join("pastes");
        let multi_line = "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\n";
        let (collapsed, _) = collapse_large_paste(multi_line, 1, &pastes_dir)
            .ok_or_else(|| Error::new("TestError", "expected paste collapse"))?;
        assert!(collapsed.starts_with("[Pasted text #1: 6 lines -> "));
        let expanded = expand_paste_references(&format!("Review this:\n{collapsed}"));
        assert!(expanded.contains("line 5\nline 6"));

        let sample_md =
            "Here is code:\n```rust\nfn main() {}\n```\nAnd more:\n```bash\ncargo test\n```\n";
        assert_eq!(
            extract_last_fenced_code_block(sample_md),
            Some("cargo test".to_owned())
        );
        let osc = format_osc52_sequence("hello", false);
        assert_eq!(osc, "\x1b]52;c;aGVsbG8=\x07");

        let sub_dir = root.join("submod");
        files::create_private_directories(&sub_dir)?;
        fs::write(sub_dir.join("README.md"), "# Submodule Readme\n")
            .map_err(|e| Error::new("TestError", e.to_string()))?;
        let dir_att = build_session_attachment(3, &sub_dir)?;
        assert_eq!(dir_att.kind, AttachmentKind::Directory);
        assert!(
            dir_att
                .text_excerpt
                .as_deref()
                .is_some_and(|t| t.contains("# Submodule Readme"))
        );

        let mentions = extract_inline_at_mentions(
            "Please compare @diagram.png and @submod/README.md:1-1",
            &root,
            &env,
        );
        assert_eq!(mentions.len(), 2);
        assert_eq!(mentions[1].line_range, Some((1, 1)));

        let completions = complete_workspace_path("dia", &root);
        assert_eq!(completions, vec!["diagram.png".to_owned()]);
        let dir_completions = complete_workspace_path("sub", &root);
        assert_eq!(dir_completions, vec!["submod/".to_owned()]);

        let leaked = "hello\x1b[39;1R\x1b[<0;42;12M world";
        assert_eq!(strip_leaked_terminal_sequences(leaked), "hello world");
        assert!(is_whisper_hallucination("Thank you."));
        assert!(!is_whisper_hallucination(
            "Add a unit test to rust/shell.rs"
        ));

        let _ = fs::remove_dir_all(&root);
        Ok(())
    }
}
