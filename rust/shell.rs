use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use crate::ask::strip_vt_and_controls;
use crate::config;
use crate::domain;
use crate::json::Value;
use crate::llm::redact::Redactor;
use crate::paths;
use crate::settings;
use crate::state::{files, redaction, secure};
use crate::text::{Text, trim};
use crate::{Error, MAX_SAFE_INTEGER, arguments};

pub const PRODUCT: &str = "github-agent-roster";
pub const COMMAND_GROUPS: &[&str] = &["Session", "Ask", "Model", "Board", "Human", "Settings"];
pub const SHELL_EVALUATION_USAGE: &str = "Use /eval SESSION accept|reject|rework 1-5 y|n --minutes N --note \"what should have been done differently, and how the model performed\".";

const MAX_HISTORY_LINES: usize = 200;
const MAX_HISTORY_FILE_BYTES: u64 = 4 * 1024 * 1024;
const BAR_CELLS: usize = 10;
const FULL_COLUMNS: usize = 76;
const PERCENT_COLUMNS: usize = 52;
const RULE_CHAR: &str = "\u{2500}";
const SEPARATOR: &str = " \u{2502} ";
const STEP_DOT: &str = " \u{00b7} ";
const STEP_MAX: usize = 200;

fn ansi_paint(value: &str, color: &str, enabled: bool) -> String {
    if !enabled {
        return value.to_owned();
    }
    let code = match color {
        "label" => "\x1b[96m",
        "white" => "\x1b[97m",
        "warn" | "yellow" => "\x1b[93m",
        "orange" => "\x1b[38;5;208m",
        "red" => "\x1b[91m",
        "green" => "\x1b[92m",
        _ => return value.to_owned(),
    };
    format!("{code}{value}\x1b[0m")
}

pub fn strip_vt_only(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut stripped = String::with_capacity(source.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == 0x1b {
            match bytes.get(index + 1).copied() {
                Some(b'[') => {
                    let mut scan = index + 2;
                    while scan < bytes.len() && (0x20..=0x3f).contains(&bytes[scan]) {
                        scan += 1;
                    }
                    if scan < bytes.len() && (0x40..=0x7e).contains(&bytes[scan]) {
                        index = scan + 1;
                        continue;
                    }
                }
                Some(b']') => {
                    let mut scan = index + 2;
                    let mut terminated = None;
                    while scan < bytes.len() {
                        if bytes[scan] == 0x07 {
                            terminated = Some(scan + 1);
                            break;
                        }
                        if bytes[scan] == 0x1b && bytes.get(scan + 1) == Some(&b'\\') {
                            terminated = Some(scan + 2);
                            break;
                        }
                        scan += 1;
                    }
                    if let Some(next_index) = terminated {
                        index = next_index;
                        continue;
                    }
                }
                Some(next) if (0x40..=0x5f).contains(&next) => {
                    index += 2;
                    continue;
                }
                _ => {}
            }
        }
        let Some(ch) = source[index..].chars().next() else {
            break;
        };
        index += ch.len_utf8();
        stripped.push(ch);
    }
    stripped
}

fn clean_display(value: Option<&Value>, default_text: &str) -> String {
    let raw = match value.filter(|v| **v != Value::Null) {
        Some(Value::String(text)) => text.to_utf8().unwrap_or_else(|_| default_text.to_owned()),
        Some(other) => other.json(),
        None => default_text.to_owned(),
    };
    let no_vt = strip_vt_only(&raw);
    let cleaned: String = no_vt
        .chars()
        .filter(|ch| !matches!(*ch, '\x00'..='\x1f' | '\x7f'))
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        default_text.to_owned()
    } else {
        trimmed.to_owned()
    }
}

pub fn shell_name(env: Option<&Value>) -> String {
    let env_str = |key: &str| -> Option<String> {
        if let Some(env) = env {
            env.get(key)
                .and_then(Value::as_string)
                .and_then(|t| t.to_utf8().ok())
                .filter(|s| !s.trim().is_empty())
        } else {
            std::env::var(key).ok().filter(|s| !s.trim().is_empty())
        }
    };
    let base_lower = |path_str: &str| -> String {
        let name = path_str
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(path_str)
            .to_lowercase();
        name.strip_suffix(".exe").unwrap_or(&name).to_owned()
    };
    if let Some(shell) = env_str("SHELL") {
        return base_lower(&shell);
    }
    if env_str("PSModulePath").is_some() {
        return "powershell".to_owned();
    }
    if let Some(comspec) = env_str("ComSpec") {
        return base_lower(&comspec);
    }
    "-".to_owned()
}

