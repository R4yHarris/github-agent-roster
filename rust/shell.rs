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

fn run_banner_git(cwd: &Path, args: &[&str]) -> Result<String, Error> {
    let git_exe = std::env::var_os("ROSTER_GIT")
        .or_else(|| std::env::var_os("ROSTER_TEST_GIT"))
        .map_or_else(|| PathBuf::from("git"), PathBuf::from);
    let mut cmd = StdCommand::new(git_exe);
    cmd.current_dir(cwd).env_clear().args(args);
    for key in [
        "PATH",
        "PATHEXT",
        "SystemRoot",
        "SYSTEMROOT",
        "WINDIR",
        "TEMP",
        "TMP",
        "HOME",
        "USERPROFILE",
    ] {
        if let Some(val) = std::env::var_os(key) {
            cmd.env(key, val);
        }
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    let out = cmd
        .output()
        .map_err(|error| Error::new("Error", error.to_string()))?;
    if !out.status.success() {
        return Err(Error::new(
            "Error",
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        ));
    }
    let text =
        String::from_utf8(out.stdout).map_err(|error| Error::new("Error", error.to_string()))?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(Error::new("Error", "git returned empty output"));
    }
    Ok(trimmed.to_owned())
}

pub fn collect_banner_facts(
    cwd: &Path,
    installation: &Path,
    environment: &Value,
    config_val: &Value,
) -> Value {
    let mut warnings = Vec::new();
    let version = match run_banner_git(installation, &["describe", "--tags", "--always"]) {
        Ok(v) => v,
        Err(_) => env!("CARGO_PKG_VERSION").to_owned(),
    };
    let node_ver = {
        let node_exe = std::env::var_os("ROSTER_NODE20")
            .or_else(|| std::env::var_os("ROSTER_TEST_NODE"))
            .map_or_else(|| PathBuf::from("node"), PathBuf::from);
        StdCommand::new(node_exe)
            .arg("-v")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| {
                s.trim()
                    .trim_start_matches('v')
                    .split('.')
                    .next()
                    .unwrap_or("-")
                    .to_owned()
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "rust".to_owned())
    };
    let sh = shell_name(Some(environment));
    let cwd_str = cwd.display().to_string();
    let branch = paths::resolve_project_root(cwd)
        .ok()
        .and_then(|root| run_banner_git(&root, &["rev-parse", "--abbrev-ref", "HEAD"]).ok())
        .unwrap_or_else(|| "-".to_owned());
    let llm = config_val.get("llm");
    let base_url = llm
        .and_then(|l| l.get("base_url"))
        .and_then(Value::as_string)
        .and_then(|t| t.to_utf8().ok())
        .filter(|s| !s.trim().is_empty());
    let model = llm
        .and_then(|l| l.get("model"))
        .and_then(Value::as_string)
        .and_then(|t| t.to_utf8().ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "-".to_owned());
    let host = base_url
        .as_deref()
        .and_then(|u| url::Url::parse(u).ok())
        .and_then(|u| {
            let h = u.host_str()?.to_owned();
            Some(match u.port() {
                Some(p) => format!("{h}:{p}"),
                None => h,
            })
        })
        .unwrap_or_else(|| "-".to_owned());
    let endpoint = if base_url.is_some() {
        "configured"
    } else {
        "-"
    };
    let configured_contracts = environment
        .get("GITHUB_AGENT_CONTRACTS")
        .and_then(Value::as_string)
        .and_then(|t| t.to_utf8().ok())
        .map(PathBuf::from);
    let contracts =
        match paths::resolve_contracts_path(installation, cwd, configured_contracts.as_deref()) {
            Ok(contracts_dir) => {
                match run_banner_git(&contracts_dir, &["describe", "--tags", "--always"]) {
                    Ok(desc) => desc,
                    Err(err) => {
                        warnings.push(Value::string(&format!(
                            "contracts check failed: {}",
                            err.message
                        )));
                        "-".to_owned()
                    }
                }
            }
            Err(err) => {
                warnings.push(Value::string(&format!(
                    "contracts check failed: {}",
                    err.message
                )));
                "-".to_owned()
            }
        };
    Value::object(vec![
        ("version", Value::string(&version)),
        ("node", Value::string(&node_ver)),
        ("shell", Value::string(&sh)),
        ("cwd", Value::string(&cwd_str)),
        ("branch", Value::string(&branch)),
        ("endpoint", Value::string(endpoint)),
        ("model", Value::string(&model)),
        ("host", Value::string(&host)),
        ("contracts", Value::string(&contracts)),
        ("update", Value::string("skipped")),
        ("warnings", Value::Array(warnings)),
    ])
}

pub fn queue_lines(asks: &[String], width: usize, color: bool) -> Vec<String> {
    if asks.is_empty() {
        return Vec::new();
    }
    let header = ansi_paint(
        "/queue list  \u{00b7}  /queue drop N  \u{00b7}  /queue clear",
        "label",
        color,
    );
    let mut rows = vec![header];
    for (idx, ask) in asks.iter().take(8).enumerate() {
        let collapsed = ask.split_whitespace().collect::<Vec<_>>().join(" ");
        let line = format!("{}. {collapsed}", idx + 1);
        let truncated = if line.chars().count() > width {
            let take = width.saturating_sub(1).max(1);
            format!("{}\u{2026}", line.chars().take(take).collect::<String>())
        } else {
            line
        };
        rows.push(ansi_paint(&truncated, "white", color));
    }
    rows
}