pub fn format_banner(facts: &Value, color: bool) -> String {
    let field = |name: &str| ansi_paint(&clean_display(facts.get(name), "-"), "white", color);
    let warnings = facts
        .get("warnings")
        .and_then(domain::items)
        .map(|items| {
            items
                .iter()
                .map(|item| clean_display(Some(item), "-"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let warn_summary = if warnings.is_empty() {
        ansi_paint("none", "white", color)
    } else {
        ansi_paint(&warnings.len().to_string(), "warn", color)
    };
    let mut lines = vec![
        format!(
            "{}  {}",
            ansi_paint(PRODUCT, "label", color),
            field("version")
        ),
        format!(
            "{} {} \u{00b7} {} \u{00b7} {} \u{00b7} {}",
            ansi_paint("node", "label", color),
            field("node"),
            field("shell"),
            field("cwd"),
            field("branch")
        ),
        format!(
            "{} {} \u{00b7} {} \u{00b7} {}",
            ansi_paint("endpoint", "label", color),
            field("endpoint"),
            field("model"),
            field("host")
        ),
        format!(
            "{} {} \u{00b7} {} {} \u{00b7} {} {warn_summary}",
            ansi_paint("contracts", "label", color),
            field("contracts"),
            ansi_paint("update", "label", color),
            field("update"),
            ansi_paint("warnings", "label", color),
        ),
    ];
    for warning in warnings {
        lines.push(ansi_paint(&warning, "warn", color));
    }
    format!("{}\n", lines.join("\n"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandPage {
    pub name: &'static str,
    pub group: &'static str,
    pub usage: &'static str,
    pub description: &'static str,
    pub example: &'static str,
    pub flags: &'static [&'static str],
    pub aliases: &'static [&'static str],
}

pub fn commands() -> &'static [CommandPage] {
    &[
        CommandPage {
            name: "help",
            group: "Session",
            usage: "/help [GROUP|COMMAND]",
            description: "Show command groups or a complete command page.",
            example: "/help run",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "learn",
            group: "Human",
            usage: "/learn --recurring",
            description: "Draft improvement proposals from three recurring slice failures; never installs skills, principals or routing changes.",
            example: "/learn --recurring",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "status",
            group: "Session",
            usage: "/status [N] [--offline]",
            description: "Show cached session fields without a model or GitHub request.",
            example: "/status",
            flags: &["--offline: use local cached evidence only"],
            aliases: &[],
        },
        CommandPage {
            name: "history",
            group: "Session",
            usage: "/history",
            description: "Show the last 20 stored safe commands.",
            example: "/history",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "resume",
            group: "Session",
            usage: "/resume [N] [--green]",
            description: "List local issue runs with no argument or resume a valid TASK. --green restores last verified product state, preserves later edits in a checkpoint, and pauses before execution.",
            example: "/resume 108",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "worktrees",
            group: "Session",
            usage: "/worktrees",
            description: "List registered issue worktree path, isolated branch, last known seat and dirty/clean Git state.",
            example: "/worktrees",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "batch",
            group: "Session",
            usage: "/batch",
            description: "Refused: One seat at a time. Worktrees are isolated. No parallel execution is started.",
            example: "/batch",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "recap",
            group: "Session",
            usage: "/recap",
            description: "Print one human metadata line: outcome, files, last test, review and finish reason. No completion body.",
            example: "/recap",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "btw",
            group: "Session",
            usage: "/btw QUESTION",
            description: "Ask one read-only model question about current task metadata; no tools, tests, writes, memory or publication changes.",
            example: "/btw Which acceptance check covers this edge case?",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "context",
            group: "Session",
            usage: "/context",
            description: "Show last measured seat provider, model, effort, input, output, context max, finish reason, character pack budget and prior-feedback inclusion. Missing counts are -.",
            example: "/context",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "usage",
            group: "Session",
            usage: "/usage",
            description: "Show one panel for the current session: model, endpoint, prompt and completion tokens, context max, effort, finish reason, tool calls, elapsed, thinking and the outbound completion cap. Missing values are -.",
            example: "/usage",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "map",
            group: "Session",
            usage: "/map",
            description: "Write a filename-only repo map: top two directory levels plus TASK paths, capped at 80 lines. Coder reads require non-docs difficulty4+.",
            example: "/map",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "checkpoints",
            group: "Session",
            usage: "/checkpoints",
            description: "List current-task checkpoint number, seat, short status and time.",
            example: "/checkpoints",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "rewind",
            group: "Session",
            usage: "/rewind N",
            description: "Restore checkpoint-covered product files; keep TASK/PLAN/logs. Refuse published PRs. /undo means latest only.",
            example: "/rewind 1",
            flags: &[],
            aliases: &["undo"],
        },
        CommandPage {
            name: "redraw",
            group: "Session",
            usage: "/redraw",
            description: "Repaint the tray without clearing scrollback.",
            example: "/redraw",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "clear",
            group: "Session",
            usage: "/clear",
            description: "Clear the screen and repaint the tray.",
            example: "/clear",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "quit",
            group: "Session",
            usage: "/quit",
            description: "Exit with code 0; bare exit is also accepted.",
            example: "/q",
            flags: &[],
            aliases: &["q"],
        },
        CommandPage {
            name: "question",
            group: "Ask",
            usage: "/question [--files PATH,PATH --] TEXT",
            description: "Answer a bounded repository question from explicitly scoped public source excerpts; no task, tools, tests, writes or publication.",
            example: "/question --files src/api.mjs -- How does this work?",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "ask",
            group: "Ask",
            usage: "/ask TEXT",
            description: "Run a local Ask through planner, coder and reviewer without creating a GitHub issue.",
            example: "/ask Add a Status section to README.md.",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "clarify",
            group: "Ask",
            usage: "/clarify [show|cancel|continue|TEXT]",
            description: "Inspect private pending intake after a restart; resume with explicit Allowed Files or a relative output directory. Never infers scope.",
            example: "/clarify show",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "plan",
            group: "Ask",
            usage: "/plan TEXT",
            description: "Explore read-only and write PLAN.md only; Enter accepts a slice and /stop keeps it without coding.",
            example: "/plan Add a Status section to README.md.",
            flags: &["--plan: use /run N --plan for the same gate on an issue"],
            aliases: &[],
        },
        CommandPage {
            name: "run",
            group: "Ask",
            usage: "/run N [--parallel K] [--attempts K] [--confirm|--plan] [--auto-model]",
            description: "Run an existing issue in builtin seats.",
            example: "/run 108 --confirm",
            flags: &[
                "--confirm: pause after the task summary; Enter continues and /stop cancels",
                "--plan: explore and write PLAN.md only before human acceptance",
                "--auto-model: route from fleet priors and human evaluations",
                "--parallel K: isolated ready child waves, bounded by declared capacity",
                "--attempts K: isolated slice candidates on distinct fleet profiles; publish only the best passing attempt",
            ],
            aliases: &[],
        },
        CommandPage {
            name: "retry",
            group: "Ask",
            usage: "/retry",
            description: "Repeat the last Ask or issue run in the same worktree without worktree add.",
            example: "/retry",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "stop",
            group: "Ask",
            usage: "/stop",
            description: "Cancel the active seat or confirmed task, like one Ctrl+C.",
            example: "/stop",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "steer",
            group: "Ask",
            usage: "/steer TEXT",
            description: "Interrupt the drafting coder model call and send queued text plus this instruction next. It cannot widen Allowed Files or edit TASK.md; Ctrl+C cancels without steering. A bare line during a drafting coder seat is the same action.",
            example: "/steer Keep the public API unchanged.",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "queue",
            group: "Ask",
            usage: "/queue [TEXT|list|drop N|clear]",
            description: "Queue an Ask while a seat is running. list shows the queue, drop N removes one, clear removes all. A bare line while a seat is running queues an Ask unless the coder is waiting for steering. /q remains quit.",
            example: "/queue Add a Status section to README.md.",
            flags: &[],
            aliases: &["qs"],
        },
        CommandPage {
            name: "remediate",
            group: "Ask",
            usage: "/remediate [TEST ...]",
            description: "Start a separate remediation agent for failing tests in this workspace. It edits only those tests and uses the largest-context fleet profile.",
            example: "/remediate tests/repl.test.mjs",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "publish",
            group: "Ask",
            usage: "/publish [SUBJECT] [--model MODEL] [--skip-review]",
            description: "Publish reviewed changes only through the GitHub App SDK.",
            example: "/publish feat: add status --model GPT-6.1-Sol",
            flags: &[
                "--model MODEL: actual publishing model; measured seat metadata wins",
                "--skip-review: if review failed, explicitly bypass that verdict only, not tests or policy",
            ],
            aliases: &[],
        },
        CommandPage {
            name: "review",
            group: "Ask",
            usage: "/review [--again]",
            description: "Run only the read-only reviewer on current verified task/diff evidence. It writes REVIEW.md, never product code. Failed review blocks publication.",
            example: "/review --again",
            flags: &["--again: archive an existing managed report and rerun the current diff"],
            aliases: &[],
        },
        CommandPage {
            name: "model",
            group: "Model",
            usage: "/model [ID|clear] [--save]",
            description: "Show model and host; select for this session unless --save is explicit.",
            example: "/model deepseek-v4.1-flash --save",
            flags: &["--save: write the private model setting"],
            aliases: &[],
        },
        CommandPage {
            name: "effort",
            group: "Model",
            usage: "/effort [l|m|h|x|none|status]",
            description: "Select session effort without writing config; docs cap at high and length retries drop reasoning.",
            example: "/effort l",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "provider",
            group: "Model",
            usage: "/provider",
            description: "Show session profile name and host, never a key.",
            example: "/provider",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "fleet",
            group: "Model",
            usage: "/fleet [list|use ID|probe [ID] [--set-model [MODEL]]|add FLAGS|discover [--wsl] [--save]]",
            description: "List or use session profiles, probe models, discover local/WSL endpoints, or add with the existing fleet flags.",
            example: "/fleet use spark-4",
            flags: &[
                "--set-model [MODEL]: explicitly save a listed model to private config",
                "--id, --base-url, --model, --context, --concurrency, --hardware, --task-class: existing fleet add flags",
                "--wsl: include running WSL addresses; --save: add discovered profiles without changing the default",
            ],
            aliases: &[],
        },
        CommandPage {
            name: "stats",
            group: "Model",
            usage: "/stats [REF] | /stats --delivery [--json]",
            description: "Read model or delivery metrics without selecting a model.",
            example: "/stats --delivery",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "issues",
            group: "Board",
            usage: "/issues",
            description: "List up to 100 open issue numbers and titles only; never bodies.",
            example: "/issues",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "waves",
            group: "Board",
            usage: "/waves [open]",
            description: "Read PLAN.md child drafts and GitHub states. Only explicit open creates those drafts; earlier open waves block later starts. No second board is stored.",
            example: "/waves",
            flags: &["open: explicitly create only missing drafts already validated in PLAN.md"],
            aliases: &[],
        },
        CommandPage {
            name: "issue",
            group: "Board",
            usage: "/issue N",
            description: "Show cached title/state/branch/PR metadata; query GitHub only on a cache miss.",
            example: "/issue 108",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "diff",
            group: "Board",
            usage: "/diff",
            description: "Print tracked git diff filenames in the current issue worktree, never file bodies.",
            example: "/diff",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "log",
            group: "Board",
            usage: "/log N|debug",
            description: "Tail local seat logs; debug tail is refused while debug is off.",
            example: "/log 108",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "eval",
            group: "Human",
            usage: "/eval TARGET accept|reject|rework --minutes N --difficulty 1-5 \"TEXT\"",
            description: "Record a human evaluation; agents cannot call it. Legacy positional difficulty and again remain accepted.",
            example: "/eval roster-108-coder accept --minutes 15 --difficulty 2 \"Meets the checks.\"",
            flags: &[
                "--minutes N: actual human elapsed minutes",
                "--difficulty 1-5: human task difficulty",
            ],
            aliases: &[],
        },
        CommandPage {
            name: "recommend",
            group: "Human",
            usage: "/recommend [feat|fix|docs|test] [--difficulty 1-5]",
            description: "Read a route for the last task or default feat/difficulty2 without changing the model.",
            example: "/recommend",
            flags: &["--difficulty 1-5: required task capacity"],
            aliases: &[],
        },
        CommandPage {
            name: "statusbar",
            group: "Settings",
            usage: "/statusbar on|off",
            description: "Toggle both tray bars for this process.",
            example: "/statusbar off",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "debug",
            group: "Settings",
            usage: "/debug on|off|status",
            description: "Toggle or show metadata-only debug logging for this process.",
            example: "/debug status",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "doctor",
            group: "Settings",
            usage: "/doctor [warm|copilot [--opt-in] [--runtime PATH] [--model ID]]",
            description: "Run existing prerequisite checks, a read-only warm probe, or blocked offline Copilot diagnostics; never enables a worker or starts login.",
            example: "/doctor copilot --opt-in",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "config",
            group: "Settings",
            usage: "/config [path|set KEY VALUE]",
            description: "Show redacted private config or its path. Set only effort/context budget; statusbar/debug are process-only. Endpoints, PEM paths and policy are refused.",
            example: "/config set context.budget 12000",
            flags: &[],
            aliases: &[],
        },
        CommandPage {
            name: "vault",
            group: "Settings",
            usage: "/vault [list]|get NAME|set NAME",
            description: "Manage named secrets without showing values.",
            example: "/vault set ROSTER_API_KEY",
            flags: &[],
            aliases: &[],
        },
    ]
}

pub fn canonical_command(name: &str) -> &str {
    commands()
        .iter()
        .find(|cmd| cmd.name == name || cmd.aliases.contains(&name))
        .map_or(name, |cmd| cmd.name)
}

pub fn complete_command(line: &str) -> (Vec<String>, String) {
    if !line.starts_with('/') || !line[1..].bytes().all(|b| b.is_ascii_lowercase()) {
        return (Vec::new(), line.to_owned());
    }
    let mut matches = commands()
        .iter()
        .flat_map(|cmd| std::iter::once(cmd.name).chain(cmd.aliases.iter().copied()))
        .map(|name| format!("/{name}"))
        .filter(|candidate| candidate.starts_with(line))
        .collect::<Vec<_>>();
    matches.sort();
    (matches, line.to_owned())
}

pub fn format_help(target: &str) -> Option<String> {
    let trimmed = target.trim();
    let selected = trimmed.strip_prefix('/').unwrap_or(trimmed).to_lowercase();
    let command = commands()
        .iter()
        .find(|entry| entry.name == selected || entry.aliases.contains(&selected.as_str()));
    let group = COMMAND_GROUPS
        .iter()
        .copied()
        .find(|name| name.eq_ignore_ascii_case(&selected));
    if let Some(command) = command.filter(|_| group.is_none()) {
        let aliases = if command.aliases.is_empty() {
            "(none)".to_owned()
        } else {
            command
                .aliases
                .iter()
                .map(|alias| format!("/{alias}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let quit_suffix = if command.name == "quit" {
            "; bare exit"
        } else {
            ""
        };
        let flags = if command.flags.is_empty() {
            "  (none)".to_owned()
        } else {
            command
                .flags
                .iter()
                .map(|flag| format!("  {flag}"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        return Some(format!(
            "Usage: {}\n{}\nAliases: {aliases}{quit_suffix}\nFlags:\n{flags}\nExample: {}\n",
            command.usage, command.description, command.example
        ));
    }
    if !selected.is_empty() && group.is_none() {
        return None;
    }
    let groups = match group {
        Some(group) => vec![group],
        None => COMMAND_GROUPS.to_vec(),
    };
    let sections = groups
        .into_iter()
        .map(|group_name| {
            let items = commands()
                .iter()
                .filter(|entry| entry.group == group_name)
                .map(|entry| format!("  {}", entry.usage))
                .collect::<Vec<_>>()
                .join("\n");
            format!("{group_name}\n{items}")
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    Some(format!("Commands:\n{sections}\n"))
}

pub fn format_context_readout(
    measured: Option<&Value>,
    pack_budget_chars: Option<u64>,
    env: Option<&Value>,
) -> Result<String, Error> {
    let redaction_cfg = redaction::Config {
        environment: env
            .cloned()
            .unwrap_or_else(crate::ask::environment_value_from_process),
        ..redaction::Config::default()
    };
    redaction_cfg.validate()?;
    let safe = |field: &str| -> Result<String, Error> {
        let raw = match measured
            .and_then(|m| m.get(field))
            .filter(|v| **v != Value::Null)
        {
            Some(Value::String(text)) => text.to_utf8()?,
            Some(other) => other.json(),
            None => "-".to_owned(),
        };
        let redacted = redaction::evidence(&Text::from(raw.as_str()), &redaction_cfg).to_utf8()?;
        let stripped = strip_vt_and_controls(&redacted);
        Ok(stripped
            .chars()
            .map(|ch| {
                if matches!(ch, '\x00'..='\x1f' | '\x7f') {
                    '?'
                } else {
                    ch
                }
            })
            .take(240)
            .collect())
    };
    let count = |val: Option<&Value>| -> Result<String, Error> {
        let Some(val) = val.filter(|v| **v != Value::Null) else {
            return Ok("-".to_owned());
        };
        let num = domain::safe_integer(Some(val))
            .ok_or_else(|| Error::type_error("Context readout count is invalid"))?;
        Ok(num.to_string())
    };
    let included = match measured.and_then(|m| m.get("priorFeedbackIncluded")) {
        None => "-",
        Some(Value::Boolean(true)) => "yes",
        Some(Value::Boolean(false)) => "no",
        Some(_) => return Err(Error::type_error("Context feedback metadata is invalid")),
    };
    let budget_val = measured
        .and_then(|m| m.get("packBudgetChars"))
        .filter(|v| **v != Value::Null);
    let budget_str = match budget_val {
        Some(val) => count(Some(val))?,
        None => match pack_budget_chars {
            None => "-".to_owned(),
            Some(n) if n <= MAX_SAFE_INTEGER => n.to_string(),
            Some(_) => return Err(Error::type_error("Context readout count is invalid")),
        },
    };
    Ok(format!(
        "Seat: {}\nProvider: {}\nModel: {}\nEffort: {}\nInput: {}\nOutput: {}\nContext max: {}\nFinish reason: {}\nPack budget (characters): {budget_str}\nPrior feedback included: {included}\n",
        safe("seat")?,
        safe("provider")?,
        safe("model")?,
        safe("effort")?,
        count(measured.and_then(|m| m.get("input")))?,
        count(measured.and_then(|m| m.get("output")))?,
        count(measured.and_then(|m| m.get("contextMax")))?,
        safe("finishReason")?,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellEvaluation {
    pub values: [String; 4],
    pub minutes: u64,
    pub comment: String,
    pub idempotent: bool,
}

pub fn parse_shell_evaluation_args(text: &str) -> Result<ShellEvaluation, Error> {
    let usage = || Error::type_error(SHELL_EVALUATION_USAGE);
    let args = arguments::split(&Text::from(text), SHELL_EVALUATION_USAGE)?
        .into_iter()
        .map(|arg| arg.to_utf8())
        .collect::<Result<Vec<_>, _>>()?;
    if args.len() < 4 || args[0].is_empty() || args[0].starts_with('-') {
        return Err(usage());
    }
    if !["accept", "reject", "rework"].contains(&args[1].as_str()) {
        return Err(usage());
    }
    if !matches!(args[2].as_str(), "1" | "2" | "3" | "4" | "5") {
        return Err(usage());
    }
    if !matches!(args[3].as_str(), "y" | "n") {
        return Err(usage());
    }
    let mut minutes = None;
    let mut note: Option<String> = None;
    let mut index = 4;
    while index < args.len() {
        let Some(flag) = args.get(index) else {
            return Err(usage());
        };
        let Some(value) = args.get(index + 1).filter(|val| !val.starts_with("--")) else {
            return Err(usage());
        };
        match flag.as_str() {
            "--minutes" if minutes.is_none() => {
                let valid = value == "0"
                    || (!value.is_empty()
                        && !value.starts_with('0')
                        && value.bytes().all(|b| b.is_ascii_digit()));
                let parsed = valid
                    .then(|| value.parse::<u64>().ok())
                    .flatten()
                    .filter(|n| *n <= MAX_SAFE_INTEGER)
                    .ok_or_else(usage)?;
                minutes = Some(parsed);
            }
            "--note" if note.is_none() => {
                note = Some(value.clone());
            }
            _ => return Err(usage()),
        }
        index += 2;
    }
    let minutes = minutes.ok_or_else(usage)?;
    if matches!(args[1].as_str(), "reject" | "rework")
        && note
            .as_deref()
            .is_none_or(|n| trim(&Text::from(n).0).is_empty())
    {
        return Err(Error::type_error(
            "A reject or rework requires --note before any evaluation is written.",
        ));
    }
    Ok(ShellEvaluation {
        values: [
            args[0].clone(),
            args[1].clone(),
            args[2].clone(),
            args[3].clone(),
        ],
        minutes,
        comment: note.unwrap_or_default(),
        idempotent: true,
    })
}

pub fn safe_history_line(line: &str, env: Option<&Value>, secret: bool) -> bool {
    if secret || trim(&Text::from(line).0).is_empty() || line.len() > 16_384 {
        return false;
    }
    if line
        .chars()
        .any(|ch| matches!(ch, '\x00'..='\x1f' | '\x7f'))
    {
        return false;
    }
    let trimmed = line.trim();
    if regex_lite::Regex::new(r"(?i)^/vault(?:\s|$)").is_ok_and(|re| re.is_match(trimmed))
        || regex_lite::Regex::new(r"(?i)\.pem(?:\b|$)").is_ok_and(|re| re.is_match(line))
        || regex_lite::Regex::new(
            r"(?i)\b(?:password|passwd|token|private[_ -]?key|authorization|bearer)\b",
        )
        .is_ok_and(|re| re.is_match(line))
    {
        return false;
    }
    let env_val = env
        .cloned()
        .unwrap_or_else(crate::ask::environment_value_from_process);
    let redactor = Redactor::new(&env_val, None);
    let Ok(redacted) = redactor.redact(&Text::from(line), None).to_utf8() else {
        return false;
    };
    let redaction_cfg = redaction::Config {
        environment: env_val,
        ..redaction::Config::default()
    };
    let Ok(evidence_redacted) = redaction::evidence(&Text::from(line), &redaction_cfg).to_utf8()
    else {
        return false;
    };
    redacted == line && evidence_redacted == line
}

pub struct ShellHistory {
    repo_root: PathBuf,
    file: PathBuf,
    env: Value,
    lines: Vec<String>,
    loaded: bool,
}

impl ShellHistory {
    pub fn new(repo_root: &Path, env: Option<&Value>) -> Self {
        Self {
            repo_root: repo_root.to_path_buf(),
            file: repo_root.join(".roster").join("history"),
            env: env
                .cloned()
                .unwrap_or_else(crate::ask::environment_value_from_process),
            lines: Vec::new(),
            loaded: false,
        }
    }

    pub fn path(&self) -> &Path {
        &self.file
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn load(&mut self) -> Result<Vec<String>, Error> {
        paths::ensure_local_path(&self.file, &self.repo_root)?;
        match fs::symlink_metadata(&self.file) {
            Ok(meta) => {
                let single_err = || {
                    Error::new(
                        "Error",
                        "Shell history must be a regular, single-link file of at most 4 MiB",
                    )
                };
                if secure::linked(&meta) || !meta.is_file() || meta.len() > MAX_HISTORY_FILE_BYTES {
                    return Err(single_err());
                }
                let before_info = paths::file_information(&self.file).map_err(|_| single_err())?;
                let handle = fs::File::open(&self.file).map_err(|_| single_err())?;
                let info = paths::handle_information(&handle).map_err(|_| single_err())?;
                let handle_meta = handle.metadata().map_err(|_| single_err())?;
                if !handle_meta.is_file()
                    || info.links != 1
                    || info.identity != before_info.identity
                    || handle_meta.len() > MAX_HISTORY_FILE_BYTES
                {
                    return Err(single_err());
                }
                let mut bytes = Vec::new();
                handle
                    .take(MAX_HISTORY_FILE_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|error| Error::new("Error", error.to_string()))?;
                if bytes.len() as u64 > MAX_HISTORY_FILE_BYTES {
                    return Err(single_err());
                }
                let content = String::from_utf8(bytes)
                    .map_err(|error| Error::new("Error", error.to_string()))?;
                let filtered = content
                    .replace("\r\n", "\n")
                    .split('\n')
                    .filter(|line| safe_history_line(line, Some(&self.env), false))
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                let start = filtered.len().saturating_sub(MAX_HISTORY_LINES);
                self.lines = filtered[start..].to_vec();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(&self.file, fs::Permissions::from_mode(0o600));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.lines.clear();
            }
            Err(error) => return Err(Error::new("Error", error.to_string())),
        }
        self.loaded = true;
        Ok(self.lines.clone())
    }

    pub fn record(&mut self, line: &str, secret: bool) -> Result<(), Error> {
        if !self.loaded {
            return Err(Error::new(
                "Error",
                "Load shell history before recording commands",
            ));
        }
        if !safe_history_line(line, Some(&self.env), secret) {
            return Ok(());
        }
        self.lines.retain(|entry| entry != line);
        self.lines.push(line.to_owned());
        if self.lines.len() > MAX_HISTORY_LINES {
            let start = self.lines.len() - MAX_HISTORY_LINES;
            self.lines = self.lines[start..].to_vec();
        }
        let snapshot = format!("{}\n", self.lines.join("\n"));
        paths::ensure_local_path(&self.file, &self.repo_root)?;
        let has_git = match fs::symlink_metadata(self.repo_root.join(".git")) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(Error::new("Error", error.to_string())),
        };
        if has_git {
            let git = std::env::var_os("ROSTER_TEST_GIT")
                .map_or_else(|| PathBuf::from("git"), PathBuf::from);
            let status = StdCommand::new(git)
                .current_dir(&self.repo_root)
                .args(["check-ignore", "--quiet", "--", ".roster/history"])
                .status()
                .map_err(|error| Error::new("Error", error.to_string()))?;
            if !status.success() {
                if status.code() == Some(1) {
                    return Err(Error::new(
                        "Error",
                        "Shell history must be untracked and gitignored; ignore .roster/history",
                    ));
                }
                return Err(Error::new("Error", "git check-ignore failed"));
            }
        }
        if let Some(parent) = self.file.parent() {
            files::create_private_directories(parent)?;
        }
        paths::ensure_local_path(&self.file, &self.repo_root)?;
        match fs::symlink_metadata(&self.file) {
            Ok(current) => {
                let links = paths::file_information(&self.file).map_or(0, |info| info.links);
                if !current.is_file() || secure::linked(&current) || links != 1 {
                    return Err(Error::new(
                        "Error",
                        "Shell history must remain a regular, single-link file",
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(Error::new("Error", error.to_string())),
        }
        let temporary = PathBuf::from(format!("{}.{}", self.file.display(), files::nonce()?));
        let write_result = (|| -> Result<(), Error> {
            let mut open = OpenOptions::new();
            open.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                open.mode(0o600);
            }
            let mut handle = open
                .open(&temporary)
                .map_err(|error| Error::new("Error", error.to_string()))?;
            handle
                .write_all(snapshot.as_bytes())
                .and_then(|()| handle.sync_all())
                .map_err(|error| Error::new("Error", error.to_string()))?;
            drop(handle);
            fs::rename(&temporary, &self.file)
                .map_err(|error| Error::new("Error", error.to_string()))?;
            Ok(())
        })();
        let _ = fs::remove_file(&temporary);
        write_result
    }
}

pub fn private_config_path(installation: &Path, cwd: &Path) -> Result<PathBuf, Error> {
    Ok(config::resolve_config_root(installation, cwd)?
        .join(".roster")
        .join("config.yml"))
}

pub fn public_config(
    installation: &Path,
    cwd: &Path,
    env: Option<&Value>,
) -> Result<String, Error> {
    let env_val = env
        .cloned()
        .unwrap_or_else(crate::ask::environment_value_from_process);
    let file = private_config_path(installation, cwd)?;
    let (cfg, is_defaults) = match settings::read_config_file(&file) {
        Ok(source) => (config::parse(&source)?, false),
        Err(error) if error.code == Some("ENOENT") => (config::defaults()?, true),
        Err(error) => return Err(error),
    };
    let formatted = config::format(cfg.as_value())?;
    let api_key_env = cfg
        .as_value()
        .get("llm")
        .and_then(|l| l.get("api_key_env"))
        .and_then(Value::as_string)
        .cloned()
        .unwrap_or_else(|| Text::from("ROSTER_API_KEY"));
    let redaction_cfg = redaction::Config {
        environment: env_val,
        api_key_env,
        ..redaction::Config::default()
    };
    redaction_cfg.validate()?;
    let redacted = redaction::evidence(&formatted, &redaction_cfg).to_utf8()?;
    let pem_re = regex_lite::Regex::new(r#"(?i)"(?:\\.|[^"\\])*\.pem(?:\\.|[^"\\])*""#)
        .map_err(|error| Error::new("Error", error.to_string()))?;
    let safe_text = pem_re.replace_all(&redacted, "\"[redacted PEM path]\"");
    let prefix = if is_defaults {
        "Private config has not been written; showing installed defaults.\n"
    } else {
        ""
    };
    Ok(format!("{prefix}{safe_text}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigSettingValue {
    Text(String),
    Boolean(bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigSetting {
    pub field: &'static str,
    pub value: ConfigSettingValue,
}

pub fn config_setting(
    key: &str,
    value: &str,
    env: Option<&Value>,
    api_key_env: Option<&str>,
) -> Result<ConfigSetting, Error> {
    let env_val = env
        .cloned()
        .unwrap_or_else(crate::ask::environment_value_from_process);
    let redaction_cfg = redaction::Config {
        environment: env_val,
        api_key_env: api_key_env.map_or_else(|| Text::from("ROSTER_API_KEY"), Text::from),
        ..redaction::Config::default()
    };
    redaction_cfg.validate()?;
    let redacted = redaction::evidence(&Text::from(value), &redaction_cfg).to_utf8()?;
    let forbidden_re = regex_lite::Regex::new(r"(?i)\.pem\b|https?://|\bagent-policy\b")
        .map_err(|error| Error::new("Error", error.to_string()))?;
    if forbidden_re.is_match(value) || redacted != value {
        return Err(Error::type_error(
            "Config set refuses secrets, PEM paths, endpoints, and policy.",
        ));
    }
    if matches!(key, "effort" | "llm.effort") {
        if !["l", "m", "h", "x", "none"].contains(&value) {
            return Err(Error::type_error(
                "Config effort must be l, m, h, x, or none.",
            ));
        }
        return Ok(ConfigSetting {
            field: "effort",
            value: ConfigSettingValue::Text(value.to_owned()),
        });
    }
    if matches!(
        key,
        "context"
            | "context-budget"
            | "context_budget"
            | "context.budget"
            | "context_chars"
            | "seat.context_chars"
    ) {
        let valid = !value.is_empty()
            && !value.starts_with('0')
            && value.bytes().all(|b| b.is_ascii_digit())
            && value
                .parse::<u64>()
                .ok()
                .is_some_and(|n| (1..=MAX_SAFE_INTEGER).contains(&n));
        if !valid {
            return Err(Error::type_error(
                "Config context budget must be a positive safe integer.",
            ));
        }
        return Ok(ConfigSetting {
            field: "context_budget",
            value: ConfigSettingValue::Text(value.to_owned()),
        });
    }
    if matches!(key, "statusbar" | "debug") {
        if !["on", "off", "true", "false"].contains(&value) {
            return Err(Error::type_error(
                "Config session toggle must be on or off.",
            ));
        }
        let field = if key == "statusbar" {
            "statusbar"
        } else {
            "debug"
        };
        return Ok(ConfigSetting {
            field,
            value: ConfigSettingValue::Boolean(matches!(value, "on" | "true")),
        });
    }
    Err(Error::type_error(
        "Config set allows only effort, context budget, statusbar, and debug.",
    ))
}

pub fn format_session_status(state: &Value, env: Option<&Value>) -> Result<String, Error> {
    let empty = Value::object(Vec::new());
    let display = state.get("display").unwrap_or(&empty);
    let env_val = env
        .cloned()
        .unwrap_or_else(crate::ask::environment_value_from_process);
    let redactor = Redactor::new(&env_val, None);
    let safe = |val: Option<&Value>, default_str: &str| -> Result<String, Error> {
        let raw = match val.filter(|v| **v != Value::Null) {
            Some(Value::String(text)) => {
                let s = text.to_utf8()?;
                if s.is_empty() {
                    default_str.to_owned()
                } else {
                    s
                }
            }
            Some(other) => other.json(),
            None => default_str.to_owned(),
        };
        let redacted = redactor.redact(&Text::from(raw.as_str()), None).to_utf8()?;
        let no_vt = strip_vt_only(&redacted);
        Ok(no_vt
            .chars()
            .map(|ch| {
                if matches!(ch, '\x00'..='\x1f' | '\x7f') {
                    '?'
                } else {
                    ch
                }
            })
            .take(240)
            .collect())
    };
    let issue_str = match display.get("issue") {
        None | Some(Value::Null) => "local".to_owned(),
        Some(val) => format!("#{}", safe(Some(val), "-")?),
    };
    let raw_effort = safe(display.get("effort"), "-")?;
    let effort_expanded = match raw_effort.as_str() {
        "l" => "low",
        "m" => "medium",
        "h" => "high",
        "x" => "max",
        other => other,
    };
    let mut out = format!(
        "Issue: {issue_str}\nBranch: {}\nSeat: {}\nState: {}\nModel: {}\nHost: {}\nEffort: {effort_expanded}\nLast finish reason: {}\nLast test name: {}\nReview: {}\n",
        safe(display.get("branch"), "-")?,
        safe(display.get("seat"), "-")?,
        safe(display.get("state"), "-")?,
        safe(display.get("model"), "-")?,
        safe(display.get("host"), "-")?,
        safe(display.get("lastFinishReason"), "-")?,
        safe(display.get("lastTestName"), "-")?,
        safe(display.get("review"), "-")?,
    );
    if let Some(waves) = display
        .get("waves")
        .and_then(domain::items)
        .filter(|w| !w.is_empty())
    {
        out.push_str("Child waves:\n");
        for row in waves {
            let issue_num = safe(row.get("issue"), "-")?;
            let wave_num = safe(row.get("wave"), "-")?;
            let seat = safe(row.get("seat"), "-")?;
            let st = safe(row.get("state"), "-")?;
            let model = safe(row.get("model"), "-")?;
            out.push_str(&format!(
                "  #{issue_num} wave:{wave_num} {seat} {st} {model}\n"
            ));
        }
    }
    if let Some(wt) = state
        .get("lastRun")
        .and_then(|r| r.get("worktreePath"))
        .filter(|v| **v != Value::Null)
    {
        out.push_str(&format!("Worktree: {}\n", safe(Some(wt), "-")?));
    }
    Ok(out)
}

pub fn format_step(parts: &[&str]) -> String {
    let cleaned = parts
        .iter()
        .map(|part| {
            let no_vt = strip_vt_only(part);
            let filtered: String = no_vt
                .chars()
                .filter(|ch| !matches!(*ch, '\x00'..='\x08' | '\x0b' | '\x0c' | '\x0e'..='\x1f' | '\x7f'))
                .collect();
            filtered
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(STEP_MAX)
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(STEP_DOT);
    cleaned.chars().take(STEP_MAX).collect()
}

pub fn wrap_narration(text: &str, columns: usize) -> Vec<String> {
    let width = if columns > 1 {
        (columns - 1).max(20)
    } else {
        79
    };
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut rest = paragraph;
        while rest.chars().count() > width {
            let byte_at_width = rest
                .char_indices()
                .nth(width)
                .map_or(rest.len(), |(idx, _)| idx);
            let search_end = rest
                .char_indices()
                .nth(width + 1)
                .map_or(rest.len(), |(idx, _)| idx);
            let prefix = &rest[..search_end];
            let cut = match prefix.rfind(' ') {
                Some(space_idx) if prefix[..space_idx].chars().count() >= width / 2 => space_idx,
                _ => byte_at_width,
            };
            lines.push(rest[..cut].to_owned());
            rest = rest[cut..].strip_prefix(' ').unwrap_or(&rest[cut..]);
        }
        lines.push(rest.to_owned());
    }
    lines
}

pub fn format_tokens(value: Option<u64>) -> String {
    let Some(value) = value.filter(|v| *v <= MAX_SAFE_INTEGER) else {
        return "-".to_owned();
    };
    if value >= 1_000_000 {
        format!("{:.1}m", value as f64 / 1_000_000.0)
    } else if value >= 1000 {
        format!("{:.1}k", value as f64 / 1000.0)
    } else {
        value.to_string()
    }
}

pub fn context_tone(used: Option<u64>, max: Option<u64>) -> Option<&'static str> {
    let used = used.filter(|v| *v <= MAX_SAFE_INTEGER)?;
    let max = max.filter(|v| (1..=MAX_SAFE_INTEGER).contains(v))?;
    let percent = ((used as f64 / max as f64) * 100.0).min(100.0);
    if percent >= 95.0 {
        Some("red")
    } else if percent >= 80.0 {
        Some("orange")
    } else if percent >= 50.0 {
        Some("yellow")
    } else {
        Some("green")
    }
}

pub fn format_context_percent(used: Option<u64>, max: Option<u64>) -> String {
    let Some(used) = used.filter(|v| *v <= MAX_SAFE_INTEGER) else {
        return "-".to_owned();
    };
    let Some(max) = max.filter(|v| (1..=MAX_SAFE_INTEGER).contains(v)) else {
        return "-".to_owned();
    };
    let percent = ((used as f64 / max as f64) * 100.0).min(100.0).round() as u64;
    format!("{percent}%")
}

pub fn format_context_bar(used: Option<u64>, max: Option<u64>, color: bool) -> String {
    let Some(tone) = context_tone(used, max) else {
        return format!("[{}]", "-".repeat(BAR_CELLS));
    };
    let used = used.unwrap_or(0) as f64;
    let max = max.unwrap_or(1) as f64;
    let percent = ((used / max) * 100.0).min(100.0);
    let filled = (((percent / 100.0) * BAR_CELLS as f64).round() as usize).min(BAR_CELLS);
    format!(
        "[{}{}]",
        ansi_paint(&"#".repeat(filled), tone, color),
        "-".repeat(BAR_CELLS - filled)
    )
}

pub fn format_tray_context(
    used: Option<u64>,
    max: Option<u64>,
    color: bool,
    detail: &str,
) -> String {
    let tone = context_tone(used, max);
    if detail == "percent" {
        return match tone {
            None => "-".to_owned(),
            Some(tone) => ansi_paint(&format_context_percent(used, max), tone, color),
        };
    }
    let Some(tone) = tone else {
        return format!("- / {}", format_tokens(max));
    };
    format!(
        "{}/{} {} {}",
        format_tokens(used),
        format_tokens(max),
        format_context_bar(used, max, color),
        ansi_paint(&format_context_percent(used, max), tone, color)
    )
}

pub fn format_elapsed(started_at_ms: Option<f64>, now_ms: f64) -> String {
    let Some(started) = started_at_ms.filter(|v| v.is_finite()) else {
        return "-".to_owned();
    };
    if !now_ms.is_finite() {
        return "-".to_owned();
    }
    let seconds = (((now_ms - started) / 1000.0).floor().max(0.0)) as u64;
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h{:02}m", seconds / 3600, (seconds % 3600) / 60)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayFrame {
    pub rule: String,
    pub rail: String,
    pub detail: Option<String>,
    pub prompt: String,
}

pub fn format_tray(
    display: &Value,
    columns: usize,
    color: bool,
    now_ms: f64,
    debug: bool,
) -> TrayFrame {
    let width = if columns > 0 {
        (columns - 1).max(8)
    } else {
        79
    };
    let terminal = if columns > 0 { columns } else { 80 };
    let state_raw = clean_display(display.get("state"), "idle");
    let state = if state_raw == "-" { "idle" } else { &state_raw };
    let phase = match state {
        "idle" => "idle",
        "planning" => "plan",
        "drafting" => "draft",
        "testing" => "test",
        "reviewing" => "review",
        "passed" => "pass",
        "failed" => "fail",
        "published" => "done",
        other => other,
    };
    let issue_label = match display.get("issue").filter(|v| **v != Value::Null) {
        Some(val) => format!("#{}", clean_display(Some(val), "-")),
        None => "local".to_owned(),
    };
    let checklist_raw = clean_display(display.get("checklist"), "");
    let checklist =
        if regex_lite::Regex::new(r"^\d+/\d+$").is_ok_and(|re| re.is_match(&checklist_raw)) {
            format!(" \u{2713}{checklist_raw}")
        } else {
            String::new()
        };
    let where_str = if state == "idle" {
        "idle".to_owned()
    } else {
        format!("{issue_label} {phase}{checklist}")
    };
    let effort = clean_display(display.get("effort"), "-");
    let id = clean_display(display.get("model"), "-");
    let effort_char = if effort == "-" {
        "-".to_owned()
    } else {
        effort.chars().next().unwrap_or('-').to_string()
    };
    let model = format!("{id} {effort_char}");
    let failed = state == "failed";
    let finish_reason = display
        .get("lastFinishReason")
        .filter(|v| **v != Value::Null)
        .map(|v| clean_display(Some(v), ""))
        .filter(|s| !s.is_empty() && s != "-");
    let started_at = display.get("startedAt").and_then(Value::as_number);
    let tail = if failed && finish_reason.is_some() {
        finish_reason.unwrap_or_else(|| "-".to_owned())
    } else if state == "idle" {
        "-".to_owned()
    } else {
        format_elapsed(started_at, now_ms)
    };
    let used = domain::safe_integer(display.get("contextUsed"));
    let max = domain::safe_integer(display.get("contextMax"));
    let detail_level = if terminal >= FULL_COLUMNS {
        "full"
    } else if terminal >= PERCENT_COLUMNS {
        "percent"
    } else {
        "none"
    };
    let context = if detail_level == "none" {
        None
    } else {
        Some(format_tray_context(used, max, color, detail_level))
    };
    let where_color = if failed {
        "red"
    } else if state == "passed" {
        "green"
    } else {
        "white"
    };
    let tail_color = if failed { "red" } else { "white" };
    let mut fields = vec![
        ansi_paint(&where_str, where_color, color),
        ansi_paint(&model, "white", color),
    ];
    if let Some(ctx) = &context {
        fields.push(ctx.clone());
    }
    let debug_star = if debug {
        ansi_paint("*", "label", color)
    } else {
        String::new()
    };
    fields.push(format!(
        "{}{debug_star}",
        ansi_paint(&tail, tail_color, color)
    ));
    let mut rail = fields.join(SEPARATOR);
    let drop_indices = if context.is_some() {
        vec![2_usize, 1_usize]
    } else {
        vec![1_usize]
    };
    for idx in drop_indices {
        if strip_vt_only(&rail).chars().count() <= width {
            break;
        }
        if idx < fields.len() {
            fields.remove(idx);
            rail = fields.join(SEPARATOR);
        }
    }
    let rule = ansi_paint(&RULE_CHAR.repeat(width), "label", color);
    let prompt = ansi_paint("roster> ", "label", color);
    let detail = if debug {
        let thinking = match display.get("thinking") {
            Some(Value::Boolean(true)) => "on",
            Some(Value::Boolean(false)) => "off",
            _ => "-",
        };
        let max_tokens = domain::safe_integer(display.get("maxTokens"))
            .map_or_else(|| "-".to_owned(), |n| n.to_string());
        Some(ansi_paint(
            &format!("thinking {thinking}{SEPARATOR}max_tokens {max_tokens}"),
            "white",
            color,
        ))
    } else {
        None
    };
    TrayFrame {
        rule,
        rail,
        detail,
        prompt,
    }
}

pub fn format_usage(display: &Value, measured: Option<&Value>, now_ms: f64) -> String {
    let get_field = |m_key: &str, d_key: &str| -> Option<&Value> {
        measured
            .and_then(|m| m.get(m_key))
            .filter(|v| **v != Value::Null)
            .or_else(|| display.get(d_key).filter(|v| **v != Value::Null))
    };
    let count_str = |val: Option<&Value>| -> String {
        domain::safe_integer(val).map_or_else(|| "-".to_owned(), |n| n.to_string())
    };
    let effort_raw = clean_display(get_field("effort", "effort"), "-");
    let effort_label = match effort_raw.as_str() {
        "l" => "low",
        "m" => "medium",
        "h" => "high",
        "x" => "max",
        "none" => "none",
        other => other,
    };
    let thinking = match display.get("thinking") {
        Some(Value::Boolean(true)) => "enabled",
        Some(Value::Boolean(false)) => "disabled",
        _ => "-",
    };
    let started_at = display.get("startedAt").and_then(Value::as_number);
    let rows = [
        ("Model", clean_display(get_field("model", "model"), "-")),
        ("Endpoint", clean_display(display.get("host"), "-")),
        (
            "Prompt tokens",
            count_str(get_field("input", "contextUsed")),
        ),
        (
            "Cached prompt tokens",
            count_str(display.get("cachedTokens")),
        ),
        (
            "Completion tokens",
            count_str(get_field("output", "outputTokens")),
        ),
        (
            "Context max",
            count_str(get_field("contextMax", "contextMax")),
        ),
        ("Effort", effort_label.to_owned()),
        (
            "Finish reason",
            clean_display(get_field("finishReason", "lastFinishReason"), "-"),
        ),
        ("Tool calls", count_str(display.get("toolCount"))),
        ("Elapsed", format_elapsed(started_at, now_ms)),
        ("Thinking", thinking.to_owned()),
        ("Max completion tokens", count_str(display.get("maxTokens"))),
    ];
    let mut out = String::new();
    for (label, value) in rows {
        out.push_str(&format!("{label}: {value}\n"));
    }
    out
}

pub fn check_app_identity(cwd: &Path, env: Option<&Value>) -> Result<Value, Error> {
    let env_val = env
        .cloned()
        .unwrap_or_else(crate::ask::environment_value_from_process);
    let present = |key: &str| -> Option<String> {
        env_val
            .get(key)
            .and_then(Value::as_string)
            .and_then(|t| t.to_utf8().ok())
            .filter(|s| !trim(&Text::from(s.as_str()).0).is_empty())
    };
    let app_id = present("GITHUB_APP_ID");
    let key_path = present("GITHUB_APP_PRIVATE_KEY_PATH");
    let env_present = app_id.is_some() && key_path.is_some();
    let mut key_file_exists = false;
    let mut key_warning: Option<&'static str> = None;
    if let Some(key_path) = key_path {
        let resolved = paths::resolve(cwd, Path::new(&key_path))?;
        match fs::metadata(&resolved) {
            Ok(entry) => {
                key_file_exists = entry.is_file();
                if !key_file_exists {
                    key_warning = Some("App private-key path is not a regular file.");
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                key_warning = Some("App private-key file is missing.");
            }
            Err(_) => {
                key_warning = Some(
                    "App private-key file could not be checked; inspect local file permissions.",
                );
            }
        }
    }
    let mut result = Value::object(vec![
        ("envPresent", Value::Boolean(env_present)),
        ("keyFileExists", Value::Boolean(key_file_exists)),
    ]);
    if let Some(warning) = key_warning {
        domain::put(&mut result, "keyWarning", Value::string(warning))?;
    }
    Ok(result)
}

pub fn format_app_identity(result: &Value) -> Result<String, Error> {
    let Some(Value::Boolean(env_present)) = result.get("envPresent") else {
        return Err(Error::type_error(
            "Expected App environment and key-file presence checks",
        ));
    };
    let Some(Value::Boolean(_)) = result.get("keyFileExists") else {
        return Err(Error::type_error(
            "Expected App environment and key-file presence checks",
        ));
    };
    let mut text = if *env_present {
        "App env present\n".to_owned()
    } else {
        "App env missing\nWindows User environment (restart the shell after setx):\n  setx GITHUB_APP_ID \"<app-id>\"\n  setx GITHUB_APP_PRIVATE_KEY_PATH \"<absolute-path-to-existing-pem>\"\nUnix shell environment:\n  export GITHUB_APP_ID=\"<app-id>\"\n  export GITHUB_APP_PRIVATE_KEY_PATH=\"<absolute-path-to-existing-pem>\"\n".to_owned()
    };
    if let Some(warning) = result
        .get("keyWarning")
        .and_then(Value::as_string)
        .and_then(|t| t.to_utf8().ok())
        .filter(|s| !s.is_empty())
    {
        text.push_str(&format!("WARNING: {warning}\n"));
    }
    text.push_str(
        "One GitHub App is used for all worktrees; environment settings do not grant policy.\n",
    );
    Ok(text)
}

#[derive(Debug, Clone)]
pub struct OnboardConfigOptions<'a> {
    pub base_url: &'a str,
    pub model: &'a str,
    pub context_max: u64,
    pub publish: bool,
    pub internet: Option<bool>,
    pub run_test: bool,
    pub reviewer: bool,
    pub turns: u64,
    pub budget: u64,
}

pub fn render_onboard_config(
    base: &Value,
    options: &OnboardConfigOptions<'_>,
) -> Result<Text, Error> {
    let mut cfg = base.clone();
    let _ = domain::remove(&mut cfg, "reviewer");
    let vllm_key_env = cfg
        .get("profiles")
        .and_then(|p| p.get("vllm-local"))
        .and_then(|v| v.get("api_key_env"))
        .cloned()
        .unwrap_or_else(|| Value::string("ROSTER_API_KEY"));
    let mut llm = cfg
        .get("llm")
        .cloned()
        .unwrap_or_else(|| Value::object(Vec::new()));
    domain::put(&mut llm, "profile", Value::string("vllm-local"))?;
    domain::put(&mut llm, "base_url", Value::string(options.base_url))?;
    domain::put(&mut llm, "model", Value::string(options.model))?;
    domain::put(
        &mut llm,
        "context_max",
        Value::Number(options.context_max as f64),
    )?;
    domain::put(&mut llm, "api_key_env", vllm_key_env)?;
    domain::put(&mut llm, "api_key_optional", Value::Boolean(true))?;
    domain::put(&mut llm, "provider", Value::string("vllm"))?;
    domain::put(&mut cfg, "llm", llm)?;

    let mut seat = cfg
        .get("seat")
        .cloned()
        .unwrap_or_else(|| Value::object(Vec::new()));
    domain::put(
        &mut seat,
        "turn_budget",
        Value::Number(options.turns as f64),
    )?;
    domain::put(
        &mut seat,
        "context_chars",
        Value::Number(options.budget as f64),
    )?;
    domain::put(&mut cfg, "seat", seat)?;

    let mut publish = cfg
        .get("publish")
        .cloned()
        .unwrap_or_else(|| Value::object(Vec::new()));
    domain::put(&mut publish, "enabled", Value::Boolean(options.publish))?;
    domain::put(&mut cfg, "publish", publish)?;

    let mut tools = cfg
        .get("tools")
        .cloned()
        .unwrap_or_else(|| Value::object(Vec::new()));
    if let Some(internet) = options.internet {
        domain::put(&mut tools, "internet", Value::Boolean(internet))?;
    }
    domain::put(&mut tools, "run_test", Value::Boolean(options.run_test))?;
    domain::put(&mut cfg, "tools", tools)?;

    let mut review = cfg
        .get("review")
        .cloned()
        .unwrap_or_else(|| Value::object(Vec::new()));
    domain::put(&mut review, "required", Value::Boolean(options.reviewer))?;
    domain::put(&mut cfg, "review", review)?;

    let mut loop_cfg = cfg
        .get("loop")
        .cloned()
        .unwrap_or_else(|| Value::object(Vec::new()));
    domain::put(&mut loop_cfg, "turns", Value::Number(options.turns as f64))?;
    domain::put(&mut cfg, "loop", loop_cfg)?;

    let mut context_cfg = cfg
        .get("context")
        .cloned()
        .unwrap_or_else(|| Value::object(Vec::new()));
    domain::put(
        &mut context_cfg,
        "budget",
        Value::Number(options.budget as f64),
    )?;
    domain::put(&mut cfg, "context", context_cfg)?;

    config::format(&cfg)
}

#[derive(Debug, Clone)]
struct PendingToolEvent {
    name: String,
    target: String,
    count: u64,
}

#[derive(Debug, Clone)]
pub struct EventSink {
    phase: Option<String>,
    finish_reason: Option<String>,
    current: Option<PendingToolEvent>,
    dirty: bool,
    last_tool_at: f64,
    written: Vec<String>,
    streaming: bool,
    coalesce_ms: f64,
    emitted: Vec<Value>,
}

impl EventSink {
    pub fn new(coalesce_ms: f64) -> Self {
        Self {
            phase: None,
            finish_reason: None,
            current: None,
            dirty: false,
            last_tool_at: f64::NEG_INFINITY,
            written: Vec::new(),
            streaming: false,
            coalesce_ms,
            emitted: Vec::new(),
        }
    }

    pub fn phase(&self) -> Option<&str> {
        self.phase.as_deref()
    }

    pub fn take_emitted(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.emitted)
    }

    fn send(&mut self, event: Value) {
        if let Some(kind) = event
            .get("kind")
            .and_then(Value::as_string)
            .and_then(|t| t.to_utf8().ok())
        {
            if ["phase", "tool", "verdict", "wait"].contains(&kind.as_str()) {
                self.streaming = false;
            }
        }
        self.emitted.push(event);
    }

    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        let Some(current) = self.current.clone() else {
            return;
        };
        self.dirty = false;
        self.send(Value::object(vec![
            ("kind", Value::string("tool")),
            ("name", Value::string(&current.name)),
            ("target", Value::string(&current.target)),
            ("count", Value::Number(current.count as f64)),
        ]));
    }

    fn send_phase(&mut self, name: Option<&str>, issue: Option<u64>) {
        let Some(name) = name.filter(|s| !s.is_empty()) else {
            return;
        };
        if self.phase.as_deref() == Some(name) {
            return;
        }
        self.phase = Some(name.to_owned());
        self.flush();
        self.current = None;
        self.send(Value::object(vec![
            ("kind", Value::string("phase")),
            (
                "issue",
                issue.map_or(Value::Null, |n| Value::Number(n as f64)),
            ),
            ("phase", Value::string(name)),
        ]));
    }

    fn record_tool(&mut self, name: &str, target: &str, now_ms: f64) {
        if let Some(cur) = &mut self.current {
            if cur.name == name && cur.target == target {
                cur.count += 1;
            } else {
                self.flush();
                self.current = Some(PendingToolEvent {
                    name: name.to_owned(),
                    target: target.to_owned(),
                    count: 1,
                });
            }
        } else {
            self.flush();
            self.current = Some(PendingToolEvent {
                name: name.to_owned(),
                target: target.to_owned(),
                count: 1,
            });
        }
        self.dirty = true;
        if now_ms - self.last_tool_at >= self.coalesce_ms {
            self.last_tool_at = now_ms;
            self.flush();
        }
    }

    fn record_verdict(&mut self, result: &str, issue: Option<u64>) {
        self.flush();
        let files_val = if self.written.is_empty() {
            Value::Null
        } else {
            Value::Array(self.written.iter().map(|s| Value::string(s)).collect())
        };
        let phase_val = self.phase.as_deref().map_or(Value::Null, Value::string);
        let reason_val = self
            .finish_reason
            .as_deref()
            .map_or(Value::Null, Value::string);
        self.send(Value::object(vec![
            ("kind", Value::string("verdict")),
            (
                "issue",
                issue.map_or(Value::Null, |n| Value::Number(n as f64)),
            ),
            ("phase", phase_val),
            ("verdict", Value::string(result)),
            ("reason", reason_val),
            ("files", files_val),
        ]));
        self.written.clear();
        self.current = None;
        self.phase = None;
    }

    pub fn receive(&mut self, event: &Value, issue: Option<u64>, now_ms: f64) {
        let Some(event_type) = event
            .get("type")
            .and_then(Value::as_string)
            .and_then(|t| t.to_utf8().ok())
        else {
            return;
        };
        let count_val = |v: Option<&Value>| -> Value {
            match v {
                Some(Value::Number(n))
                    if n.is_finite()
                        && *n >= 0.0
                        && n.fract() == 0.0
                        && *n <= MAX_SAFE_INTEGER as f64 =>
                {
                    Value::Number(*n)
                }
                _ => Value::Null,
            }
        };
        match event_type.as_str() {
            "seat-start" => {
                self.written.clear();
                self.current = None;
                let seat_phase = match event
                    .get("seat")
                    .and_then(Value::as_string)
                    .and_then(|t| t.to_utf8().ok())
                    .as_deref()
                {
                    Some("planner") => Some("plan"),
                    Some("coder") => Some("draft"),
                    Some("reviewer") => Some("review"),
                    _ => None,
                };
                self.send_phase(seat_phase, issue);
            }
            "tool" => {
                let name = event
                    .get("name")
                    .and_then(Value::as_string)
                    .and_then(|t| t.to_utf8().ok())
                    .unwrap_or_default();
                let target = event
                    .get("path")
                    .and_then(Value::as_string)
                    .and_then(|t| t.to_utf8().ok())
                    .unwrap_or_default();
                if name == "run_test" {
                    self.send_phase(Some("test"), issue);
                }
                self.record_tool(&name, &target, now_ms);
            }
            "wrote" => {
                if let Some(p) = event
                    .get("path")
                    .and_then(Value::as_string)
                    .and_then(|t| t.to_utf8().ok())
                {
                    if !self.written.contains(&p) {
                        self.written.push(p);
                    }
                }
            }
            "waiting" => {
                self.flush();
                let seconds = domain::safe_integer(event.get("elapsedSeconds")).unwrap_or(0);
                let local = event.get("local") == Some(&Value::Boolean(true));
                self.send(Value::object(vec![
                    ("kind", Value::string("wait")),
                    ("seconds", Value::Number(seconds as f64)),
                    ("local", Value::Boolean(local)),
                ]));
            }
            "completion" | "finish-reason" => {
                self.finish_reason = event
                    .get("reason")
                    .and_then(Value::as_string)
                    .and_then(|t| t.to_utf8().ok());
            }
            "http" => {
                if !domain::is_text(event.get("phase"), "start") {
                    return;
                }
                self.streaming = false;
                let thinking = match event.get("thinking") {
                    Some(Value::Boolean(b)) => Value::Boolean(*b),
                    _ => Value::Null,
                };
                self.emitted.push(Value::object(vec![
                    ("kind", Value::string("usage")),
                    ("thinking", thinking),
                    ("maxTokens", count_val(event.get("maxTokens"))),
                ]));
            }
            "delta" => {
                let Some(text) = event
                    .get("text")
                    .and_then(Value::as_string)
                    .and_then(|t| t.to_utf8().ok())
                    .filter(|s| !s.is_empty())
                else {
                    return;
                };
                if !self.streaming {
                    self.flush();
                }
                let start = !self.streaming;
                self.streaming = true;
                self.send(Value::object(vec![
                    ("kind", Value::string("delta")),
                    ("text", Value::string(&text)),
                    ("start", Value::Boolean(start)),
                ]));
            }
            "seat-measurement" => {
                let finish_reason = event
                    .get("finishReason")
                    .and_then(Value::as_string)
                    .cloned()
                    .map_or(Value::Null, Value::String);
                self.emitted.push(Value::object(vec![
                    ("kind", Value::string("usage")),
                    ("input", count_val(event.get("input"))),
                    ("output", count_val(event.get("output"))),
                    ("contextMax", count_val(event.get("contextMax"))),
                    ("finishReason", finish_reason),
                ]));
            }
            "usage" => {
                self.emitted.push(Value::object(vec![
                    ("kind", Value::string("usage")),
                    ("input", count_val(event.get("input"))),
                    ("output", count_val(event.get("output"))),
                    ("cached", count_val(event.get("cached"))),
                ]));
            }
            "seat-end" => {
                if let Some(verdict) = event
                    .get("verdict")
                    .and_then(Value::as_string)
                    .and_then(|t| t.to_utf8().ok())
                    .filter(|s| !s.is_empty())
                {
                    let v = if verdict == "pass" { "pass" } else { "fail" };
                    self.record_verdict(v, issue);
                } else {
                    self.flush();
                }
            }
            "seat-error" => {
                self.record_verdict("fail", issue);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_commands_help_tray_usage_and_settings_preserve_contracts() -> Result<(), Error> {
        assert_eq!(canonical_command("q"), "quit");
        assert_eq!(canonical_command("undo"), "rewind");
        assert_eq!(canonical_command("qs"), "queue");
        let (completions, _) = complete_command("/qu");
        assert_eq!(completions, vec!["/question", "/queue", "/quit"]);
        let quit_help = format_help("/q").ok_or_else(|| Error::new("TestError", "help"))?;
        assert!(quit_help.contains("Aliases: /q; bare exit"));
        let map_help = format_help("/map").ok_or_else(|| Error::new("TestError", "map help"))?;
        assert!(map_help.contains("Write a filename-only repo map"));
        assert!(format_help("unknown-command").is_none());
        assert!(format_help("//").is_none());
        assert!(format_help("//run").is_none());
        assert_eq!(
            wrap_narration("hello world 12345678 next", 21),
            vec!["hello world 12345678", "next"]
        );
        assert!(format_context_readout(None, Some(MAX_SAFE_INTEGER + 1), None).is_err());

        let eval = parse_shell_evaluation_args(
            "roster-42-coder reject 3 n --minutes 10 --note \"Missed edge case\"",
        )?;
        assert_eq!(eval.values[1], "reject");
        assert_eq!(eval.minutes, 10);
        assert_eq!(eval.comment, "Missed edge case");
        assert!(parse_shell_evaluation_args("roster-42-coder reject 3 n --minutes 10").is_err());

        let setting = config_setting("effort", "h", Some(&Value::object(Vec::new())), None)?;
        assert_eq!(setting.field, "effort");
        assert!(
            config_setting(
                "effort",
                "https://bad.example",
                Some(&Value::object(Vec::new())),
                None
            )
            .is_err()
        );

        assert!(safe_history_line(
            "/status 42",
            Some(&Value::object(Vec::new())),
            false
        ));
        assert!(!safe_history_line(
            "/vault set TOKEN",
            Some(&Value::object(Vec::new())),
            false
        ));
        assert!(!safe_history_line(
            "export TOKEN=secret",
            Some(&Value::object(Vec::new())),
            false
        ));

        let display = Value::object(vec![
            ("issue", Value::Number(42.0)),
            ("state", Value::string("drafting")),
            ("model", Value::string("qwen")),
            ("effort", Value::string("h")),
            ("contextUsed", Value::Number(12_000.0)),
            ("contextMax", Value::Number(32_000.0)),
            ("startedAt", Value::Number(1000.0)),
        ]);
        let tray = format_tray(&display, 80, false, 66_000.0, true);
        assert!(tray.rail.contains("#42 draft"));
        assert!(tray.rail.contains("qwen h"));
        assert!(tray.rail.contains("12.0k/32.0k"));
        assert!(tray.rail.contains("1m05s*"));
        assert!(tray.detail.is_some());

        let usage = format_usage(&display, None, 66_000.0);
        assert!(usage.contains("Model: qwen\n"));
        assert!(usage.contains("Prompt tokens: 12000\n"));
        assert!(usage.contains("Effort: high\n"));
        assert!(usage.contains("Elapsed: 1m05s\n"));

        let app_missing = check_app_identity(Path::new("."), Some(&Value::object(Vec::new())))?;
        assert_eq!(app_missing.get("envPresent"), Some(&Value::Boolean(false)));
        let formatted_app = format_app_identity(&app_missing)?;
        assert!(formatted_app.contains("App env missing"));

        let mut sink = EventSink::new(200.0);
        sink.receive(
            &Value::object(vec![
                ("type", Value::string("seat-start")),
                ("seat", Value::string("coder")),
            ]),
            Some(42),
            1000.0,
        );
        assert_eq!(sink.phase(), Some("draft"));
        sink.receive(
            &Value::object(vec![
                ("type", Value::string("tool")),
                ("name", Value::string("read_file")),
                ("path", Value::string("README.md")),
            ]),
            Some(42),
            1000.0,
        );
        sink.receive(
            &Value::object(vec![
                ("type", Value::string("wrote")),
                ("path", Value::string("README.md")),
            ]),
            Some(42),
            1050.0,
        );
        sink.receive(
            &Value::object(vec![
                ("type", Value::string("seat-end")),
                ("verdict", Value::string("pass")),
            ]),
            Some(42),
            1100.0,
        );
        let events = sink.take_emitted();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].get("kind"), Some(&Value::string("phase")));
        assert_eq!(events[1].get("kind"), Some(&Value::string("tool")));
        assert_eq!(events[2].get("kind"), Some(&Value::string("verdict")));
        Ok(())
    }
}