pub fn render_tray_block(
    display: &Value,
    ask_queue: &[String],
    statusbar: bool,
    debug: bool,
    columns: usize,
    color: bool,
    now_ms: f64,
) -> (String, String, usize) {
    let frame = format_tray(display, columns, color, now_ms, debug);
    let queued = queue_lines(ask_queue, columns, color);
    let children = if statusbar {
        display
            .get("waves")
            .and_then(domain::items)
            .map(|items| {
                items
                    .iter()
                    .map(|row| {
                        let issue = domain::safe_integer(row.get("issue")).unwrap_or(0);
                        let wave = domain::safe_integer(row.get("wave")).unwrap_or(1);
                        let seat = clean_display(row.get("seat"), "-");
                        let state = clean_display(row.get("state"), "-");
                        let model = clean_display(row.get("model"), "-");
                        let line = format!("#{issue} wave:{wave} {seat} {state} {model}");
                        line.chars()
                            .take(columns.saturating_sub(1).max(8))
                            .collect::<String>()
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut block = String::new();
    if !queued.is_empty() {
        block.push_str(&queued.join("\n"));
        block.push('\n');
    }
    let bar_lines = if statusbar {
        let detail_count = usize::from(frame.detail.is_some());
        3 + detail_count + queued.len() + children.len()
    } else {
        queued.len()
    };
    if statusbar {
        block.push_str(&frame.rule);
        block.push('\n');
        block.push_str(&frame.rail);
        block.push('\n');
        for child in &children {
            block.push_str(child);
            block.push('\n');
        }
        block.push_str(&frame.rule);
        block.push('\n');
        if let Some(detail) = &frame.detail {
            block.push_str(detail);
            block.push('\n');
        }
    }
    (block, frame.prompt, bar_lines)
}

pub async fn set_config_value(
    installation: &Path,
    cwd: &Path,
    field: &str,
    value: &str,
    token: Option<&crate::cancel::CancellationToken>,
) -> Result<Value, Error> {
    let root = config::resolve_config_root(installation, cwd)?;
    let previous = settings::read_private_file(&root, settings::Name::Config)?;
    let mut cfg = match &previous {
        Some(src) => config::parse(src)?.as_value().clone(),
        None => config::load(installation, cwd, &Value::object(Vec::new()))?
            .config
            .as_value()
            .clone(),
    };
    match field {
        "model" => {
            let mut llm = cfg
                .get("llm")
                .cloned()
                .unwrap_or_else(|| Value::object(Vec::new()));
            domain::put(&mut llm, "model", Value::string(value))?;
            domain::put(&mut cfg, "llm", llm)?;
        }
        "effort" => {
            let mut llm = cfg
                .get("llm")
                .cloned()
                .unwrap_or_else(|| Value::object(Vec::new()));
            domain::put(&mut llm, "effort", Value::string(value))?;
            domain::put(&mut llm, "effort_override", Value::string(value))?;
            domain::put(&mut cfg, "llm", llm)?;
        }
        "context_budget" => {
            let budget = value
                .parse::<u64>()
                .ok()
                .filter(|n| (1..=MAX_SAFE_INTEGER).contains(n))
                .ok_or_else(|| {
                    Error::type_error("Config context budget must be a positive safe integer.")
                })?;
            let mut seat = cfg
                .get("seat")
                .cloned()
                .unwrap_or_else(|| Value::object(Vec::new()));
            domain::put(&mut seat, "context_chars", Value::Number(budget as f64))?;
            domain::put(&mut cfg, "seat", seat)?;
            if let Some(mut ctx) = cfg.get("context").cloned() {
                domain::put(&mut ctx, "budget", Value::Number(budget as f64))?;
                domain::put(&mut cfg, "context", ctx)?;
            }
        }
        _ => {
            return Err(Error::type_error(
                "Unsupported private configuration setting",
            ));
        }
    }
    let formatted = config::format(&cfg)?;
    let validated = config::parse(&formatted)?.as_value().clone();
    settings::ensure_private_files_ignored(&root, &[settings::Name::Config], token).await?;
    settings::write_private_documents(
        &root,
        &[settings::Document {
            name: settings::Name::Config,
            source: formatted,
            expected_source: Some(previous),
        }],
    )?;
    Ok(validated)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellAction {
    Continue(String),
    ClearScreen,
    Redraw,
    PromptSecret(String),
    Exit,
}

pub struct ShellSession {
    pub cwd: PathBuf,
    pub installation: PathBuf,
    pub environment: Value,
    pub config: Value,
    pub display: Value,
    pub statusbar: bool,
    pub debug: crate::diagnostics::DebugLog,
    pub history: ShellHistory,
    pub ask_queue: Vec<String>,
    pub queued_input: Vec<String>,
    pub fleet_profile_id: Option<String>,
    pub route_next: bool,
    pub pending_secret: Option<String>,
    pub last_measured_seat: Option<Value>,
    pub last_worktree: Option<PathBuf>,
    pub last_issue: Option<u64>,
    pub last_task: Option<String>,
    pub last_finish_reason: Option<String>,
    pub issue_cache: std::collections::BTreeMap<u64, Value>,
}

impl ShellSession {
    pub fn new(
        cwd: &Path,
        installation: &Path,
        environment: &Value,
        debug_enabled: bool,
    ) -> Result<Self, Error> {
        let cfg = match config::load(installation, cwd, environment) {
            Ok(loaded) => loaded.config.as_value().clone(),
            Err(_) => config::defaults()?.as_value().clone(),
        };
        let project_root = paths::resolve_project_root(cwd).unwrap_or_else(|_| cwd.to_path_buf());
        let branch = run_banner_git(&project_root, &["rev-parse", "--abbrev-ref", "HEAD"])
            .unwrap_or_else(|_| "-".to_owned());
        let llm = cfg.get("llm");
        let model = llm
            .and_then(|l| l.get("model"))
            .and_then(Value::as_string)
            .and_then(|t| t.to_utf8().ok())
            .unwrap_or_default();
        let base_url = llm
            .and_then(|l| l.get("base_url"))
            .and_then(Value::as_string)
            .and_then(|t| t.to_utf8().ok())
            .unwrap_or_default();
        let host = if base_url.is_empty() {
            String::new()
        } else {
            url::Url::parse(&base_url)
                .ok()
                .and_then(|u| {
                    let h = u.host_str()?.to_owned();
                    Some(match u.port() {
                        Some(p) => format!("{h}:{p}"),
                        None => h,
                    })
                })
                .unwrap_or_default()
        };
        let effort = llm
            .and_then(|l| l.get("effort"))
            .and_then(Value::as_string)
            .and_then(|t| t.to_utf8().ok())
            .unwrap_or_else(|| "h".to_owned());
        let context_max = llm
            .and_then(|l| l.get("context_max"))
            .and_then(|v| domain::safe_integer(Some(v)))
            .map_or(Value::Null, |n| Value::Number(n as f64));
        let seat_id = cfg
            .get("seat")
            .and_then(|s| s.get("id"))
            .and_then(Value::as_string)
            .and_then(|t| t.to_utf8().ok())
            .unwrap_or_else(|| "coder".to_owned());
        let display = Value::object(vec![
            ("issue", Value::Null),
            ("branch", Value::string(&branch)),
            ("seat", Value::string(&seat_id)),
            ("state", Value::string("idle")),
            ("busy", Value::Boolean(false)),
            ("model", Value::string(&model)),
            ("host", Value::string(&host)),
            ("effort", Value::string(&effort)),
            ("contextUsed", Value::Null),
            ("contextMax", context_max),
            ("startedAt", Value::Null),
            ("lastFinishReason", Value::Null),
            ("lastTestName", Value::Null),
            ("review", Value::Null),
        ]);
        let mut history = ShellHistory::new(&project_root, Some(environment));
        let _ = history.load();
        let debug = crate::diagnostics::DebugLog::new(
            Some(environment),
            if debug_enabled { Some(true) } else { None },
            None,
        )?;
        Ok(Self {
            cwd: cwd.to_path_buf(),
            installation: installation.to_path_buf(),
            environment: environment.clone(),
            config: cfg,
            display,
            statusbar: true,
            debug,
            history,
            ask_queue: Vec::new(),
            queued_input: Vec::new(),
            fleet_profile_id: None,
            route_next: false,
            pending_secret: None,
            last_measured_seat: None,
            last_worktree: None,
            last_issue: None,
            last_task: None,
            last_finish_reason: None,
            issue_cache: std::collections::BTreeMap::new(),
        })
    }

    fn current_root(&self) -> PathBuf {
        paths::resolve_project_root(&self.cwd).unwrap_or_else(|_| self.cwd.clone())
    }

    fn api_key_env(&self) -> Option<String> {
        self.config
            .get("llm")
            .and_then(|l| l.get("api_key_env"))
            .and_then(Value::as_string)
            .and_then(|t| t.to_utf8().ok())
    }

    fn host_str(&self) -> String {
        let base_url = self
            .config
            .get("llm")
            .and_then(|l| l.get("base_url"))
            .and_then(Value::as_string)
            .and_then(|t| t.to_utf8().ok())
            .unwrap_or_default();
        if base_url.is_empty() {
            return "-".to_owned();
        }
        url::Url::parse(&base_url)
            .ok()
            .and_then(|u| {
                let h = u.host_str()?.to_owned();
                Some(match u.port() {
                    Some(p) => format!("{h}:{p}"),
                    None => h,
                })
            })
            .unwrap_or_else(|| "-".to_owned())
    }

    fn sync_model_display(&mut self) {
        let llm = self.config.get("llm");
        let model = llm
            .and_then(|l| l.get("model"))
            .and_then(Value::as_string)
            .and_then(|t| t.to_utf8().ok())
            .unwrap_or_default();
        let effort = llm
            .and_then(|l| l.get("effort"))
            .and_then(Value::as_string)
            .and_then(|t| t.to_utf8().ok())
            .unwrap_or_else(|| "h".to_owned());
        let ctx_max = llm
            .and_then(|l| l.get("context_max"))
            .and_then(|v| domain::safe_integer(Some(v)))
            .map_or(Value::Null, |n| Value::Number(n as f64));
        let host = self.host_str();
        let _ = domain::put(&mut self.display, "model", Value::string(&model));
        let _ = domain::put(&mut self.display, "host", Value::string(&host));
        let _ = domain::put(&mut self.display, "effort", Value::string(&effort));
        let _ = domain::put(&mut self.display, "contextMax", ctx_max);
        let _ = domain::put(&mut self.display, "contextUsed", Value::Null);
    }

    fn redact_text(&self, text: &str) -> Result<String, Error> {
        let mut cfg = redaction::Config {
            environment: self.environment.clone(),
            ..redaction::Config::default()
        };
        if let Some(key) = self.api_key_env() {
            cfg.api_key_env = Text::from(key.as_str());
        }
        cfg.validate()?;
        redaction::evidence(&Text::from(text), &cfg).to_utf8()
    }

    pub async fn dispatch_line(&mut self, line: &str) -> Result<ShellAction, Error> {
        if let Some(secret_name) = self.pending_secret.take() {
            let value = line.strip_suffix('\r').unwrap_or(line);
            let vault = crate::vault::FileVault::from_environment(&self.environment)?;
            vault.set(&secret_name, value)?;
            return Ok(ShellAction::Continue(format!(
                "Stored secret {secret_name}.\n"
            )));
        }
        let text = line.trim();
        if text.is_empty() {
            if !self.ask_queue.is_empty() {
                let next = self.ask_queue.remove(0);
                let out = self.run_ask_text(&next).await?;
                return Ok(ShellAction::Continue(format!(
                    "Starting queued Ask: {next}\n{out}"
                )));
            }
            return Ok(ShellAction::Continue(String::new()));
        }
        let _ = self.history.record(text, false);
        if text == "exit" {
            return Ok(ShellAction::Exit);
        }
        if text == "/" {
            let help = format_help("").unwrap_or_default();
            return Ok(ShellAction::Continue(help));
        }
        if !text.starts_with('/') {
            if crate::ask::is_repository_question_ask(text)? {
                let ans = self.run_question_text(text).await?;
                return Ok(ShellAction::Continue(ans));
            }
            let out = self.run_ask_text(text).await?;
            return Ok(ShellAction::Continue(out));
        }
        let unknown = "Unknown command. /help lists commands.\n";
        let re = regex_lite::Regex::new(r"(?s)^/([a-z]+)(?:\s+(.*))?$")
            .map_err(|error| Error::new("Error", error.to_string()))?;
        let Some(caps) = re.captures(text) else {
            return Err(Error::new("Error", unknown.trim()));
        };
        let input_cmd = caps.get(1).map_or("", |m| m.as_str());
        let raw_args = caps.get(2).map_or("", |m| m.as_str()).trim();
        let command = canonical_command(input_cmd);
        match command {
            "quit" => {
                if !raw_args.is_empty() {
                    return Err(Error::type_error("Use /quit."));
                }
                Ok(ShellAction::Exit)
            }
            "help" => {
                if raw_args.chars().any(char::is_whitespace) {
                    return Err(Error::type_error("Use /help [GROUP|COMMAND]."));
                }
                match format_help(raw_args) {
                    Some(h) => Ok(ShellAction::Continue(h)),
                    None => Err(Error::new("Error", unknown.trim())),
                }
            }
            "clear" => {
                if !raw_args.is_empty() {
                    return Err(Error::type_error("Use /clear."));
                }
                Ok(ShellAction::ClearScreen)
            }
            "redraw" => {
                if !raw_args.is_empty() {
                    return Err(Error::type_error("Use /redraw."));
                }
                Ok(ShellAction::Redraw)
            }
            "statusbar" => {
                if !matches!(raw_args, "on" | "off") {
                    return Err(Error::type_error("Use /statusbar on or /statusbar off."));
                }
                self.statusbar = raw_args == "on";
                Ok(ShellAction::Continue(format!("Status bars {raw_args}.\n")))
            }
            "debug" => {
                if !matches!(raw_args, "on" | "off" | "status") {
                    return Err(Error::type_error(
                        "Use /debug on, /debug off, or /debug status.",
                    ));
                }
                if raw_args != "status" {
                    self.debug.set_enabled(raw_args == "on")?;
                }
                let state = if self.debug.enabled() { "on" } else { "off" };
                Ok(ShellAction::Continue(format!("Debug logging {state}.\n")))
            }
            "history" => {
                if !raw_args.is_empty() {
                    return Err(Error::type_error("Use /history."));
                }
                let safe = self
                    .history
                    .lines()
                    .iter()
                    .filter(|l| safe_history_line(l, Some(&self.environment), false))
                    .cloned()
                    .collect::<Vec<_>>();
                let start = safe.len().saturating_sub(20);
                let tail = &safe[start..];
                if tail.is_empty() {
                    Ok(ShellAction::Continue("No stored commands.\n".to_owned()))
                } else {
                    Ok(ShellAction::Continue(format!("{}\n", tail.join("\n"))))
                }
            }
            "context" => {
                if !raw_args.is_empty() {
                    return Err(Error::type_error("Use /context."));
                }
                let budget = self
                    .config
                    .get("seat")
                    .and_then(|s| s.get("context_chars"))
                    .and_then(|v| domain::safe_integer(Some(v)));
                let out = format_context_readout(
                    self.last_measured_seat.as_ref(),
                    budget,
                    Some(&self.environment),
                )?;
                Ok(ShellAction::Continue(self.redact_text(&out)?))
            }
            "usage" => {
                if !raw_args.is_empty() {
                    return Err(Error::type_error("Use /usage."));
                }
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
                let out = format_usage(&self.display, self.last_measured_seat.as_ref(), now_ms);
                Ok(ShellAction::Continue(self.redact_text(&out)?))
            }
            "status" => {
                let fields = if raw_args.is_empty() {
                    Vec::new()
                } else {
                    raw_args.split_whitespace().collect::<Vec<_>>()
                };
                let offline_count = fields.iter().filter(|&&f| f == "--offline").count();
                let numbers = fields
                    .iter()
                    .copied()
                    .filter(|&f| f != "--offline")
                    .collect::<Vec<_>>();
                if offline_count > 1
                    || numbers.len() > 1
                    || numbers
                        .first()
                        .is_some_and(|n| crate::start::parse_issue_number(n).is_err())
                {
                    return Err(Error::type_error("Use /status [N] [--offline]."));
                }
                if numbers.is_empty() {
                    let state_val = Value::object(vec![
                        ("display", self.display.clone()),
                        (
                            "lastRun",
                            match &self.last_worktree {
                                Some(wt) => Value::object(vec![(
                                    "worktreePath",
                                    Value::string(&wt.display().to_string()),
                                )]),
                                None => Value::Null,
                            },
                        ),
                    ]);
                    let out = format_session_status(&state_val, Some(&self.environment))?;
                    return Ok(ShellAction::Continue(out));
                }
                let number = crate::start::parse_issue_number(numbers[0])?;
                let offline = offline_count > 0;
                let source = crate::cancel::CancellationSource::default();
                let token = source.token();
                let opts = crate::status::Options {
                    issue: number,
                    offline,
                    json: false,
                };
                let root = self.current_root();
                let snap = if offline {
                    crate::status::observe(&root, &self.environment, &opts, &token).await?
                } else {
                    let mut transport =
                        crate::llm::http::HttpTransport::new().map_err(|failure| *failure.error)?;
                    crate::status::read(
                        &mut transport,
                        &self.installation,
                        &root,
                        &self.environment,
                        &opts,
                        &token,
                    )
                    .await?
                };
                Ok(ShellAction::Continue(snap.format(false)?))
            }
            "model" => {
                if raw_args.is_empty() {
                    let model = self
                        .config
                        .get("llm")
                        .and_then(|l| l.get("model"))
                        .and_then(Value::as_string)
                        .and_then(|t| t.to_utf8().ok())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "(unset)".to_owned());
                    let out = format!("Model: {model}\nHost: {}\n", self.host_str());
                    return Ok(ShellAction::Continue(self.redact_text(&out)?));
                }
                let parts = raw_args.split_whitespace().collect::<Vec<_>>();
                if parts.is_empty()
                    || parts.len() > 2
                    || parts[0].starts_with('-')
                    || (parts.len() == 2 && parts[1] != "--save")
                {
                    return Err(Error::type_error("Use /model [ID|clear] [--save]."));
                }
                let save = parts.len() == 2;
                let selected = if parts[0] == "clear" {
                    String::new()
                } else {
                    let resolved = crate::run::resolve_publish_model(
                        None,
                        Some(&Value::object(vec![("AI_MODEL", Value::string(parts[0]))])),
                        None,
                        false,
                    )?;
                    if resolved == "builtin-stub" {
                        return Err(Error::type_error(
                            "Use an actual served model ID, not builtin-stub.",
                        ));
                    }
                    if self.redact_text(&resolved)? != resolved {
                        return Err(Error::type_error("Use a public model ID, not a secret."));
                    }
                    resolved
                };
                if save {
                    let source = crate::cancel::CancellationSource::default();
                    set_config_value(
                        &self.installation,
                        &self.cwd,
                        "model",
                        &selected,
                        Some(&source.token()),
                    )
                    .await?;
                }
                let mut llm = self
                    .config
                    .get("llm")
                    .cloned()
                    .unwrap_or_else(|| Value::object(Vec::new()));
                domain::put(&mut llm, "model", Value::string(&selected))?;
                domain::put(&mut self.config, "llm", llm)?;
                self.route_next = selected.is_empty();
                self.sync_model_display();
                let label = if selected.is_empty() {
                    "(unset)"
                } else {
                    &selected
                };
                let out = format!("Model: {label}\nHost: {}\n", self.host_str());
                Ok(ShellAction::Continue(self.redact_text(&out)?))
            }
            "effort" => {
                if raw_args.is_empty() || raw_args == "status" {
                    let eff = self
                        .config
                        .get("llm")
                        .and_then(|l| l.get("effort"))
                        .and_then(Value::as_string)
                        .and_then(|t| t.to_utf8().ok())
                        .unwrap_or_else(|| "h".to_owned());
                    return Ok(ShellAction::Continue(format!("Effort: {eff}\n")));
                }
                if !matches!(raw_args, "l" | "m" | "h" | "x" | "none") {
                    return Err(Error::type_error("Use /effort l|m|h|x|none|status."));
                }
                let mut llm = self
                    .config
                    .get("llm")
                    .cloned()
                    .unwrap_or_else(|| Value::object(Vec::new()));
                domain::put(&mut llm, "effort", Value::string(raw_args))?;
                domain::put(&mut llm, "effort_override", Value::string(raw_args))?;
                domain::put(&mut self.config, "llm", llm)?;
                self.sync_model_display();
                Ok(ShellAction::Continue(format!("Effort: {raw_args}\n")))
            }
            "provider" => {
                if !raw_args.is_empty() {
                    return Err(Error::type_error("Use /provider."));
                }
                let prof = self.fleet_profile_id.clone().unwrap_or_else(|| {
                    self.config
                        .get("llm")
                        .and_then(|l| l.get("profile"))
                        .and_then(Value::as_string)
                        .and_then(|t| t.to_utf8().ok())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "(custom)".to_owned())
                });
                let out = format!("Profile: {prof}\nHost: {}\n", self.host_str());
                Ok(ShellAction::Continue(self.redact_text(&out)?))
            }
            "fleet" => {
                let words = arguments::split(&Text::from(raw_args), crate::fleet::FLEET_CLI_USAGE)?
                    .into_iter()
                    .map(|w| w.to_utf8())
                    .collect::<Result<Vec<_>, _>>()?;
                let sub = words.first().map_or("list", String::as_str);
                let root = self.current_root();
                if sub == "list" {
                    if words.len() > 1 {
                        return Err(Error::type_error("Use /fleet or /fleet list."));
                    }
                    let loaded = crate::fleet::load(&root)?;
                    if loaded.profiles().is_empty() {
                        return Ok(ShellAction::Continue(
                            "No fleet profiles configured.\n".to_owned(),
                        ));
                    }
                    let mut out = String::new();
                    for prof in loaded.profiles() {
                        let pv = prof.as_value();
                        let id = clean_display(pv.get("id"), "-");
                        let base_url = clean_display(pv.get("base_url"), "");
                        let host = url::Url::parse(&base_url)
                            .ok()
                            .and_then(|u| {
                                let h = u.host_str()?.to_owned();
                                Some(match u.port() {
                                    Some(p) => format!("{h}:{p}"),
                                    None => h,
                                })
                            })
                            .unwrap_or_else(|| "-".to_owned());
                        let model = clean_display(pv.get("model"), "-");
                        let ctx = domain::safe_integer(pv.get("context_max"))
                            .filter(|&n| n > 0)
                            .map_or_else(|| "-".to_owned(), |n| n.to_string());
                        out.push_str(&format!("{id} | {host} | {model} | ctx {ctx}\n"));
                    }
                    return Ok(ShellAction::Continue(self.redact_text(&out)?));
                }
                if sub == "use" {
                    if words.len() != 2 {
                        return Err(Error::type_error("Use /fleet use ID."));
                    }
                    let loaded = crate::fleet::load(&root)?;
                    let prof = crate::fleet::get_profile(&loaded, &Value::string(&words[1]))?;
                    let next_cfg = crate::fleet::with_profile(&self.config, prof.as_value())?;
                    let id = clean_display(prof.as_value().get("id"), &words[1]);
                    let model = clean_display(prof.as_value().get("model"), "-");
                    self.config = next_cfg;
                    self.fleet_profile_id = Some(id.clone());
                    self.route_next = false;
                    self.sync_model_display();
                    let out = format!(
                        "Session profile: {id}\nModel: {model}\nHost: {}\n",
                        self.host_str()
                    );
                    return Ok(ShellAction::Continue(self.redact_text(&out)?));
                }
                let parsed_cmd = crate::fleet::parse_fleet_cli_args(&words)?;
                let mut transport =
                    crate::llm::http::HttpTransport::new().map_err(|failure| *failure.error)?;
                let source = crate::cancel::CancellationSource::default();
                let out = crate::fleet::run_fleet_cli(
                    &parsed_cmd,
                    &root,
                    &self.installation,
                    &self.environment,
                    true,
                    &mut transport,
                    Some(&source.token()),
                )
                .await?;
                Ok(ShellAction::Continue(self.redact_text(&out)?))
            }
            "config" => {
                let words = arguments::split(
                    &Text::from(raw_args),
                    "Use /config, /config path, or /config set KEY VALUE.",
                )?
                .into_iter()
                .map(|w| w.to_utf8())
                .collect::<Result<Vec<_>, _>>()?;
                if words.is_empty() {
                    let out =
                        public_config(&self.installation, &self.cwd, Some(&self.environment))?;
                    return Ok(ShellAction::Continue(out));
                }
                if words.len() == 1 && words[0] == "path" {
                    let path = private_config_path(&self.installation, &self.cwd)?;
                    return Ok(ShellAction::Continue(format!("{}\n", path.display())));
                }
                if words.len() != 3 || words[0] != "set" {
                    return Err(Error::type_error(
                        "Use /config, /config path, or /config set KEY VALUE.",
                    ));
                }
                let api_key_env = self.api_key_env();
                let setting = config_setting(
                    &words[1],
                    &words[2],
                    Some(&self.environment),
                    api_key_env.as_deref(),
                )?;
                match setting.field {
                    "statusbar" => {
                        self.statusbar = setting.value == ConfigSettingValue::Boolean(true);
                    }
                    "debug" => {
                        self.debug
                            .set_enabled(setting.value == ConfigSettingValue::Boolean(true))?;
                    }
                    "effort" | "context_budget" => {
                        let ConfigSettingValue::Text(val_str) = setting.value else {
                            return Err(Error::type_error("Config setting must be text"));
                        };
                        let source = crate::cancel::CancellationSource::default();
                        let saved = set_config_value(
                            &self.installation,
                            &self.cwd,
                            setting.field,
                            &val_str,
                            Some(&source.token()),
                        )
                        .await?;
                        self.config = saved;
                        self.sync_model_display();
                    }
                    _ => {}
                }
                let suffix = if matches!(setting.field, "statusbar" | "debug") {
                    " for this process"
                } else {
                    ""
                };
                Ok(ShellAction::Continue(format!(
                    "Config {} set{suffix}.\n",
                    setting.field
                )))
            }
            "vault" => {
                if raw_args.is_empty() || raw_args == "list" {
                    let vault = crate::vault::FileVault::from_environment(&self.environment)?;
                    let names = vault.list()?;
                    if names.is_empty() {
                        return Ok(ShellAction::Continue("No secrets stored.\n".to_owned()));
                    }
                    return Ok(ShellAction::Continue(format!("{}\n", names.join("\n"))));
                }
                if let Some(name) = raw_args.strip_prefix("get ").map(str::trim) {
                    if !name.is_empty() && !name.chars().any(char::is_whitespace) {
                        crate::vault::validate_secret_name(&Text::from(name))?;
                        let vault = crate::vault::FileVault::from_environment(&self.environment)?;
                        if vault.get(name)?.is_none() {
                            return Err(Error::new(
                                "Error",
                                format!("No secret stored for {name}."),
                            ));
                        }
                        return Ok(ShellAction::Continue(format!(
                            "{name} is stored (value hidden; pipe roster vault get NAME to retrieve it).\n"
                        )));
                    }
                }
                if let Some(name) = raw_args.strip_prefix("set ").map(str::trim) {
                    if !name.is_empty() && !name.chars().any(char::is_whitespace) {
                        crate::vault::validate_secret_name(&Text::from(name))?;
                        self.pending_secret = Some(name.to_owned());
                        return Ok(ShellAction::PromptSecret(
                            "Secret (input hidden): ".to_owned(),
                        ));
                    }
                }
                Err(Error::type_error(
                    "Use /vault [list], /vault get NAME, or /vault set NAME.",
                ))
            }
            "doctor" => {
                let words = arguments::split(
                    &Text::from(raw_args),
                    "Use /doctor, /doctor warm, or /doctor copilot [--opt-in] [--runtime PATH] [--model ID].",
                )?
                .into_iter()
                .map(|w| w.to_utf8())
                .collect::<Result<Vec<_>, _>>()?;
                if words.first().map(String::as_str) == Some("copilot") {
                    let mut opts = crate::capabilities::parse_copilot_doctor_args(&words[1..])?;
                    opts.is_tty = Some(true);
                    let report = crate::capabilities::check_copilot_doctor(&opts)?;
                    let out = crate::capabilities::format_copilot_doctor(&report)?;
                    return Ok(ShellAction::Continue(self.redact_text(&out)?));
                }
                if words.is_empty() {
                    let source = crate::cancel::CancellationSource::default();
                    let root = self.current_root();
                    let report = crate::local_cli::doctor(
                        &self.installation,
                        &root,
                        &self.environment,
                        &source.token(),
                    )
                    .await?;
                    return Ok(ShellAction::Continue(self.redact_text(&report.output)?));
                }
                if words.len() == 1 && words[0] == "warm" {
                    let mut transport =
                        crate::llm::http::HttpTransport::new().map_err(|failure| *failure.error)?;
                    let source = crate::cancel::CancellationSource::default();
                    let report = crate::diagnostics::warm_doctor(
                        &self.config,
                        &self.environment,
                        &mut transport,
                        &source.token(),
                    )
                    .await?;
                    let status_str = if report.skipped {
                        "skipped".to_owned()
                    } else {
                        report
                            .status
                            .map_or_else(|| "unknown".to_owned(), |s| s.to_string())
                    };
                    let out = format!("Warm probe: host={} status={status_str}\n", self.host_str());
                    return Ok(ShellAction::Continue(self.redact_text(&out)?));
                }
                Err(Error::type_error(
                    "Use /doctor, /doctor warm, or /doctor copilot [--opt-in] [--runtime PATH] [--model ID].",
                ))
            }
            "stats" => {
                let root = self.current_root();
                let source = crate::cancel::CancellationSource::default();
                let token = source.token();
                let mut opts = crate::metrics::Options::default();
                let mut json_flag = false;
                if !raw_args.is_empty() {
                    if raw_args.starts_with('-') {
                        for word in raw_args.split_whitespace() {
                            match word {
                                "--delivery" => opts.delivery = true,
                                "--json" => json_flag = true,
                                _ => {
                                    return Err(Error::type_error(
                                        "Use /stats [REF] | /stats --delivery [--json].",
                                    ));
                                }
                            }
                        }
                    } else if raw_args.chars().any(char::is_whitespace) {
                        return Err(Error::type_error("Use /stats [REF]."));
                    } else {
                        opts.reference = Some(raw_args.to_owned());
                    }
                }
                let evidence =
                    crate::metrics::load(&root, &self.environment, &opts, &token).await?;
                let out = if opts.delivery {
                    let groups = evidence.delivery_groups()?;
                    if json_flag {
                        format!("{}\n", Value::Array(groups).pretty())
                    } else {
                        crate::learning::format_delivery_metrics(&groups)?
                    }
                } else {
                    let groups = evidence.groups()?;
                    if json_flag {
                        format!("{}\n", Value::Array(groups).pretty())
                    } else {
                        let mut table = crate::learning::format_metrics(&groups)?;
                        let proposals = crate::metrics::list_failure_proposals(&root)?;
                        table.push_str(&crate::learning::format_failure_proposals(&proposals)?);
                        table
                    }
                };
                Ok(ShellAction::Continue(out))
            }
            "learn" => {
                if raw_args != "--recurring" {
                    return Err(Error::type_error("Use /learn --recurring."));
                }
                let root = self.current_root();
                let res = crate::metrics::propose_recurring_failures(
                    &self.installation,
                    &root,
                    &self.environment,
                )?;
                Ok(ShellAction::Continue(format!(
                    "Recurring failures: {} draft proposals created; {} existing drafts preserved. Human review required.\n",
                    res.created.len(),
                    res.existing.len()
                )))
            }
            "recommend" => {
                let mut task_class = "feat".to_owned();
                let mut difficulty = 2_u8;
                if !raw_args.is_empty() {
                    let words = raw_args.split_whitespace().collect::<Vec<_>>();
                    if !crate::learning::TASK_CLASSES.contains(&words[0]) {
                        return Err(Error::type_error(
                            "Use /recommend [feat|fix|docs|test] [--difficulty 1-5].",
                        ));
                    }
                    task_class = words[0].to_owned();
                    if words.len() == 3 && words[1] == "--difficulty" {
                        difficulty = words[2]
                            .parse::<u8>()
                            .ok()
                            .filter(|d| (1..=5).contains(d))
                            .ok_or_else(|| {
                                Error::type_error(
                                    "Use /recommend [feat|fix|docs|test] [--difficulty 1-5].",
                                )
                            })?;
                    } else if words.len() != 1 {
                        return Err(Error::type_error(
                            "Use /recommend [feat|fix|docs|test] [--difficulty 1-5].",
                        ));
                    }
                }
                let root = self.current_root();
                let source = crate::cancel::CancellationSource::default();
                let evidence = crate::metrics::load(
                    &root,
                    &self.environment,
                    &crate::metrics::Options::default(),
                    &source.token(),
                )
                .await?;
                let rec = evidence.recommendation(&task_class, Some(difficulty))?;
                let out = crate::learning::format_recommendation(
                    rec.as_ref(),
                    &task_class,
                    &self.config,
                    &self.environment,
                )?;
                Ok(ShellAction::Continue(out))
            }
            "eval" => {
                if domain::truthy(self.environment.get("ROSTER_SEAT").unwrap_or(&Value::Null)) {
                    return Err(Error::new(
                        "Error",
                        "AI-Eval is human-only; an agent seat cannot record an evaluation.",
                    ));
                }
                let parsed = parse_shell_evaluation_args(raw_args)?;
                let flags = vec![
                    parsed.values[0].clone(),
                    parsed.values[1].clone(),
                    parsed.values[2].clone(),
                    parsed.values[3].clone(),
                    "--local".to_owned(),
                    "--minutes".to_owned(),
                    parsed.minutes.to_string(),
                    "--comment".to_owned(),
                    parsed.comment,
                ];
                let decision = crate::evaluation::Decision::parse(&flags)?;
                let root = self.current_root();
                let source = crate::cancel::CancellationSource::default();
                let recorded =
                    crate::evaluation::record(&root, &self.environment, &decision, &source.token())
                        .await?;
                Ok(ShellAction::Continue(recorded.format()))
            }
            "log" => {
                if raw_args == "debug" {
                    if !self.debug.enabled() {
                        return Err(Error::new(
                            "Error",
                            "Debug logging is off. Use /debug on before /log debug.",
                        ));
                    }
                    let tail = self.debug.tail(50)?;
                    let out = match tail {
                        Some((_, lines)) if !lines.is_empty() => format!("{}\n", lines.join("\n")),
                        _ => "No debug events recorded in this process.\n".to_owned(),
                    };
                    return Ok(ShellAction::Continue(out));
                }
                let issue = crate::start::parse_issue_number(raw_args)
                    .map_err(|_| Error::type_error("Use /log N."))?;
                let root = self.current_root();
                let api_key_env = self.api_key_env();
                let logs = crate::diagnostics::read_issue_logs(
                    &root,
                    issue,
                    Some(&self.environment),
                    api_key_env.as_deref(),
                    50,
                )?;
                if logs.is_empty() {
                    return Ok(ShellAction::Continue(format!(
                        "No local run logs for issue #{issue}.\n"
                    )));
                }
                let mut out = String::new();
                for log in logs {
                    out.push_str(&format!(
                        "{}\n{}\n",
                        log.path.display(),
                        log.lines.join("\n")
                    ));
                }
                Ok(ShellAction::Continue(out))
            }
            "queue" => {
                if raw_args.is_empty() || raw_args == "list" {
                    if self.ask_queue.is_empty() {
                        return Ok(ShellAction::Continue("No Asks queued.\n".to_owned()));
                    }
                    let mut out = String::new();
                    for (idx, ask) in self.ask_queue.iter().enumerate() {
                        out.push_str(&format!("{}. {ask}\n", idx + 1));
                    }
                    return Ok(ShellAction::Continue(out));
                }
                if raw_args == "clear" {
                    self.ask_queue.clear();
                    return Ok(ShellAction::Continue("Ask queue cleared.\n".to_owned()));
                }
                if let Some(num_str) = raw_args.strip_prefix("drop ").map(str::trim) {
                    if let Ok(idx) = num_str.parse::<usize>() {
                        if (1..=8).contains(&idx) {
                            if idx > self.ask_queue.len() {
                                return Err(Error::type_error(
                                    "No queued Ask at that number. Use /queue list.",
                                ));
                            }
                            let removed = self.ask_queue.remove(idx - 1);
                            return Ok(ShellAction::Continue(format!(
                                "Dropped {idx}. {removed}\n"
                            )));
                        }
                    }
                }
                if raw_args.len() > 4096 || self.redact_text(raw_args)? != raw_args {
                    return Err(Error::type_error(
                        "Queued Ask is empty, longer than 4096 characters, or contains secret material.",
                    ));
                }
                if self.ask_queue.len() >= 8 {
                    return Err(Error::type_error(
                        "Ask queue is full (8). Use /queue drop N or /queue clear.",
                    ));
                }
                self.ask_queue.push(raw_args.to_owned());
                let preview: String = raw_args.chars().take(120).collect();
                Ok(ShellAction::Continue(format!(
                    "Queued Ask {}: {preview}\n",
                    self.ask_queue.len()
                )))
            }
            "clarify" => {
                let root = self.current_root();
                let api_key_env = self.api_key_env();
                let opts = crate::ask::ClarificationOptions {
                    repo_root: &root,
                    env: Some(&self.environment),
                    api_key_env: api_key_env.as_deref(),
                    id: None,
                };
                if raw_args.is_empty() || raw_args == "show" {
                    let record = crate::ask::read_clarification(&opts)?;
                    let out = crate::ask::format_clarification_checkpoint(record.as_ref(), true)?;
                    return Ok(ShellAction::Continue(self.redact_text(&out)?));
                }
                if raw_args == "cancel" {
                    let cancelled = crate::ask::cancel_clarification(&opts)?;
                    return Ok(ShellAction::Continue(if cancelled {
                        "Clarification cancelled. No new execution or publication started.\n"
                            .to_owned()
                    } else {
                        "No pending clarification.\n".to_owned()
                    }));
                }
                let answer = if raw_args == "continue" {
                    None
                } else {
                    Some(raw_args)
                };
                let resumed = crate::ask::resume_clarification(
                    answer,
                    &crate::ask::ClarificationOptions {
                        repo_root: &root,
                        env: Some(&self.environment),
                        api_key_env: api_key_env.as_deref(),
                        id: None,
                    },
                )?;
                let out = self.run_ask_text(&resumed.ask).await?;
                let _ = crate::ask::cancel_clarification(&crate::ask::ClarificationOptions {
                    repo_root: &root,
                    env: Some(&self.environment),
                    api_key_env: api_key_env.as_deref(),
                    id: Some(&resumed.record.id),
                });
                Ok(ShellAction::Continue(out))
            }
            "question" => {
                if raw_args.is_empty() {
                    return Err(Error::type_error(
                        "Use /question TEXT with an explicit Files allowed section.",
                    ));
                }
                let q_text = if raw_args.starts_with("--files") {
                    let words = arguments::split(
                        &Text::from(raw_args),
                        "Use /question [--files PATH,PATH --] TEXT.",
                    )?
                    .into_iter()
                    .map(|w| w.to_utf8())
                    .collect::<Result<Vec<_>, _>>()?;
                    crate::ask::repository_question_text(&words)?
                } else {
                    raw_args.to_owned()
                };
                let out = self.run_question_text(&q_text).await?;
                Ok(ShellAction::Continue(out))
            }
            "btw" => {
                if raw_args.is_empty() {
                    return Err(Error::type_error("Use /btw QUESTION."));
                }
                let mut transport =
                    crate::llm::http::HttpTransport::new().map_err(|failure| *failure.error)?;
                let source = crate::cancel::CancellationSource::default();
                let token = source.token();
                let mut side_opts = crate::ask::SideQuestionOptions {
                    worktree_path: self.last_worktree.as_deref(),
                    run_ask: None,
                    config: &self.config,
                    env: Some(&self.environment),
                    vault: None,
                    transport: &mut transport,
                    signal: Some(&token),
                };
                let ans = crate::ask::ask_side_question(raw_args, &mut side_opts).await?;
                Ok(ShellAction::Continue(format!(
                    "{}\n",
                    self.redact_text(&ans)?
                )))
            }
            "ask" => {
                if raw_args.is_empty() {
                    return Err(Error::type_error("Use /ask TEXT."));
                }
                let out = self.run_ask_text(raw_args).await?;
                Ok(ShellAction::Continue(out))
            }
            "plan" => {
                if raw_args.is_empty() {
                    return Err(Error::type_error("Use /plan TEXT, or /run N --plan."));
                }
                let mut transport =
                    crate::llm::http::HttpTransport::new().map_err(|failure| *failure.error)?;
                let source = crate::cancel::CancellationSource::default();
                let token = source.token();
                let root = self.current_root();
                let mut write_opts = crate::ask::WriteAskOptions {
                    repo_root: &root,
                    config: &self.config,
                    id: None,
                    env: Some(&self.environment),
                    vault: None,
                    transport: &mut transport,
                    signal: Some(&token),
                };
                let outcome = crate::ask::write_ask(raw_args, &mut write_opts).await?;
                Ok(ShellAction::Continue(crate::ask::format_ask(&outcome)))
            }
            "map" => {
                if !raw_args.is_empty() {
                    return Err(Error::type_error("Use /map."));
                }
                let wt = self
                    .last_worktree
                    .clone()
                    .unwrap_or_else(|| self.current_root());
                let api_key_env = self.api_key_env();
                let (_, lines) = crate::diagnostics::write_repo_map(
                    &wt,
                    Some(&self.environment),
                    api_key_env.as_deref(),
                )?;
                Ok(ShellAction::Continue(format!(
                    "Repo map: {lines} filename-only lines (cap 80).\n"
                )))
            }
            "diff" => {
                if !raw_args.is_empty() {
                    return Err(Error::type_error("Use /diff."));
                }
                let wt = self
                    .last_worktree
                    .clone()
                    .unwrap_or_else(|| self.current_root());
                let raw = run_banner_git(
                    &wt,
                    &[
                        "--no-pager",
                        "diff",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--name-only",
                        "-z",
                        "HEAD",
                        "--",
                    ],
                )
                .unwrap_or_default();
                let names = crate::start::parse_diff_names(&raw);
                if names.is_empty() {
                    Ok(ShellAction::Continue("No tracked diff.\n".to_owned()))
                } else {
                    Ok(ShellAction::Continue(format!("{}\n", names.join("\n"))))
                }
            }
            "worktrees" => {
                if !raw_args.is_empty() {
                    return Err(Error::type_error("Use /worktrees."));
                }
                let root = self.current_root();
                let raw = run_banner_git(&root, &["worktree", "list", "--porcelain", "-z"])
                    .unwrap_or_default();
                let inv = crate::start::parse_inventory(raw.as_bytes()).unwrap_or_default();
                let issue_wts = inv
                    .into_iter()
                    .filter(|wt| {
                        wt.branch
                            .as_deref()
                            .and_then(crate::start::parse_issue_branch)
                            .is_some()
                    })
                    .collect::<Vec<_>>();
                if issue_wts.is_empty() {
                    return Ok(ShellAction::Continue(
                        "No registered issue worktrees.\n".to_owned(),
                    ));
                }
                let mut out = String::new();
                for wt in issue_wts {
                    let br = wt.branch.as_deref().unwrap_or("-");
                    out.push_str(&format!(
                        "{} | {br} | coder | registered\n",
                        wt.path.display()
                    ));
                }
                Ok(ShellAction::Continue(self.redact_text(&out)?))
            }
            "recap" => {
                if !raw_args.is_empty() {
                    return Err(Error::type_error("Use /recap."));
                }
                let wt = self
                    .last_worktree
                    .clone()
                    .unwrap_or_else(|| self.current_root());
                let recap = crate::start::format_run_recap(
                    &wt,
                    None,
                    self.last_finish_reason.as_deref(),
                    Some(&self.environment),
                )?;
                Ok(ShellAction::Continue(recap))
            }
            "issues" => {
                if !raw_args.is_empty() {
                    return Err(Error::type_error("Use /issues."));
                }
                let root = self.current_root();
                let origin = run_banner_git(&root, &["remote", "get-url", "origin"])?;
                let repo = crate::ask::github_repository(&origin)?;
                let out = StdCommand::new("gh")
                    .current_dir(&root)
                    .env("GH_PROMPT_DISABLED", "1")
                    .args([
                        "issue",
                        "list",
                        "--repo",
                        &repo,
                        "--state",
                        "open",
                        "--limit",
                        "100",
                        "--json",
                        "number,title",
                    ])
                    .output()
                    .map_err(|e| Error::new("Error", e.to_string()))?;
                if !out.status.success() {
                    return Err(Error::new(
                        "Error",
                        "Could not read board metadata with gh; check repository access and connectivity.",
                    ));
                }
                let raw = String::from_utf8_lossy(&out.stdout);
                let issues = crate::start::parse_open_issues_json(&raw)?;
                if issues.is_empty() {
                    return Ok(ShellAction::Continue("No open issues.\n".to_owned()));
                }
                let mut text = String::new();
                for item in &issues {
                    let num = domain::safe_integer(item.get("number")).unwrap_or(0);
                    let title = clean_display(item.get("title"), "-");
                    text.push_str(&format!("#{num} {title}\n"));
                }
                if issues.len() == 100 {
                    text.push_str(
                        "Showing the first 100 open issues; use GitHub for the remaining board.\n",
                    );
                }
                Ok(ShellAction::Continue(self.redact_text(&text)?))
            }
            "issue" => {
                let number = crate::start::parse_issue_number(raw_args)
                    .map_err(|_| Error::type_error("Use /issue N."))?;
                if let Some(cached) = self.issue_cache.get(&number) {
                    let summary =
                        crate::start::format_issue_summary(cached, Some(&self.environment))?;
                    return Ok(ShellAction::Continue(summary));
                }
                let root = self.current_root();
                let origin = run_banner_git(&root, &["remote", "get-url", "origin"])?;
                let repo = crate::ask::github_repository(&origin)?;
                let out = StdCommand::new("gh")
                    .current_dir(&root)
                    .env("GH_PROMPT_DISABLED", "1")
                    .args([
                        "issue",
                        "view",
                        &number.to_string(),
                        "--repo",
                        &repo,
                        "--json",
                        "number,title,state",
                    ])
                    .output()
                    .map_err(|e| Error::new("Error", e.to_string()))?;
                if !out.status.success() {
                    return Err(Error::new(
                        "Error",
                        format!("Could not read issue #{number} metadata with gh."),
                    ));
                }
                let issue_json =
                    crate::json::parse(&Text::from(String::from_utf8_lossy(&out.stdout).as_ref()))?;
                let cached = Value::object(vec![
                    ("issue", issue_json),
                    ("branch", Value::string(&format!("agents/{number}"))),
                    ("openPr", Value::Null),
                ]);
                self.issue_cache.insert(number, cached.clone());
                let summary = crate::start::format_issue_summary(&cached, Some(&self.environment))?;
                Ok(ShellAction::Continue(summary))
            }
            "remediate" => {
                let files = raw_args
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                let root = self.current_root();
                let task = crate::fleet::remediation_task(&files, &root.display().to_string())?;
                let loaded_fleet = crate::fleet::load(&root)?;
                let prof_vals = loaded_fleet
                    .profiles()
                    .iter()
                    .map(|p| p.as_value().clone())
                    .collect::<Vec<_>>();
                let selected = crate::fleet::select_remediation_profile(
                    &prof_vals,
                    self.fleet_profile_id.as_deref(),
                );
                let prof_label = if let Some(ref p) = selected {
                    let id = clean_display(p.get("id"), "current");
                    self.config = crate::fleet::with_profile(&self.config, p)?;
                    self.fleet_profile_id = Some(id.clone());
                    self.sync_model_display();
                    id
                } else {
                    self.fleet_profile_id
                        .clone()
                        .unwrap_or_else(|| "current".to_owned())
                };
                let task_files = task
                    .get("files")
                    .and_then(domain::items)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|i| i.as_string()?.to_utf8().ok())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                let header = format!(
                    "Remediation agent for {}\nFiles: {task_files}\nProfile: {prof_label}\n",
                    root.display()
                );
                Ok(ShellAction::Continue(self.redact_text(&header)?))
            }
            "publish" => {
                config::require_publication_enabled(&self.config)?;
                let parsed = crate::run::parse_publish_args(raw_args)?;
                let subject = parsed
                    .subject
                    .unwrap_or_else(|| "<conventional subject>".to_owned());
                let root = self
                    .last_worktree
                    .clone()
                    .unwrap_or_else(|| self.current_root());
                let task_slug = crate::run::publication_task(
                    &root,
                    Some(&self.environment),
                    self.last_task.as_deref(),
                )?;
                let publish_env = crate::run::build_publish_env(
                    &self.config,
                    &self.environment,
                    None,
                    parsed.model.as_deref(),
                    None,
                    Some(&task_slug),
                )?;
                let model = publish_env
                    .get("AI_MODEL")
                    .and_then(Value::as_string)
                    .and_then(|t| t.to_utf8().ok())
                    .unwrap_or_else(|| "GPT-6.1-Sol".to_owned());
                let msg = crate::run::build_publish_message(&crate::run::PublishMessageOptions {
                    subject: &subject,
                    model: &model,
                    summary: &subject,
                    tests_skipped: false,
                    issue_number: self.last_issue,
                    seats: None,
                    ghcp: true,
                    scope_files: &[],
                })?;
                let env_block = crate::run::format_publish_environment(&publish_env);
                let cmd_line =
                    crate::run::format_publish_command(&msg, &model, None, cfg!(windows))?;
                let out = format!(
                    "From {}, publish reviewed changes:\nSet these metadata values (empty values clear inherited fields):\n{env_block}{cmd_line}\n",
                    root.display()
                );
                Ok(ShellAction::Continue(out))
            }
            "stop" => {
                if !raw_args.is_empty() {
                    return Err(Error::type_error("Use /stop."));
                }
                Ok(ShellAction::Continue("No active run.\n".to_owned()))
            }
            "batch" => Err(Error::new(
                "Error",
                "One seat at a time. Worktrees are isolated.",
            )),
            "steer" => {
                if raw_args.is_empty() {
                    return Err(Error::type_error("Use /steer TEXT."));
                }
                Err(Error::new(
                    "Error",
                    "Steering requires a drafting coder model call; it cannot change planner or reviewer scope.",
                ))
            }
            "checkpoints" | "rewind" => {
                let Some(wt) = &self.last_worktree else {
                    return Err(Error::new(
                        "Error",
                        "Run or resume a task worktree before using checkpoints.",
                    ));
                };
                let Some(task) = &self.last_task else {
                    return Err(Error::new(
                        "Error",
                        "Run or resume a task worktree before using checkpoints.",
                    ));
                };
                if command == "checkpoints" {
                    if !raw_args.is_empty() {
                        return Err(Error::type_error("Use /checkpoints."));
                    }
                    let cp_dir = wt
                        .join(".roster")
                        .join("checkpoints")
                        .join(&crate::identity::sha256_hex(task.as_bytes())[..16]);
                    let records_file = cp_dir.join("records.jsonl");
                    let Some(snap) = secure::read(&records_file, 65_536)? else {
                        return Ok(ShellAction::Continue("No checkpoints.\n".to_owned()));
                    };
                    let text = String::from_utf8(snap.bytes)
                        .map_err(|e| Error::new("Error", e.to_string()))?;
                    let mut out = String::new();
                    for line in text.lines().filter(|l| !l.trim().is_empty()) {
                        let rec = crate::json::parse(&Text::from(line))?;
                        let num = domain::safe_integer(rec.get("number")).unwrap_or(0);
                        let seat = clean_display(rec.get("seat"), "-");
                        let status = clean_display(rec.get("status"), "-");
                        let time = clean_display(rec.get("time"), "-");
                        out.push_str(&format!("{num} | {seat} | {status} | {time}\n"));
                    }
                    if out.is_empty() {
                        return Ok(ShellAction::Continue("No checkpoints.\n".to_owned()));
                    }
                    return Ok(ShellAction::Continue(out));
                }
                Err(Error::new(
                    "Error",
                    "Rewind requires the current TASK.md product scope.",
                ))
            }
            "resume" => {
                if raw_args.is_empty() {
                    return Ok(ShellAction::Continue("No local issue runs.\n".to_owned()));
                }
                let parts = arguments::split(&Text::from(raw_args), "Use /resume [N] [--green].")?
                    .into_iter()
                    .map(|w| w.to_utf8())
                    .collect::<Result<Vec<_>, _>>()?;
                let number = parts
                    .first()
                    .and_then(|s| crate::start::parse_issue_number(s).ok())
                    .ok_or_else(|| Error::type_error("Use /resume [N] [--green]."))?;
                let root = self.current_root();
                let wt = root.join(".worktrees").join(format!("issue-{number}"));
                let info = crate::start::inspect_local_issue_worktree(number, &wt)?;
                self.last_worktree = Some(wt);
                self.last_issue = Some(number);
                self.last_task = Some(format!("issue-{number}"));
                let state = clean_display(info.get("state"), "idle");
                let _ = domain::put(&mut self.display, "issue", Value::Number(number as f64));
                let _ = domain::put(&mut self.display, "state", Value::string(&state));
                Ok(ShellAction::Continue(format!(
                    "Resumed local issue #{number} ({state}).\n"
                )))
            }
            "waves" => {
                if !raw_args.is_empty() && raw_args != "open" {
                    return Err(Error::type_error("Use /waves or /waves open."));
                }
                let Some(wt) = &self.last_worktree else {
                    return Err(Error::new(
                        "Error",
                        "Run or resume a plan worktree before /waves.",
                    ));
                };
                let wave_plan = crate::start::read_wave_plan(wt)?;
                let mut out = String::new();
                if let Some(issues) = wave_plan.plan.get("issues").and_then(domain::items) {
                    for draft in issues {
                        let w = domain::safe_integer(draft.get("wave")).unwrap_or(1);
                        let title = clean_display(draft.get("title"), "-");
                        out.push_str(&format!("wave:{w} | {title} | todo\n"));
                    }
                }
                Ok(ShellAction::Continue(self.redact_text(&out)?))
            }
            "run" | "review" | "retry" => Err(Error::new(
                "Error",
                format!(
                    "Use `roster run` from the CLI or supply an Ask directly at `roster> ` (/{command})."
                ),
            )),
            _ => Err(Error::new("Error", unknown.trim())),
        }
    }

    async fn run_question_text(&mut self, text: &str) -> Result<String, Error> {
        let mut transport =
            crate::llm::http::HttpTransport::new().map_err(|failure| *failure.error)?;
        let source = crate::cancel::CancellationSource::default();
        let token = source.token();
        let root = self.current_root();
        let mut q_opts = crate::ask::QuestionOptions {
            cwd: &root,
            config: &self.config,
            env: Some(&self.environment),
            vault: None,
            transport: &mut transport,
            signal: Some(&token),
        };
        let res = crate::ask::answer_repository_question(text, &mut q_opts).await?;
        Ok(format!("{}\n", self.redact_text(&res.answer)?))
    }

    async fn run_ask_text(&mut self, text: &str) -> Result<String, Error> {
        let cleaned = crate::planning::intake::clean_ask(&Value::string(text))?;
        if crate::ask::is_repository_question_ask(&cleaned)? {
            return self.run_question_text(&cleaned).await;
        }
        let classified = crate::planning::intake::classify(&Value::string(&cleaned), None, &[])?;
        let ask_kind = classified
            .get("kind")
            .and_then(Value::as_string)
            .map(Text::to_utf8)
            .transpose()?
            .unwrap_or_else(|| "slice".to_owned());
        let root = self.current_root();
        let api_key_env = self.api_key_env();
        if ask_kind == "clarify" {
            let cp = crate::ask::save_clarification(
                &cleaned,
                &crate::ask::ClarificationOptions {
                    repo_root: &root,
                    env: Some(&self.environment),
                    api_key_env: api_key_env.as_deref(),
                    id: None,
                },
            )?;
            let cp_id = cp.id.clone();
            let detail = crate::planning::intake::clarification(&Value::string(&cleaned))?;
            let str_field = |k: &str| {
                detail
                    .get(k)
                    .and_then(Value::as_string)
                    .and_then(|t| t.to_utf8().ok())
                    .unwrap_or_default()
            };
            let list_field = |k: &str| {
                detail
                    .get(k)
                    .and_then(domain::items)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|i| i.as_string()?.to_utf8().ok())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            };
            let clarify_out = crate::ask::AskOutcome::Clarify {
                ask_kind: "clarify".to_owned(),
                clarification: str_field("clarification"),
                guidance: str_field("guidance"),
                interpretation: str_field("interpretation"),
                assumptions: list_field("assumptions"),
                questions: list_field("questions"),
                template: str_field("template"),
                checkpoint: Some(cp),
            };
            return Ok(format!(
                "{}Checkpoint saved privately: {cp_id}. Use /clarify to inspect, /clarify TEXT with explicit Allowed Files to resume, or /clarify cancel.\n",
                crate::ask::format_ask(&clarify_out)
            ));
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
        let _ = domain::put(&mut self.display, "state", Value::string("planning"));
        let _ = domain::put(&mut self.display, "startedAt", Value::Number(now_ms));

        let mut transport =
            crate::llm::http::HttpTransport::new().map_err(|failure| *failure.error)?;
        let source = crate::cancel::CancellationSource::default();
        let token = source.token();
        let mut write_opts = crate::ask::WriteAskOptions {
            repo_root: &root,
            config: &self.config,
            id: None,
            env: Some(&self.environment),
            vault: None,
            transport: &mut transport,
            signal: Some(&token),
        };
        let mut outcome = match crate::ask::write_ask(&cleaned, &mut write_opts).await {
            Ok(out) => out,
            Err(err) => {
                let _ = domain::put(&mut self.display, "state", Value::string("failed"));
                return Err(err);
            }
        };
        let mut extra_summary = String::new();
        if let crate::ask::AskOutcome::Draft {
            ref draft,
            ref mut command,
        } = outcome
        {
            if command.is_empty() {
                *command = format!(
                    "roster run --ask-file {} --runtime builtin --no-fetch",
                    crate::ask::quote_for_shell(&draft.ask_path.display().to_string())
                );
            }
            if let Some(usage) = &draft.usage {
                if let Some(pt) = domain::safe_integer(usage.get("prompt_tokens")) {
                    let _ = domain::put(&mut self.display, "contextUsed", Value::Number(pt as f64));
                }
                if let Some(ct) = domain::safe_integer(usage.get("completion_tokens")) {
                    let _ =
                        domain::put(&mut self.display, "outputTokens", Value::Number(ct as f64));
                }
            }
            if let Some(ref task_path) = draft.task_path {
                if let Ok(task_src) = fs::read_to_string(task_path) {
                    if let Ok(doc) = crate::planning::task::parse(
                        &Value::string(&task_src),
                        &crate::planning::task::Authority::default(),
                    ) {
                        let title_str = doc.title.to_utf8().unwrap_or_default();
                        let files_str = doc.files_allowed.join(", ");
                        let checks_str = doc
                            .acceptance_checks
                            .iter()
                            .filter_map(|c| c.to_utf8().ok())
                            .map(|c| format!("- {c}"))
                            .collect::<Vec<_>>()
                            .join("\n");
                        extra_summary = format!(
                            "Task summary:\nOutcome: {title_str}\nAllowed files: {files_str}\nChecks:\n{checks_str}\n"
                        );
                    }
                }
            }
            if let Some(parent) = draft.ask_path.parent() {
                self.last_worktree = Some(parent.to_path_buf());
            }
        }
        let _ = domain::put(&mut self.display, "state", Value::string("passed"));
        Ok(format!(
            "{extra_summary}{}",
            crate::ask::format_ask(&outcome)
        ))
    }
}

fn terminal_columns() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&c| c >= 20)
        .unwrap_or(80)
}

pub async fn start_repl(
    cwd: &Path,
    installation: &Path,
    environment: &Value,
    debug_enabled: bool,
) -> Result<u8, Error> {
    use std::io::BufRead;
    let mut session = ShellSession::new(cwd, installation, environment, debug_enabled)?;
    let color = std::env::var_os("NO_COLOR").is_none();
    let facts = collect_banner_facts(cwd, installation, environment, &session.config);
    let banner = format_banner(&facts, color);
    let mut stdout = std::io::stdout();
    stdout
        .write_all(banner.as_bytes())
        .and_then(|()| stdout.flush())
        .map_err(|e| Error::new("Error", e.to_string()))?;

    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let mut prev_bar_lines: usize;
    loop {
        let cols = terminal_columns();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
        let (tray_block, prompt, bar_lines) = render_tray_block(
            &session.display,
            &session.ask_queue,
            session.statusbar,
            session.debug.enabled(),
            cols,
            color,
            now_ms,
        );
        prev_bar_lines = bar_lines;
        stdout
            .write_all(format!("{tray_block}{prompt}").as_bytes())
            .and_then(|()| stdout.flush())
            .map_err(|e| Error::new("Error", e.to_string()))?;

        let mut line = String::new();
        let read_bytes = reader
            .read_line(&mut line)
            .map_err(|e| Error::new("Error", e.to_string()))?;
        if read_bytes == 0 {
            stdout
                .write_all(b"\n")
                .and_then(|()| stdout.flush())
                .map_err(|e| Error::new("Error", e.to_string()))?;
            break;
        }
        if prev_bar_lines > 0 && color {
            let up = prev_bar_lines + 1;
            let _ = stdout.write_all(format!("\x1b[{up}A\r\x1b[J").as_bytes());
            let shown_line = if session.pending_secret.is_some() {
                "Secret (input hidden): ********\n".to_owned()
            } else {
                format!("{prompt}{}", line.trim_end_matches(['\r', '\n'])) + "\n"
            };
            let _ = stdout.write_all(shown_line.as_bytes());
            let _ = stdout.flush();
        }
        let trimmed_line = line.trim_end_matches(['\r', '\n']);
        match session.dispatch_line(trimmed_line).await {
            Ok(ShellAction::Continue(out)) => {
                if !out.is_empty() {
                    stdout
                        .write_all(out.as_bytes())
                        .and_then(|()| stdout.flush())
                        .map_err(|e| Error::new("Error", e.to_string()))?;
                }
            }
            Ok(ShellAction::ClearScreen) => {
                stdout
                    .write_all(b"\x1b[2J\x1b[H")
                    .and_then(|()| stdout.flush())
                    .map_err(|e| Error::new("Error", e.to_string()))?;
            }
            Ok(ShellAction::Redraw) => {}
            Ok(ShellAction::PromptSecret(prompt_str)) => {
                stdout
                    .write_all(prompt_str.as_bytes())
                    .and_then(|()| stdout.flush())
                    .map_err(|e| Error::new("Error", e.to_string()))?;
                let mut sec_line = String::new();
                let n = reader
                    .read_line(&mut sec_line)
                    .map_err(|e| Error::new("Error", e.to_string()))?;
                let _ = stdout.write_all(b"\n");
                if n == 0 {
                    break;
                }
                let sec_trimmed = sec_line.trim_end_matches(['\r', '\n']);
                if let Ok(ShellAction::Continue(out)) = session.dispatch_line(sec_trimmed).await {
                    let _ = stdout.write_all(out.as_bytes());
                    let _ = stdout.flush();
                }
            }
            Ok(ShellAction::Exit) => break,
            Err(err) => {
                let mut stderr = std::io::stderr();
                let _ = stderr.write_all(format!("{}\n", err.message).as_bytes());
                let _ = stderr.flush();
            }
        }
    }
    Ok(0)
}

pub async fn run_onboard(
    cwd: &Path,
    installation: &Path,
    environment: &Value,
    discover_all: bool,
) -> Result<u8, Error> {
    use std::io::BufRead;
    let platform = if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    };
    let mut stdout = std::io::stdout();
    let write_out = |out: &mut std::io::Stdout, text: &str| -> Result<(), Error> {
        out.write_all(text.as_bytes())
            .and_then(|()| out.flush())
            .map_err(|e| Error::new("Error", e.to_string()))
    };
    write_out(
        &mut stdout,
        &format!("Roster onboarding\n\n1. Platform\nOS: {platform}\n"),
    )?;
    let project_root = paths::resolve_project_root(cwd)?;
    let config_path = project_root.join(".roster").join("config.yml");
    let fleet_path = project_root.join(".roster").join("fleet.yml");
    paths::ensure_local_path(&config_path, &project_root)?;
    let previous = settings::read_private_file(&project_root, settings::Name::Config)?;
    let base_source = match &previous {
        Some(src) => src.clone(),
        None => settings::read_config_file(&project_root.join("roster.config.example.yml"))
            .or_else(|_| {
                settings::read_config_file(&installation.join("roster.config.example.yml"))
            })?,
    };
    let base = config::parse(&base_source)?.as_value().clone();
    let previous_fleet = settings::read_private_file(&project_root, settings::Name::Fleet)?;
    let fleet_val = match &previous_fleet {
        Some(src) => crate::fleet::parse(src)?,
        None => {
            crate::fleet::validate(&Value::object(vec![("profiles", Value::Array(Vec::new()))]))?
        }
    };
    let api_key_env = base
        .get("profiles")
        .and_then(|p| p.get("vllm-local"))
        .and_then(|v| v.get("api_key_env"))
        .and_then(Value::as_string)
        .and_then(|t| t.to_utf8().ok())
        .unwrap_or_else(|| "ROSTER_API_KEY".to_owned());

    let stdin = std::io::stdin();
    let reader = std::cell::RefCell::new(stdin.lock());
    let ask = |out: &mut std::io::Stdout, prompt: &str, fallback: &str| -> Result<String, Error> {
        write_out(out, prompt)?;
        let mut line = String::new();
        let n = reader
            .borrow_mut()
            .read_line(&mut line)
            .map_err(|e| Error::new("Error", e.to_string()))?;
        if n == 0 {
            return Err(Error::new(
                "Error",
                "Onboarding ended before all questions were answered",
            ));
        }
        let trimmed = line.trim();
        Ok(if trimmed.is_empty() {
            fallback.to_owned()
        } else {
            trimmed.to_owned()
        })
    };
    let yes_no =
        |out: &mut std::io::Stdout, prompt: &str, default_yes: bool| -> Result<bool, Error> {
            let def_str = if default_yes { "yes" } else { "no" };
            loop {
                let ans = ask(out, &format!("{prompt}? [{def_str}] "), def_str)?.to_lowercase();
                if matches!(ans.as_str(), "yes" | "y") {
                    return Ok(true);
                }
                if matches!(ans.as_str(), "no" | "n") {
                    return Ok(false);
                }
                write_out(out, "Please answer yes or no.\n")?;
            }
        };
    let ask_int = |out: &mut std::io::Stdout,
                   prompt: &str,
                   fallback: u64,
                   min: u64,
                   max: u64|
     -> Result<u64, Error> {
        loop {
            let raw = ask(
                out,
                &format!("{prompt} [{fallback}]: "),
                &fallback.to_string(),
            )?;
            if let Ok(n) = raw.parse::<u64>() {
                if n >= min && n <= max {
                    return Ok(n);
                }
            }
            write_out(out, &format!("Enter an integer from {min} to {max}.\n"))?;
        }
    };

    if previous.is_some()
        && !yes_no(
            &mut stdout,
            "Replace onboarding settings in the existing private config",
            false,
        )?
    {
        write_out(&mut stdout, "Kept existing private config unchanged.\n")?;
        return Ok(0);
    }
    write_out(
        &mut stdout,
        "\n2. LLM endpoint\nWSL talking to a Windows-hosted server may need the Windows host IP, not localhost.\n",
    )?;
    write_out(
        &mut stdout,
        if discover_all {
            "Scanning local, LAN, and ROSTER_DISCOVER_HOSTS endpoints (unauthenticated metadata probes only).\n"
        } else {
            "Scanning local and configured endpoints (unauthenticated metadata probes only).\n"
        },
    )?;

    let mut transport = crate::llm::http::HttpTransport::new().map_err(|failure| *failure.error)?;
    let candidates = if discover_all {
        crate::fleet::discovery_candidates(Some(environment), &[])
    } else {
        crate::fleet::local_candidates()
    };
    let mut found = Vec::new();
    let mut failures = 0_usize;
    for candidate in candidates {
        match crate::fleet::probe_model_details(
            &candidate,
            &mut transport,
            Some(environment),
            Some(&api_key_env),
            false,
            Some(2_000),
        )
        .await
        {
            Ok(models) => found.push((candidate, models)),
            Err(_) => failures += 1,
        }
    }
    if failures > 0 {
        write_out(
            &mut stdout,
            &format!("{failures} discovery probe(s) failed or timed out.\n"),
        )?;
    }
    if found.is_empty() {
        write_out(
            &mut stdout,
            "No OpenAI-compatible server answered. Enter a base URL manually.\n",
        )?;
    }
    for (idx, (url_str, models)) in found.iter().enumerate() {
        let model_list = models
            .iter()
            .filter_map(|m| {
                let id = m.get("id")?.as_string()?.to_utf8().ok()?;
                let ctx = domain::safe_integer(m.get("context_max"))
                    .map_or_else(|| "ctx=unreported".to_owned(), |c| format!("ctx={c}"));
                Some(format!("{id} {ctx}"))
            })
            .collect::<Vec<_>>()
            .join(", ");
        write_out(
            &mut stdout,
            &format!("  {}. {url_str} models={model_list}\n", idx + 1),
        )?;
    }

    let default_url = found
        .first()
        .map(|(u, _)| u.clone())
        .or_else(|| {
            base.get("llm")
                .and_then(|l| l.get("base_url"))
                .and_then(Value::as_string)
                .and_then(|t| t.to_utf8().ok())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "http://127.0.0.1:8000/v1".to_owned());

    let base_url = loop {
        let raw = ask(
            &mut stdout,
            &format!("vLLM base URL [{default_url}]: "),
            &default_url,
        )?;
        match crate::fleet::normalize_base_url(&Value::string(&raw)) {
            Ok(norm) => break norm.to_utf8()?,
            Err(_) => {
                write_out(
                    &mut stdout,
                    "Enter an HTTP(S) base URL without credentials, a query, or a fragment; not a full /models or /chat/completions endpoint.\n",
                )?;
            }
        }
    };

    let probed = crate::fleet::probe_model_details(
        &base_url,
        &mut transport,
        Some(environment),
        Some(&api_key_env),
        true,
        Some(5_000),
    )
    .await
    .ok();
    let (model, reported_ctx) = if let Some(ref details) = probed {
        write_out(&mut stdout, "Available models:\n")?;
        for (idx, m) in details.iter().enumerate() {
            let id = clean_display(m.get("id"), "-");
            write_out(&mut stdout, &format!("  {}. {id}\n", idx + 1))?;
        }
        loop {
            let choice = ask(&mut stdout, "Select a model [1]: ", "1")?;
            if let Ok(idx) = choice.parse::<usize>() {
                if idx >= 1 && idx <= details.len() {
                    let entry = &details[idx - 1];
                    let id = clean_display(entry.get("id"), "");
                    let ctx = domain::safe_integer(entry.get("context_max"));
                    break (id, ctx);
                }
            }
            write_out(&mut stdout, "Enter a listed model number.\n")?;
        }
    } else {
        write_out(&mut stdout, "Model probe failed: unavailable\n")?;
        let id = loop {
            let raw = ask(&mut stdout, "Model ID [required]: ", "")?;
            if let Ok(valid) = crate::run::resolve_publish_model(
                None,
                Some(&Value::object(vec![("AI_MODEL", Value::string(&raw))])),
                None,
                false,
            ) {
                if valid != "builtin-stub" {
                    break valid;
                }
            }
            write_out(
                &mut stdout,
                "Enter the actual single-line served model ID, not unknown or a secret.\n",
            )?;
        };
        (id, None)
    };

    let default_ctx = base
        .get("llm")
        .and_then(|l| l.get("context_max"))
        .and_then(|v| domain::safe_integer(Some(v)))
        .unwrap_or(32_768);
    let context_max = match reported_ctx {
        Some(c) => {
            write_out(
                &mut stdout,
                &format!("Using context_max={c} reported by /v1/models.\n"),
            )?;
            c
        }
        None => ask_int(
            &mut stdout,
            "Model context tokens (0 = unknown)",
            default_ctx,
            0,
            MAX_SAFE_INTEGER,
        )?,
    };

    if !yes_no(
        &mut stdout,
        "Add more endpoints later with roster fleet add. Continue",
        true,
    )? {
        write_out(&mut stdout, "Private config and fleet were not changed.\n")?;
        return Ok(0);
    }

    write_out(
        &mut stdout,
        "\n3. Permissions (local preferences, not GitHub policy grants)\nThese flags do not grant contracts policy.\n",
    )?;
    let publish = yes_no(&mut stdout, "Allow publish through GitHub App", true)?;
    let reviewer = yes_no(&mut stdout, "Require reviewer before publish", true)?;
    let run_test = yes_no(&mut stdout, "Allow run_test", true)?;
    let advanced = yes_no(&mut stdout, "Show advanced settings", false)?;
    let mut internet = None;
    let mut turns = base
        .get("loop")
        .and_then(|l| l.get("turns"))
        .and_then(|v| domain::safe_integer(Some(v)))
        .unwrap_or(12);
    let mut budget = base
        .get("context")
        .and_then(|c| c.get("budget"))
        .and_then(|v| domain::safe_integer(Some(v)))
        .unwrap_or(8_000);
    if advanced {
        write_out(&mut stdout, "\nAdvanced\n")?;
        internet = Some(yes_no(
            &mut stdout,
            "Internet search and research outside the worktree (stored only)",
            true,
        )?);
        turns = ask_int(&mut stdout, "Max tool turns", turns, 1, 64)?;
        budget = ask_int(
            &mut stdout,
            "Context char budget",
            budget,
            1,
            MAX_SAFE_INTEGER,
        )?;
    }

    let next_cfg = render_onboard_config(
        &base,
        &OnboardConfigOptions {
            base_url: &base_url,
            model: &model,
            context_max,
            publish,
            internet,
            run_test,
            reviewer,
            turns,
            budget,
        },
    )?;
    let default_prof = Value::object(vec![
        ("id", Value::string("default")),
        ("base_url", Value::string(&base_url)),
        ("model", Value::string(&model)),
        ("provider", Value::string("vllm")),
        ("context_max", Value::Number(context_max as f64)),
        ("concurrency", Value::Number(1.0)),
        ("hardware", Value::string("unspecified")),
        (
            "notes",
            Value::string("Default endpoint selected during onboarding."),
        ),
    ]);
    let mut seeded_profiles = vec![default_prof];
    for p in fleet_val.profiles() {
        if !domain::is_text(p.as_value().get("id"), "default") {
            seeded_profiles.push(p.as_value().clone());
        }
    }
    let seeded_fleet = crate::fleet::format(&Value::object(vec![(
        "profiles",
        Value::Array(seeded_profiles),
    )]))?;

    if publish {
        let app_status = check_app_identity(&project_root, Some(environment))?;
        let app_text = format_app_identity(&app_status)?;
        write_out(
            &mut stdout,
            &format!("\nApp identity (environment only)\n{app_text}"),
        )?;
    }
    write_out(
        &mut stdout,
        &format!(
            "\n4. Review\nConfig: {}\nFleet: {} (default profile; other profiles kept)\nDiscovered models with reported context limits are added; unreported limits require fleet add --context.\nEndpoint: {base_url}\nModel: {model}\nModels probe: {}\nModel context tokens: {}\nPublish enabled: {publish}\nrun_test allowed: {run_test}\nReviewer required: {reviewer}\nInternet preference: {} (stored only; no live internet tool)\nMax tool turns: {turns}\nContext char budget: {budget}\nGitHub App publication still requires GITHUB_APP_ID, a private key, and human-owned policy.\n",
            config_path.display(),
            fleet_path.display(),
            if probed.is_some() {
                "succeeded"
            } else {
                "failed; model supplied manually"
            },
            if context_max == 0 {
                "unknown".to_owned()
            } else {
                context_max.to_string()
            },
            match internet {
                Some(b) => b.to_string(),
                None => "not set".to_owned(),
            }
        ),
    )?;
    if !yes_no(&mut stdout, "Confirm write .roster/config.yml", true)? {
        write_out(&mut stdout, "Private config was not changed.\n")?;
        return Ok(0);
    }
    let source = crate::cancel::CancellationSource::default();
    settings::ensure_private_files_ignored(
        &project_root,
        &[settings::Name::Config, settings::Name::Fleet],
        Some(&source.token()),
    )
    .await?;
    settings::write_private_documents(
        &project_root,
        &[
            settings::Document {
                name: settings::Name::Fleet,
                source: seeded_fleet,
                expected_source: Some(previous_fleet),
            },
            settings::Document {
                name: settings::Name::Config,
                source: next_cfg,
                expected_source: Some(previous),
            },
        ],
    )?;
    write_out(
        &mut stdout,
        &format!(
            "\nSaved private config: {}\nSaved fleet: {}\n\n5. Doctor\n",
            config_path.display(),
            fleet_path.display()
        ),
    )?;
    let checked =
        crate::local_cli::doctor(installation, &project_root, environment, &source.token()).await?;
    write_out(&mut stdout, &checked.output)?;
    if !checked.ok {
        write_out(
            &mut stdout,
            "Doctor found missing prerequisites; config is saved, not a grant to publish.\n",
        )?;
        return Ok(1);
    }
    Ok(0)
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

    #[tokio::test(flavor = "current_thread")]
    async fn shell_session_dispatcher_banner_and_tray_block_work_end_to_end() -> Result<(), Error> {
        let install = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = std::env::temp_dir().join(format!(
            "roster-shell-test-{}-{}",
            std::process::id(),
            files::nonce()?
        ));
        files::create_private_directories(&root)?;
        let env = Value::object(Vec::new());
        let mut session = ShellSession::new(&root, install, &env, false)?;
        session.config = config::defaults()?.as_value().clone();
        let facts = collect_banner_facts(&root, install, &env, &session.config);
        let banner = format_banner(&facts, false);
        assert!(banner.contains("github-agent-roster"));
        assert!(banner.contains("contracts"));

        let (block, prompt, bar_lines) = render_tray_block(
            &session.display,
            &session.ask_queue,
            true,
            false,
            80,
            false,
            1000.0,
        );
        assert_eq!(bar_lines, 3);
        assert!(block.contains("idle"));
        assert_eq!(prompt, "roster> ");

        let help_act = session.dispatch_line("/help").await?;
        assert!(matches!(help_act, ShellAction::Continue(ref s) if s.contains("Commands:\n")));

        let status_act = session.dispatch_line("/status").await?;
        assert!(matches!(status_act, ShellAction::Continue(ref s) if s.contains("State: idle\n")));

        let eff_act = session.dispatch_line("/effort m").await?;
        assert_eq!(eff_act, ShellAction::Continue("Effort: m\n".to_owned()));

        let sb_act = session.dispatch_line("/statusbar off").await?;
        assert_eq!(
            sb_act,
            ShellAction::Continue("Status bars off.\n".to_owned())
        );
        assert!(!session.statusbar);

        let q_act = session
            .dispatch_line("/queue Add a Status section to README.md.")
            .await?;
        assert!(matches!(q_act, ShellAction::Continue(ref s) if s.contains("Queued Ask 1:")));
        let q_list = session.dispatch_line("/queue list").await?;
        assert!(
            matches!(q_list, ShellAction::Continue(ref s) if s.contains("1. Add a Status section to README.md."))
        );
        let q_clear = session.dispatch_line("/queue clear").await?;
        assert_eq!(
            q_clear,
            ShellAction::Continue("Ask queue cleared.\n".to_owned())
        );

        let long_ask = "Can we take inventory of all the command features within the roster CLI, / or otherwise to determine a feature list and then update the documentaiton and Readme.md file in the base repo so that when a user lands on the GitHub page they see all the features nad functionalities and understand how to use them. Take inventory, what is currently working or a future feature or command and then update the command and status into the readme.md file.";
        let ask_act = session.dispatch_line(long_ask).await?;
        assert!(
            matches!(ask_act, ShellAction::Continue(ref s) if s.contains("Ask kind: slice") && s.contains("Allowed files: README.md"))
        );

        assert_eq!(
            session.dispatch_line("/clear").await?,
            ShellAction::ClearScreen
        );
        assert_eq!(session.dispatch_line("/redraw").await?, ShellAction::Redraw);
        assert_eq!(session.dispatch_line("/quit").await?, ShellAction::Exit);
        let _ = fs::remove_dir_all(&root);
        Ok(())
    }
}
