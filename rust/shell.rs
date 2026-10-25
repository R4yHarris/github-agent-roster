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
            usage: "/doctor [warm|copilot [--opt-in] [--runtime PATH] [--model ID]|openshell [--opt-in] [--approve-live] [--runtime PATH] [--gateway URL] [--policy PATH] [--provider ID]]",
            description: "Run existing prerequisite checks, a read-only warm probe, or blocked offline Copilot/OpenShell diagnostics; never enables a worker or starts login.",
            example: "/doctor openshell --opt-in",
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
        CommandPage {
            name: "update",
            group: "Settings",
            usage: "/update [--check]",
            description: "Check for or install the latest Roster release from origin/main into ~/.roster.",
            example: "/update --check",
            flags: &["--check"],
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
    let env_val = crate::ask::environment_value_from_process();
    Ok(
        config::resolve_config_root_with_env(installation, cwd, Some(&env_val))?
            .join(".roster")
            .join("config.yml"),
    )
}

pub fn public_config(
    installation: &Path,
    cwd: &Path,
    env: Option<&Value>,
) -> Result<String, Error> {
    let env_val = env
        .cloned()
        .unwrap_or_else(crate::ask::environment_value_from_process);
    let file = config::resolve_config_root_with_env(installation, cwd, Some(&env_val))?
        .join(".roster")
        .join("config.yml");
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
    if root.join(".git").exists() || root.join(".gitignore").exists() {
        settings::ensure_private_files_ignored(&root, &[settings::Name::Config], token).await?;
    }
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
    pub live_output: bool,
    pub color: bool,
    pub tray_visible: bool,
    pub bar_lines: usize,
    pub last_ask_file: Option<PathBuf>,
    pub last_ask_text: Option<String>,
    pub pending_confirm: bool,
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
        let history_root =
            config::resolve_config_root_with_env(installation, cwd, Some(environment))
                .unwrap_or_else(|_| project_root.clone());
        let mut history = ShellHistory::new(&history_root, Some(environment));
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
            live_output: false,
            color: std::env::var_os("NO_COLOR").is_none(),
            tray_visible: false,
            bar_lines: 0,
            last_ask_file: None,
            last_ask_text: None,
            pending_confirm: false,
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
            if self.pending_confirm {
                self.pending_confirm = false;
                let out = self.execute_local_ask_delivery().await?;
                return Ok(ShellAction::Continue(format!("Plan accepted.\n{out}")));
            }
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
            if let Some(cmd) = translate_natural_language_command(text) {
                let act = Box::pin(self.dispatch_line(&cmd)).await?;
                return match act {
                    ShellAction::Continue(out) => Ok(ShellAction::Continue(format!(
                        "Translated natural language -> {cmd}\n{out}"
                    ))),
                    other => Ok(other),
                };
            }
            if is_workspace_inventory_ask(text)? {
                let out = self.execute_workspace_inventory(text).await?;
                return Ok(ShellAction::Continue(out));
            }
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
            "update" => {
                if !raw_args.is_empty() && raw_args != "--check" {
                    return Err(Error::type_error("Use /update [--check]."));
                }
                let out =
                    run_self_update(&self.installation, &self.environment, raw_args == "--check")?;
                Ok(ShellAction::Continue(self.redact_text(&out)?))
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
                    let loaded = crate::fleet::load_with_env(
                        &root,
                        Some(&self.installation),
                        Some(&self.environment),
                    )?;
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
                    let loaded = crate::fleet::load_with_env(
                        &root,
                        Some(&self.installation),
                        Some(&self.environment),
                    )?;
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
                    "Use /doctor, /doctor warm, /doctor copilot [--opt-in] [--runtime PATH] [--model ID], or /doctor openshell [--opt-in] [--approve-live] [--runtime PATH] [--gateway URL] [--policy PATH] [--provider ID].",
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
                if words.first().map(String::as_str) == Some("openshell") {
                    let mut opts = crate::openshell::parse_openshell_doctor_args(&words[1..])?;
                    opts.is_tty = Some(true);
                    let report = crate::openshell::check_openshell_doctor(&opts)?;
                    let out = crate::openshell::format_openshell_doctor(&report)?;
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
                    "Use /doctor, /doctor warm, /doctor copilot [--opt-in] [--runtime PATH] [--model ID], or /doctor openshell [--opt-in] [--approve-live] [--runtime PATH] [--gateway URL] [--policy PATH] [--provider ID].",
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
                    run_ask: self.last_ask_text.as_deref(),
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
                let out = self.run_ask_text_mode(raw_args, true).await?;
                Ok(ShellAction::Continue(out))
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
                let loaded_fleet = crate::fleet::load_with_env(
                    &root,
                    Some(&self.installation),
                    Some(&self.environment),
                )?;
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
                if self.pending_confirm {
                    self.pending_confirm = false;
                    let _ = domain::put(&mut self.display, "state", Value::string("idle"));
                    return Ok(ShellAction::Continue("Run cancelled.\n".to_owned()));
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
                let cp_identity = task.strip_prefix("issue-").unwrap_or(task.as_str());
                let primary_dir = wt.join(".roster").join("checkpoints").join(cp_identity);
                let read_numbered_records = || -> Result<Vec<Value>, Error> {
                    if !primary_dir.is_dir() {
                        return Ok(Vec::new());
                    }
                    let mut numbered = Vec::new();
                    for name in secure::entries(&primary_dir)? {
                        if !name.is_empty()
                            && !name.starts_with('0')
                            && name.bytes().all(|b| b.is_ascii_digit())
                        {
                            let n = crate::start::parse_issue_number(&name).map_err(|_| {
                                Error::new(
                                    "Error",
                                    "Checkpoint metadata does not match the requested task",
                                )
                            })?;
                            numbered.push((n, name));
                        }
                    }
                    numbered.sort_by_key(|(n, _)| *n);
                    let mut records = Vec::with_capacity(numbered.len());
                    let time_re =
                        regex_lite::Regex::new(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$")
                            .map_err(|e| Error::new("Error", e.to_string()))?;
                    for (expected_num, name) in numbered {
                        let file = primary_dir.join(name);
                        let snap = secure::read(&file, 262_144)?.ok_or_else(|| {
                            Error::new("Error", "Checkpoint metadata disappeared")
                        })?;
                        if paths::file_information(&file)?.links != 1 {
                            return Err(Error::new(
                                "Error",
                                "Checkpoint metadata must be a bounded regular single-link file",
                            ));
                        }
                        let text = String::from_utf8(snap.bytes)
                            .map_err(|_| Error::new("Error", "Checkpoint metadata is invalid"))?;
                        let rec = crate::json::parse(&Text::from(text.as_str()))
                            .map_err(|_| Error::new("Error", "Checkpoint metadata is invalid"))?;
                        let status = clean_display(rec.get("status"), "");
                        let tree = clean_display(rec.get("tree"), "");
                        let time = clean_display(rec.get("time"), "");
                        let valid_oid = [40, 64].contains(&tree.len())
                            && tree
                                .bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
                        let scope_ok = rec.get("scope").and_then(domain::items).is_some_and(|s| {
                            s.len() <= 64 && s.iter().all(|f| f.as_string().is_some())
                        });
                        if domain::safe_integer(rec.get("number")) != Some(expected_num)
                            || !domain::is_text(rec.get("task"), task)
                            || !domain::is_text(rec.get("seat"), "coder")
                            || !matches!(status.as_str(), "before-write" | "verified")
                            || !valid_oid
                            || !domain::is_text(
                                rec.get("ref"),
                                &format!("refs/roster/checkpoints/{task}/{expected_num}"),
                            )
                            || !scope_ok
                            || !time_re.is_match(&time)
                        {
                            return Err(Error::new(
                                "Error",
                                "Checkpoint metadata does not match the requested task",
                            ));
                        }
                        records.push(rec);
                    }
                    Ok(records)
                };
                if command == "checkpoints" {
                    if !raw_args.is_empty() {
                        return Err(Error::type_error("Use /checkpoints."));
                    }
                    let mut records = read_numbered_records()?;
                    if records.is_empty() {
                        let cp_dir = wt
                            .join(".roster")
                            .join("checkpoints")
                            .join(&crate::identity::sha256_hex(task.as_bytes())[..16]);
                        let records_file = cp_dir.join("records.jsonl");
                        if let Some(snap) = secure::read(&records_file, 65_536)? {
                            let text = String::from_utf8(snap.bytes)
                                .map_err(|e| Error::new("Error", e.to_string()))?;
                            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                                records.push(crate::json::parse(&Text::from(line))?);
                            }
                        }
                    }
                    if records.is_empty() {
                        return Ok(ShellAction::Continue("No checkpoints.\n".to_owned()));
                    }
                    let mut out = String::new();
                    for rec in &records {
                        let num = domain::safe_integer(rec.get("number")).unwrap_or(0);
                        let seat = clean_display(rec.get("seat"), "-");
                        let status = clean_display(rec.get("status"), "-");
                        let time = clean_display(rec.get("time"), "-");
                        out.push_str(&format!("{num} | {seat} | {status} | {time}\n"));
                    }
                    return Ok(ShellAction::Continue(out));
                }
                let latest = input_cmd.eq_ignore_ascii_case("undo");
                if latest {
                    if !raw_args.is_empty() {
                        return Err(Error::type_error(
                            "Use /undo for the latest checkpoint only.",
                        ));
                    }
                } else {
                    let _ = crate::start::parse_issue_number(raw_args)
                        .map_err(|_| Error::type_error("Use /rewind N."))?;
                }
                if secure::read(&wt.join("TASK.md"), 65_536)?.is_none() {
                    return Err(Error::new(
                        "Error",
                        "Rewind requires the current TASK.md product scope.",
                    ));
                }
                crate::execution::checkpoints::require_restore_integration()?;
                unreachable!()
            }
            "resume" => {
                if raw_args.is_empty() {
                    let wt_root = self.current_root().join(".worktrees");
                    if !wt_root.is_dir() {
                        return Ok(ShellAction::Continue("No local issue runs.\n".to_owned()));
                    }
                    let mut found = Vec::new();
                    for name in secure::entries(&wt_root)? {
                        if let Some(num_str) = name.strip_prefix("issue-") {
                            if let Ok(number) = crate::start::parse_issue_number(num_str) {
                                let wt = wt_root.join(&name);
                                if let Ok(info) =
                                    crate::start::inspect_local_issue_worktree(number, &wt)
                                {
                                    found.push((number, info));
                                }
                            }
                        }
                    }
                    found.sort_by_key(|(number, _)| *number);
                    if found.is_empty() {
                        return Ok(ShellAction::Continue("No local issue runs.\n".to_owned()));
                    }
                    let mut out = String::new();
                    for (number, info) in found {
                        let title =
                            clean_display(info.get("issue").and_then(|iss| iss.get("title")), "-");
                        let seat = clean_display(info.get("seat"), "coder");
                        let state = clean_display(info.get("state"), "idle");
                        out.push_str(&format!(
                            "#{number} | {title} | {seat} | {state} | issue-{number}\n"
                        ));
                    }
                    return Ok(ShellAction::Continue(out));
                }
                let parts = arguments::split(&Text::from(raw_args), "Use /resume [N] [--green].")?
                    .into_iter()
                    .map(|w| w.to_utf8())
                    .collect::<Result<Vec<_>, _>>()?;
                let green = parts.len() == 2 && parts[1] == "--green";
                if !green && parts.len() != 1 {
                    return Err(Error::type_error("Use /resume [N] [--green]."));
                }
                let number = parts
                    .first()
                    .and_then(|s| crate::start::parse_issue_number(s).ok())
                    .ok_or_else(|| Error::type_error("Use /resume [N] [--green]."))?;
                let root = self.current_root();
                let wt = root.join(".worktrees").join(format!("issue-{number}"));
                let info = crate::start::inspect_local_issue_worktree(number, &wt)?;
                if green {
                    if info.get("planMode") == Some(&Value::Boolean(true)) {
                        return Err(Error::new(
                            "Error",
                            "Green resume requires an executable TASK.md, not a plan",
                        ));
                    }
                    if secure::read(&wt.join("TASK.md"), 65_536)?.is_none() {
                        return Err(Error::new(
                            "Error",
                            "Green resume requires the current TASK.md",
                        ));
                    }
                    crate::execution::checkpoints::require_restore_integration()?;
                }
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
                if raw_args == "open"
                    && domain::truthy(self.environment.get("ROSTER_SEAT").unwrap_or(&Value::Null))
                {
                    return Err(Error::new(
                        "Error",
                        "Opening wave drafts requires an explicit human command, not an agent seat.",
                    ));
                }
                let Some(wt) = &self.last_worktree else {
                    return Err(Error::new(
                        "Error",
                        "Run or resume a plan worktree before /waves.",
                    ));
                };
                let wave_plan = crate::start::read_wave_plan(wt)?;
                let rows = crate::start::compute_wave_board_rows(
                    &wave_plan.plan,
                    &[],
                    self.last_issue,
                    self.display
                        .get("state")
                        .and_then(Value::as_string)
                        .and_then(|t| t.to_utf8().ok())
                        .as_deref(),
                )?;
                let mut out = String::new();
                for row in rows {
                    let w = domain::safe_integer(row.get("wave")).unwrap_or(1);
                    let title = clean_display(row.get("title"), "-");
                    let state = clean_display(row.get("state"), "todo");
                    let issue_suffix = domain::safe_integer(row.get("issue"))
                        .map_or_else(String::new, |n| format!(" | #{n}"));
                    out.push_str(&format!("wave:{w} | {title} | {state}{issue_suffix}\n"));
                }
                Ok(ShellAction::Continue(self.redact_text(&out)?))
            }
            "run" | "review" | "retry" => {
                if command == "retry" {
                    if !raw_args.is_empty() {
                        return Err(Error::type_error("Use /retry."));
                    }
                    if self.last_ask_file.is_some() {
                        let out = self.execute_local_ask_delivery().await?;
                        return Ok(ShellAction::Continue(out));
                    }
                    if let Some(ask_text) = self.last_ask_text.clone() {
                        let out = self.run_ask_text(&ask_text).await?;
                        return Ok(ShellAction::Continue(out));
                    }
                    return Err(Error::new("Error", "No previous command to retry."));
                }
                if command == "run" && raw_args.is_empty() && self.last_ask_file.is_some() {
                    let out = self.execute_local_ask_delivery().await?;
                    return Ok(ShellAction::Continue(out));
                }
                Err(Error::new(
                    "Error",
                    format!(
                        "Use `roster run` from the CLI or supply an Ask directly at `roster> ` (/{command})."
                    ),
                ))
            }
            _ => Err(Error::new("Error", unknown.trim())),
        }
    }

    fn llm_credentials(&self) -> crate::llm::openai::Credentials {
        let mut vault_obj = Vec::new();
        if let Ok(vault) = crate::vault::FileVault::from_environment(&self.environment) {
            for key_name in ["APERTURE_API_KEY", "ROSTER_API_KEY", "OPENAI_API_KEY"] {
                if let Ok(Some(val)) = vault.get(key_name) {
                    if !val.is_empty() {
                        vault_obj.push((Text::from(key_name), Value::string(val.as_str())));
                    }
                }
            }
            if let Some(custom_key) = self.api_key_env() {
                if !vault_obj
                    .iter()
                    .any(|(k, _)| k.0 == Text::from(custom_key.as_str()).0)
                {
                    if let Ok(Some(val)) = vault.get(&custom_key) {
                        if !val.is_empty() {
                            vault_obj.push((
                                Text::from(custom_key.as_str()),
                                Value::string(val.as_str()),
                            ));
                        }
                    }
                }
            }
        }
        crate::llm::openai::Credentials {
            environment: Some(self.environment.clone()),
            vault_entries: Some(Value::Object(vault_obj)),
        }
    }

    async fn run_question_text(&mut self, text: &str) -> Result<String, Error> {
        if is_workspace_inventory_ask(text)? {
            return self.execute_workspace_inventory(text).await;
        }
        let mut transport =
            crate::llm::http::HttpTransport::new().map_err(|failure| *failure.error)?;
        let source = crate::cancel::CancellationSource::default();
        let token = source.token();
        let root = self.current_root();
        let reqs = crate::planning::intake::requirements(&Value::string(text), true)?;
        let effective_text = if !reqs.explicit || reqs.files.is_empty() {
            let discovered = auto_discover_question_files(&root, text);
            if discovered.is_empty() {
                return self.execute_workspace_inventory(text).await;
            }
            let bullets = discovered
                .iter()
                .map(|f| format!("- {f}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!("{text}\n\n## Files allowed\n{bullets}")
        } else {
            text.to_owned()
        };
        let creds = self.llm_credentials();
        let mut q_opts = crate::ask::QuestionOptions {
            cwd: &root,
            config: &self.config,
            env: Some(&self.environment),
            vault: creds.vault_entries.as_ref(),
            transport: &mut transport,
            signal: Some(&token),
        };
        let res = crate::ask::answer_repository_question(&effective_text, &mut q_opts).await?;
        Ok(format!("{}\n", self.redact_text(&res.answer)?))
    }

    async fn execute_workspace_inventory(&mut self, text: &str) -> Result<String, Error> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
        let _ = domain::put(&mut self.display, "state", Value::string("drafting"));
        let _ = domain::put(&mut self.display, "startedAt", Value::Number(now_ms));
        self.last_ask_text = Some(text.to_owned());

        let drives = discover_drives(&self.cwd, &self.environment);
        let repos = discover_repositories(&self.cwd, &self.environment);

        let wants_file = regex_lite::Regex::new(
            r"(?i)\b(?:markdown|\.md\b|file|report|place\s+it|save\s+it|write\s+it|make\s+me\s+a)\b",
        )
        .map_err(|e| Error::new("Error", e.to_string()))?
        .is_match(text);

        if wants_file {
            let llm_summary = self
                .try_summarize_inventory_with_llm(text, &drives, &repos)
                .await;
            let report_md = render_workspace_inventory_markdown(
                text,
                &self.cwd,
                &drives,
                &repos,
                llm_summary.as_deref(),
            );
            let target_path = resolve_inventory_target_path(text, &self.cwd, &self.environment);
            if let Some(parent) = target_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            fs::write(&target_path, report_md.as_bytes())
                .map_err(|e| Error::new("Error", format!("{}: {e}", target_path.display())))?;
            let mut out = format!(
                "Wrote markdown report: {}\nDrives discovered: {} | Git repositories/worktrees discovered: {}\n\n{}",
                target_path.display(),
                drives.len(),
                repos.len(),
                report_md
            );
            if !out.ends_with('\n') {
                out.push('\n');
            }
            let _ = domain::put(&mut self.display, "state", Value::string("passed"));
            let _ = domain::put(&mut self.display, "review", Value::string("pass"));
            return self.redact_text(&out);
        }

        let answer = match self
            .answer_workspace_query_with_llm(text, &drives, &repos)
            .await
        {
            Some(llm_ans) => llm_ans,
            None => render_targeted_workspace_answer(text, &self.cwd, &drives, &repos),
        };
        let mut out = answer;
        if !out.ends_with('\n') {
            out.push('\n');
        }
        let _ = domain::put(&mut self.display, "state", Value::string("passed"));
        let _ = domain::put(&mut self.display, "review", Value::string("pass"));
        self.redact_text(&out)
    }

    async fn answer_workspace_query_with_llm(
        &mut self,
        ask_text: &str,
        drives: &[DriveEntry],
        repos: &[RepoEntry],
    ) -> Option<String> {
        let llm = self.config.get("llm")?;
        if !llm.get("base_url").is_some_and(domain::truthy)
            || !llm.get("model").is_some_and(domain::truthy)
        {
            return None;
        }
        let mut chat_cfg = self.config.clone();
        let mut chat_llm = llm.clone();
        let _ = domain::put(&mut chat_llm, "max_tokens", Value::Number(4096.0));
        let _ = domain::put(&mut chat_llm, "effort", Value::string("none"));
        let _ = domain::put(&mut chat_llm, "thinking", Value::Boolean(false));
        let _ = domain::put(&mut chat_cfg, "llm", chat_llm);
        let admission = crate::llm::admission::Admission::default();
        let mut chat = crate::llm::builtin::BuiltinChat::new(
            &chat_cfg,
            &admission,
            crate::llm::builtin::Options {
                retry_length: true,
                stream: false,
            },
        )
        .ok()??;
        let mut transport = crate::llm::http::HttpTransport::new().ok()?;

        let mut sorted_repos = repos.to_vec();
        sorted_repos.sort_by_key(|a| std::cmp::Reverse(a.modified_epoch));

        let drive_lines = drives
            .iter()
            .map(|d| {
                format!(
                    "- {} ({}) | total={} | free={} | used={} | dev_roots={}",
                    d.mount,
                    d.kind,
                    d.total_gib,
                    d.free_gib,
                    d.used_pct,
                    if d.dev_roots.is_empty() {
                        "-".to_owned()
                    } else {
                        d.dev_roots.join(", ")
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        let repo_lines = sorted_repos
            .iter()
            .enumerate()
            .map(|(idx, r)| {
                let wts = if r.linked_worktrees.is_empty() {
                    "none".to_owned()
                } else {
                    r.linked_worktrees.join(", ")
                };
                format!(
                    "{}. {} | kind={} | path={} | branch={} | state={} | modified_utc={} (epoch={}) | last_commit={} | linked_worktrees=[{}] | remote={}",
                    idx + 1,
                    r.name,
                    r.kind,
                    r.path,
                    r.branch,
                    r.state,
                    r.modified_utc,
                    r.modified_epoch,
                    r.last_commit,
                    wts,
                    r.remote
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        let user_payload = format!(
            "Current working directory: {}\n\nDiscovered Drives / Mounts:\n{}\n\nDiscovered Git Repositories & Worktrees (pre-sorted in descending order from most recently modified #1 to oldest):\n{}\n\nUser question / request:\n{}",
            self.cwd.display(),
            drive_lines,
            repo_lines,
            ask_text
        );

        let req = Value::object(vec![(
            "messages",
            Value::Array(vec![
                Value::object(vec![
                    ("role", Value::string("system")),
                    (
                        "content",
                        Value::string(
                            "You are Roster, an intelligent software engineering and workspace orchestrator CLI. Answer the user's specific question or request directly, accurately, and concisely using the live discovered drive, repository, worktree, and modification-timestamp data provided. Filter, group, or sort the repositories exactly as the user asked (for example, only repositories on a specific drive/directory, which repositories have linked worktrees, the single most recently modified repository directory, or a ranked list in descending order of modification time). Do not dump unrelated boilerplate.",
                        ),
                    ),
                ]),
                Value::object(vec![
                    ("role", Value::string("user")),
                    ("content", Value::string(&user_payload)),
                ]),
            ]),
        )]);
        let creds = self.llm_credentials();
        let mut obs = crate::llm::control::NoObserver;
        let res = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            chat.complete(
                &req,
                &mut transport,
                &mut obs,
                &creds,
                crate::llm::openai::CallOptions {
                    signal: None,
                    parent: None,
                    retry_command: None,
                },
            ),
        )
        .await
        .ok()?
        .ok()?;
        if let Some(usage) = &res.usage {
            if let Some(pt) = domain::safe_integer(usage.get("prompt_tokens")) {
                let _ = domain::put(&mut self.display, "contextUsed", Value::Number(pt as f64));
            }
            if let Some(ct) = domain::safe_integer(usage.get("completion_tokens")) {
                let _ = domain::put(&mut self.display, "outputTokens", Value::Number(ct as f64));
            }
        }
        if domain::is_text(res.finish_reason.as_ref(), "length") {
            return None;
        }
        res.message
            .get("content")
            .and_then(Value::as_string)
            .and_then(|t| t.to_utf8().ok())
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    }

    async fn try_summarize_inventory_with_llm(
        &mut self,
        ask_text: &str,
        drives: &[DriveEntry],
        repos: &[RepoEntry],
    ) -> Option<String> {
        let llm = self.config.get("llm")?;
        if !llm.get("base_url").is_some_and(domain::truthy)
            || !llm.get("model").is_some_and(domain::truthy)
        {
            return None;
        }
        let mut chat_cfg = self.config.clone();
        let mut chat_llm = llm.clone();
        let _ = domain::put(&mut chat_llm, "max_tokens", Value::Number(1024.0));
        let _ = domain::put(&mut chat_llm, "effort", Value::string("none"));
        let _ = domain::put(&mut chat_llm, "thinking", Value::Boolean(false));
        let _ = domain::put(&mut chat_cfg, "llm", chat_llm);
        let admission = crate::llm::admission::Admission::default();
        let mut chat = crate::llm::builtin::BuiltinChat::new(
            &chat_cfg,
            &admission,
            crate::llm::builtin::Options {
                retry_length: false,
                stream: false,
            },
        )
        .ok()??;
        let mut transport = crate::llm::http::HttpTransport::new().ok()?;
        let drive_summary = drives
            .iter()
            .map(|d| {
                format!(
                    "{} ({}, free {} / {})",
                    d.mount, d.kind, d.free_gib, d.total_gib
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        let repo_summary = repos
            .iter()
            .take(20)
            .map(|r| format!("{} [{} | {}] at {}", r.name, r.branch, r.state, r.path))
            .collect::<Vec<_>>()
            .join("; ");
        let prompt = format!(
            "User request: {ask_text}\nDiscovered drives: {drive_summary}\nDiscovered repositories ({} total): {repo_summary}\nWrite a concise 2-sentence executive summary of the development drives and repository state.",
            repos.len()
        );
        let req = Value::object(vec![(
            "messages",
            Value::Array(vec![
                Value::object(vec![
                    ("role", Value::string("system")),
                    (
                        "content",
                        Value::string(
                            "Provide a concise 2-sentence plain-text executive summary of the discovered development drives and Git repositories. Do not use markdown headings or code fences.",
                        ),
                    ),
                ]),
                Value::object(vec![
                    ("role", Value::string("user")),
                    ("content", Value::string(&prompt)),
                ]),
            ]),
        )]);
        let creds = self.llm_credentials();
        let mut obs = crate::llm::control::NoObserver;
        let res = tokio::time::timeout(
            std::time::Duration::from_secs(12),
            chat.complete(
                &req,
                &mut transport,
                &mut obs,
                &creds,
                crate::llm::openai::CallOptions {
                    signal: None,
                    parent: None,
                    retry_command: None,
                },
            ),
        )
        .await
        .ok()?
        .ok()?;
        if let Some(usage) = &res.usage {
            if let Some(pt) = domain::safe_integer(usage.get("prompt_tokens")) {
                let _ = domain::put(&mut self.display, "contextUsed", Value::Number(pt as f64));
            }
            if let Some(ct) = domain::safe_integer(usage.get("completion_tokens")) {
                let _ = domain::put(&mut self.display, "outputTokens", Value::Number(ct as f64));
            }
        }
        res.message
            .get("content")
            .and_then(Value::as_string)
            .and_then(|t| t.to_utf8().ok())
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    }

    fn has_configured_endpoint(&self) -> bool {
        let Some(llm) = self.config.get("llm") else {
            return false;
        };
        llm.get("base_url").is_some_and(domain::truthy)
            && llm.get("model").is_some_and(domain::truthy)
    }

    async fn translate_natural_language_with_llm(
        &mut self,
        text: &str,
        root: &Path,
    ) -> Option<NaturalLanguageTranslation> {
        let llm = self.config.get("llm")?;
        if !llm.get("base_url").is_some_and(domain::truthy)
            || !llm.get("model").is_some_and(domain::truthy)
        {
            return None;
        }
        let mut chat_cfg = self.config.clone();
        let mut chat_llm = llm.clone();
        let _ = domain::put(&mut chat_llm, "max_tokens", Value::Number(1024.0));
        let _ = domain::put(&mut chat_llm, "effort", Value::string("none"));
        let _ = domain::put(&mut chat_llm, "thinking", Value::Boolean(false));
        let _ = domain::put(&mut chat_cfg, "llm", chat_llm);
        let admission = crate::llm::admission::Admission::default();
        let mut chat = crate::llm::builtin::BuiltinChat::new(
            &chat_cfg,
            &admission,
            crate::llm::builtin::Options {
                retry_length: false,
                stream: false,
            },
        )
        .ok()??;
        let mut transport = crate::llm::http::HttpTransport::new().ok()?;
        let has_git = root.join(".git").exists();
        let repo_files = collect_workspace_file_map(root);
        let prompt = Value::object(vec![
            ("user_input", Value::string(text)),
            ("cwd", Value::string(&root.display().to_string())),
            ("is_git_repo", Value::Boolean(has_git)),
            (
                "workspace_files",
                Value::Array(repo_files.iter().map(|f| Value::string(f)).collect()),
            ),
        ])
        .json();
        let req = Value::object(vec![(
            "messages",
            Value::Array(vec![
                Value::object(vec![
                    ("role", Value::string("system")),
                    (
                        "content",
                        Value::string(
                            "Translate the user's natural-language request into a single JSON object (no markdown fences) with keys:\n- \"intent\": one of \"command\", \"workspace_inventory\", \"direct_file_task\", \"question\", or \"delivery_task\"\n- \"command\": slash command string if intent==\"command\" (e.g. \"/fleet list\", \"/doctor\", \"/doctor openshell\", \"/status\", \"/usage\", \"/issues\", \"/map\", \"/stats\")\n- \"outcome\": concise single-sentence imperative software delivery outcome\n- \"target_file\": relative filename (e.g. \"SUMMARY.md\") if creating/writing a file\n- \"files\": array of 1 to 6 relative file paths in scope\n- \"checks\": array of 1 to 3 concrete observable acceptance checks",
                        ),
                    ),
                ]),
                Value::object(vec![
                    ("role", Value::string("user")),
                    ("content", Value::string(&prompt)),
                ]),
            ]),
        )]);
        let creds = self.llm_credentials();
        let mut obs = crate::llm::control::NoObserver;
        let res = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            chat.complete(
                &req,
                &mut transport,
                &mut obs,
                &creds,
                crate::llm::openai::CallOptions {
                    signal: None,
                    parent: None,
                    retry_command: None,
                },
            ),
        )
        .await
        .ok()?
        .ok()?;
        if let Some(usage) = &res.usage {
            if let Some(pt) = domain::safe_integer(usage.get("prompt_tokens")) {
                let _ = domain::put(&mut self.display, "contextUsed", Value::Number(pt as f64));
            }
            if let Some(ct) = domain::safe_integer(usage.get("completion_tokens")) {
                let _ = domain::put(&mut self.display, "outputTokens", Value::Number(ct as f64));
            }
        }
        let raw_content = res
            .message
            .get("content")
            .and_then(Value::as_string)
            .and_then(|t| t.to_utf8().ok())?;
        parse_natural_language_translation_json(&raw_content, text, has_git, &repo_files)
    }

    async fn execute_direct_workspace_file_task(
        &mut self,
        text: &str,
        suggested_file: Option<&str>,
        summary_hint: Option<&str>,
    ) -> Result<String, Error> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
        let _ = domain::put(&mut self.display, "state", Value::string("drafting"));
        let _ = domain::put(&mut self.display, "startedAt", Value::Number(now_ms));

        let mut target_path = resolve_inventory_target_path(text, &self.cwd, &self.environment);
        if let Some(sf) = suggested_file.filter(|s| !s.is_empty() && !s.contains("..")) {
            if target_path.file_name().and_then(|n| n.to_str())
                == Some("DEV_DRIVES_AND_REPOSITORIES.md")
                && !text
                    .to_lowercase()
                    .contains("DEV_DRIVES_AND_REPOSITORIES.md")
            {
                let clean_name = Path::new(sf)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("OUTPUT.md");
                if let Some(parent) = target_path.parent() {
                    target_path = parent.join(clean_name);
                }
            }
        }

        let drives = discover_drives(&self.cwd, &self.environment);
        let repos = discover_repositories(&self.cwd, &self.environment);
        let local_files = collect_workspace_file_map(&self.cwd);

        let mut generated_body = None;
        if let Some(llm) = self.config.get("llm") {
            if llm.get("base_url").is_some_and(domain::truthy)
                && llm.get("model").is_some_and(domain::truthy)
            {
                let mut chat_cfg = self.config.clone();
                let mut chat_llm = llm.clone();
                let _ = domain::put(&mut chat_llm, "max_tokens", Value::Number(4096.0));
                let _ = domain::put(&mut chat_llm, "effort", Value::string("none"));
                let _ = domain::put(&mut chat_llm, "thinking", Value::Boolean(false));
                let _ = domain::put(&mut chat_cfg, "llm", chat_llm);
                let admission = crate::llm::admission::Admission::default();
                if let Ok(Some(mut chat)) = crate::llm::builtin::BuiltinChat::new(
                    &chat_cfg,
                    &admission,
                    crate::llm::builtin::Options {
                        retry_length: false,
                        stream: false,
                    },
                ) {
                    if let Ok(mut transport) = crate::llm::http::HttpTransport::new() {
                        let ctx_prompt = format!(
                            "User request: {text}\nWorking directory: {}\nTarget file: {}\nFiles in directory: {}\nDiscovered drives: {}\nDiscovered Git repositories: {}\n\nWrite the complete, well-structured content for `{}` fulfilling the user's request.",
                            self.cwd.display(),
                            target_path.display(),
                            local_files.join(", "),
                            drives.len(),
                            repos.len(),
                            target_path
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("OUTPUT.md")
                        );
                        let req = Value::object(vec![(
                            "messages",
                            Value::Array(vec![
                                Value::object(vec![
                                    ("role", Value::string("system")),
                                    (
                                        "content",
                                        Value::string(
                                            "Generate the exact file content requested by the user. Output only the file content without wrapping the entire document in outer markdown code fences.",
                                        ),
                                    ),
                                ]),
                                Value::object(vec![
                                    ("role", Value::string("user")),
                                    ("content", Value::string(&ctx_prompt)),
                                ]),
                            ]),
                        )]);
                        let creds = self.llm_credentials();
                        let mut obs = crate::llm::control::NoObserver;
                        if let Ok(Ok(res)) = tokio::time::timeout(
                            std::time::Duration::from_secs(20),
                            chat.complete(
                                &req,
                                &mut transport,
                                &mut obs,
                                &creds,
                                crate::llm::openai::CallOptions {
                                    signal: None,
                                    parent: None,
                                    retry_command: None,
                                },
                            ),
                        )
                        .await
                        {
                            if let Some(usage) = &res.usage {
                                if let Some(pt) = domain::safe_integer(usage.get("prompt_tokens")) {
                                    let _ = domain::put(
                                        &mut self.display,
                                        "contextUsed",
                                        Value::Number(pt as f64),
                                    );
                                }
                                if let Some(ct) =
                                    domain::safe_integer(usage.get("completion_tokens"))
                                {
                                    let _ = domain::put(
                                        &mut self.display,
                                        "outputTokens",
                                        Value::Number(ct as f64),
                                    );
                                }
                            }
                            generated_body = res
                                .message
                                .get("content")
                                .and_then(Value::as_string)
                                .and_then(|t| t.to_utf8().ok())
                                .map(|s| s.trim().to_owned())
                                .filter(|s| !s.is_empty());
                        }
                    }
                }
            }
        }

        let content = generated_body.unwrap_or_else(|| {
            let title = summary_hint.unwrap_or(text);
            let mut fallback = format!("# {title}\n\n");
            fallback.push_str(&format!("- **Directory**: `{}`\n", self.cwd.display()));
            if !local_files.is_empty() {
                fallback.push_str("\n## Workspace Files\n\n");
                for f in &local_files {
                    fallback.push_str(&format!("- `{f}`\n"));
                }
            }
            fallback
        });
        let final_content = if content.ends_with('\n') {
            content
        } else {
            format!("{content}\n")
        };
        if let Some(parent) = target_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(&target_path, final_content.as_bytes())
            .map_err(|e| Error::new("Error", format!("{}: {e}", target_path.display())))?;
        let _ = domain::put(&mut self.display, "state", Value::string("passed"));
        let _ = domain::put(&mut self.display, "review", Value::string("pass"));
        self.redact_text(&format!(
            "Translated natural language -> Direct workspace file delivery\nWrote file: {}\n\n{}",
            target_path.display(),
            final_content
        ))
    }

    async fn run_ask_text(&mut self, text: &str) -> Result<String, Error> {
        self.run_ask_text_mode(text, false).await
    }

    async fn run_ask_text_mode(
        &mut self,
        text: &str,
        confirm_after_plan: bool,
    ) -> Result<String, Error> {
        let mut cleaned = crate::planning::intake::clean_ask(&Value::string(text))?;
        if is_workspace_inventory_ask(&cleaned)? {
            return self.execute_workspace_inventory(&cleaned).await;
        }
        if crate::ask::is_repository_question_ask(&cleaned)? {
            return self.run_question_text(&cleaned).await;
        }
        self.last_ask_text = Some(cleaned.clone());
        let root = self.current_root();
        let has_git = root.join(".git").exists();
        let mut classified =
            crate::planning::intake::classify(&Value::string(&cleaned), None, &[])?;
        let mut ask_kind = classified
            .get("kind")
            .and_then(Value::as_string)
            .map(Text::to_utf8)
            .transpose()?
            .unwrap_or_else(|| "slice".to_owned());

        let mut translation_banner = String::new();
        if ask_kind == "clarify" || !has_git {
            if let Some(translated) = self
                .translate_natural_language_with_llm(&cleaned, &root)
                .await
            {
                match translated {
                    NaturalLanguageTranslation::ShellCommand { command, .. } => {
                        if let ShellAction::Continue(out) =
                            Box::pin(self.dispatch_line(&command)).await?
                        {
                            return Ok(format!("Translated natural language -> {command}\n{out}"));
                        }
                    }
                    NaturalLanguageTranslation::WorkspaceInventory => {
                        return self.execute_workspace_inventory(&cleaned).await;
                    }
                    NaturalLanguageTranslation::DirectFileTask {
                        target_file,
                        summary,
                    } => {
                        return self
                            .execute_direct_workspace_file_task(
                                &cleaned,
                                Some(&target_file),
                                Some(&summary),
                            )
                            .await;
                    }
                    NaturalLanguageTranslation::Question { question, files } => {
                        let bullets = files
                            .iter()
                            .map(|f| format!("- {f}"))
                            .collect::<Vec<_>>()
                            .join("\n");
                        let scoped = format!("{question}\n\n## Files allowed\n{bullets}");
                        return self.run_question_text(&scoped).await;
                    }
                    NaturalLanguageTranslation::DeliveryTask {
                        structured_ask,
                        outcome,
                        files,
                        ..
                    } => {
                        translation_banner = format!(
                            "Translated natural language -> Task: {outcome} [files: {}]\n",
                            files.join(", ")
                        );
                        cleaned = structured_ask;
                        classified =
                            crate::planning::intake::classify(&Value::string(&cleaned), None, &[])?;
                        ask_kind = classified
                            .get("kind")
                            .and_then(Value::as_string)
                            .map(Text::to_utf8)
                            .transpose()?
                            .unwrap_or_else(|| "slice".to_owned());
                    }
                }
            } else if !has_git && is_direct_file_creation_request(&cleaned) {
                return self
                    .execute_direct_workspace_file_task(&cleaned, None, None)
                    .await;
            }
        }

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
            )
            .ok();
            let cp_id = cp
                .as_ref()
                .map_or_else(|| "ephemeral".to_owned(), |c| c.id.clone());
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
                checkpoint: cp,
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
        let mut outcome = {
            let mut observer = ShellLiveObserver::new(
                &mut self.display,
                &mut self.last_measured_seat,
                &mut self.last_finish_reason,
                &self.ask_queue,
                self.statusbar,
                self.debug.enabled(),
                self.color,
                self.live_output,
                &mut self.tray_visible,
                &mut self.bar_lines,
                self.last_issue,
            );
            match crate::ask::write_ask_with_observer(&cleaned, &mut write_opts, &mut observer)
                .await
            {
                Ok(out) => out,
                Err(err) => {
                    let _ = domain::put(&mut self.display, "state", Value::string("failed"));
                    return Err(err);
                }
            }
        };
        let mut extra_summary = String::new();
        if let crate::ask::AskOutcome::Draft {
            ref draft,
            ref mut command,
        } = outcome
        {
            self.last_ask_file = Some(draft.ask_path.clone());
            self.last_task = Some(format!("ask-{}", draft.id));
            if command.is_empty() {
                *command = format!(
                    "roster run --ask-file {} --runtime builtin --no-fetch",
                    crate::ask::quote_for_shell(&draft.ask_path.display().to_string())
                );
            }
            if let Some(usage) = &draft.usage {
                let mut meas = vec![
                    ("seat", Value::string("planner")),
                    (
                        "provider",
                        self.config
                            .get("llm")
                            .and_then(|l| l.get("provider"))
                            .cloned()
                            .unwrap_or_else(|| Value::string("vllm")),
                    ),
                    (
                        "model",
                        self.display.get("model").cloned().unwrap_or(Value::Null),
                    ),
                    (
                        "effort",
                        self.display.get("effort").cloned().unwrap_or(Value::Null),
                    ),
                    (
                        "contextMax",
                        self.display
                            .get("contextMax")
                            .cloned()
                            .unwrap_or(Value::Null),
                    ),
                    (
                        "finishReason",
                        self.display
                            .get("lastFinishReason")
                            .cloned()
                            .unwrap_or_else(|| Value::string("stop")),
                    ),
                ];
                if let Some(pt) = domain::safe_integer(usage.get("prompt_tokens")) {
                    let _ = domain::put(&mut self.display, "contextUsed", Value::Number(pt as f64));
                    meas.push(("input", Value::Number(pt as f64)));
                }
                if let Some(ct) = domain::safe_integer(usage.get("completion_tokens")) {
                    let _ =
                        domain::put(&mut self.display, "outputTokens", Value::Number(ct as f64));
                    meas.push(("output", Value::Number(ct as f64)));
                }
                self.last_measured_seat = Some(Value::object(meas));
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
            if let Some(dir) = draft
                .task_path
                .as_deref()
                .and_then(Path::parent)
                .or_else(|| draft.plan_path.as_deref().and_then(Path::parent))
                .or_else(|| draft.ask_path.parent())
            {
                self.last_worktree = Some(dir.to_path_buf());
            }
        }
        if confirm_after_plan {
            self.pending_confirm = true;
            let _ = domain::put(&mut self.display, "state", Value::string("planning"));
            return Ok(format!(
                "{translation_banner}{extra_summary}{}Plan ready. Press Enter to accept and continue, or /stop to keep the plan without coding.\n",
                crate::ask::format_ask(&outcome)
            ));
        }
        let plan_summary = format!(
            "{translation_banner}{extra_summary}{}",
            crate::ask::format_ask(&outcome)
        );
        if self.live_output && self.has_configured_endpoint() && self.last_ask_file.is_some() {
            match self.execute_local_ask_delivery().await {
                Ok(delivery_out) => {
                    return Ok(format!("{plan_summary}\n{delivery_out}"));
                }
                Err(err) => {
                    let _ = domain::put(&mut self.display, "state", Value::string("passed"));
                    return Ok(format!(
                        "{plan_summary}Local worktree execution skipped ({}); use /run after resolving or press Enter after /plan.\n",
                        err.message
                    ));
                }
            }
        }
        let _ = domain::put(&mut self.display, "state", Value::string("passed"));
        Ok(plan_summary)
    }

    async fn execute_local_ask_delivery(&mut self) -> Result<String, Error> {
        let Some(ask_file) = self.last_ask_file.clone() else {
            return Err(Error::new(
                "Error",
                "Plan or supply an Ask before running local delivery.",
            ));
        };
        let root = self.current_root();
        let env_with_tools = ensure_delivery_tool_env(&self.environment);
        let mut loaded = config::load(&self.installation, &root, &env_with_tools)?;
        if let Some(ref prof_id) = self.fleet_profile_id {
            let fleet_cat = crate::fleet::load_with_env(
                &root,
                Some(&self.installation),
                Some(&self.environment),
            )?;
            let prof = crate::fleet::get_profile(&fleet_cat, &Value::string(prof_id))?;
            loaded.config = loaded.config.with_profile(prof)?;
        }
        let exe = std::env::current_exe()
            .map_err(|_| Error::new("Error", "Cannot resolve the current native CLI image"))?;
        let Ok((host, approval)) = crate::host::invocation_host(&env_with_tools, &exe) else {
            let _ = domain::put(&mut self.display, "state", Value::string("passed"));
            return Ok(format!(
                "Ask artifacts ready at {}.\n",
                ask_file.parent().unwrap_or(&ask_file).display()
            ));
        };
        if !root.join(".git").exists() {
            let _ = domain::put(&mut self.display, "state", Value::string("passed"));
            return Ok(format!(
                "Ask artifacts ready at {}.\n",
                ask_file.parent().unwrap_or(&ask_file).display()
            ));
        }
        let start = crate::start::Options::resolve(
            &loaded.config,
            Some("current"),
            Some(crate::start::Sync::Offline),
        )?;
        let credentials = crate::delivery::native_invocation_credentials(&env_with_tools, &loaded)?;
        let mut transport =
            crate::llm::http::HttpTransport::new().map_err(|failure| *failure.error)?;
        let mut observer = ShellLiveObserver::new(
            &mut self.display,
            &mut self.last_measured_seat,
            &mut self.last_finish_reason,
            &self.ask_queue,
            self.statusbar,
            self.debug.enabled(),
            self.color,
            self.live_output,
            &mut self.tray_visible,
            &mut self.bar_lines,
            self.last_issue,
        );
        let admission = crate::llm::admission::Admission::default();
        let mut driver = crate::delivery::Driver {
            transport: &mut transport,
            observer: &mut observer,
            credentials: &credentials,
            admission: &admission,
        };
        let mut ownership = crate::delivery::Ownership::default();
        let source = crate::cancel::CancellationSource::default();
        let token = source.token();
        let result = driver
            .run_local(
                crate::delivery::Options {
                    cwd: &root,
                    installation: &self.installation,
                    loaded: &loaded,
                    input: crate::delivery::Input::AskFile(ask_file.clone()),
                    host,
                    approval,
                    start,
                    fetch: crate::start::FetchPolicy::Deny,
                    skip_review: false,
                },
                token,
                &mut ownership,
            )
            .await;
        match result {
            Ok(outcome) => {
                self.last_worktree = Some(outcome.worktree.clone());
                self.last_task = Some(outcome.task.clone());
                let passed = outcome.succeeded();
                let _ = domain::put(
                    &mut self.display,
                    "state",
                    Value::string(if passed { "passed" } else { "failed" }),
                );
                let rev_verdict = if passed {
                    "pass".to_owned()
                } else {
                    outcome
                        .reviewer_report
                        .as_ref()
                        .and_then(|r| r.get("verdict"))
                        .and_then(Value::as_string)
                        .and_then(|t| t.to_utf8().ok())
                        .unwrap_or_else(|| "fail".to_owned())
                };
                let _ = domain::put(&mut self.display, "review", Value::string(&rev_verdict));
                Ok(outcome.format())
            }
            Err(err) => {
                let _ = domain::put(&mut self.display, "state", Value::string("failed"));
                let safe_err = crate::delivery::safe_invocation_error(&err, &loaded, &credentials)
                    .unwrap_or(err);
                Err(safe_err)
            }
        }
    }
}

fn ensure_delivery_tool_env(env: &Value) -> Value {
    let mut next = env.clone();
    if next.get("ROSTER_GIT").is_none() {
        if let Some(p) = which_executable(if cfg!(windows) { "git.exe" } else { "git" }) {
            let _ = domain::put(&mut next, "ROSTER_GIT", Value::string(&p));
        }
    }
    if next.get("ROSTER_NODE20").is_none() {
        if let Some(p) = which_executable(if cfg!(windows) { "node.exe" } else { "node" }) {
            let _ = domain::put(&mut next, "ROSTER_NODE20", Value::string(&p));
        }
    }
    if next.get("ROSTER_CARGO").is_none() {
        if let Some(p) = which_executable(if cfg!(windows) { "cargo.exe" } else { "cargo" }) {
            let _ = domain::put(&mut next, "ROSTER_CARGO", Value::string(&p));
        }
    }
    next
}

fn which_executable(name: &str) -> Option<String> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate.display().to_string());
        }
    }
    None
}

struct ShellLiveObserver<'a> {
    display: &'a mut Value,
    last_measured_seat: &'a mut Option<Value>,
    last_finish_reason: &'a mut Option<String>,
    ask_queue: &'a [String],
    statusbar: bool,
    debug: bool,
    color: bool,
    live_output: bool,
    tray_visible: &'a mut bool,
    bar_lines: &'a mut usize,
    issue: Option<u64>,
    sink: EventSink,
    last_key: Option<String>,
    last_rows: usize,
    stream_acc: String,
}

impl<'a> ShellLiveObserver<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        display: &'a mut Value,
        last_measured_seat: &'a mut Option<Value>,
        last_finish_reason: &'a mut Option<String>,
        ask_queue: &'a [String],
        statusbar: bool,
        debug: bool,
        color: bool,
        live_output: bool,
        tray_visible: &'a mut bool,
        bar_lines: &'a mut usize,
        issue: Option<u64>,
    ) -> Self {
        Self {
            display,
            last_measured_seat,
            last_finish_reason,
            ask_queue,
            statusbar,
            debug,
            color,
            live_output,
            tray_visible,
            bar_lines,
            issue,
            sink: EventSink::new(200.0),
            last_key: None,
            last_rows: 0,
            stream_acc: String::new(),
        }
    }

    fn write_live(&mut self, text: &str, replace: bool, key: Option<&str>) {
        if !self.live_output {
            return;
        }
        let cols = terminal_columns();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
        let mut stdout = std::io::stdout();
        let was_visible = *self.tray_visible;
        if was_visible {
            let _ = stdout.write_all(b"\r\x1b[2K");
            if *self.bar_lines > 0 {
                let _ = stdout.write_all(format!("\x1b[{}A\r\x1b[J", *self.bar_lines).as_bytes());
            }
            *self.tray_visible = false;
        }
        if replace && was_visible && self.last_rows > 0 {
            let _ = stdout.write_all(format!("\x1b[{}A\r\x1b[J", self.last_rows).as_bytes());
        }
        let _ = stdout.write_all(text.as_bytes());
        let plain = strip_vt_only(text);
        let trimmed = plain.strip_suffix('\n').unwrap_or(&plain);
        self.last_rows = trimmed
            .split('\n')
            .map(|line| {
                let len = line.chars().count();
                if len == 0 {
                    1
                } else {
                    len.div_ceil(cols.max(1))
                }
            })
            .sum::<usize>()
            .max(1);
        self.last_key = key.map(str::to_owned);
        let (tray_block, prompt, new_bar_lines) = render_tray_block(
            self.display,
            self.ask_queue,
            self.statusbar,
            self.debug,
            cols,
            self.color,
            now_ms,
        );
        let _ = stdout.write_all(format!("{tray_block}{prompt}").as_bytes());
        let _ = stdout.flush();
        *self.bar_lines = new_bar_lines;
        *self.tray_visible = true;
    }

    fn repaint_rail(&mut self) {
        if !self.live_output {
            return;
        }
        let cols = terminal_columns();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
        let mut stdout = std::io::stdout();
        if *self.tray_visible {
            let _ = stdout.write_all(b"\r\x1b[2K");
            if *self.bar_lines > 0 {
                let _ = stdout.write_all(format!("\x1b[{}A\r\x1b[J", *self.bar_lines).as_bytes());
            }
        }
        let (tray_block, prompt, new_bar_lines) = render_tray_block(
            self.display,
            self.ask_queue,
            self.statusbar,
            self.debug,
            cols,
            self.color,
            now_ms,
        );
        let _ = stdout.write_all(format!("{tray_block}{prompt}").as_bytes());
        let _ = stdout.flush();
        *self.bar_lines = new_bar_lines;
        *self.tray_visible = true;
    }
}

impl crate::llm::control::Observer for ShellLiveObserver<'_> {
    async fn emit(&mut self, event: Value) -> Result<(), Error> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
        let kind = event
            .get("type")
            .and_then(Value::as_string)
            .and_then(|t| t.to_utf8().ok())
            .unwrap_or_default();
        match kind.as_str() {
            "seat-start" => {
                let seat = clean_display(event.get("seat"), "");
                let state = match seat.as_str() {
                    "planner" => "planning",
                    "coder" => "drafting",
                    "reviewer" => "reviewing",
                    _ => "planning",
                };
                let _ = domain::put(self.display, "state", Value::string(state));
                let _ = domain::put(self.display, "startedAt", Value::Number(now_ms));
                let _ = domain::put(self.display, "contextUsed", Value::Null);
                let _ = domain::put(self.display, "outputTokens", Value::Null);
                let _ = domain::put(self.display, "toolCount", Value::Number(0.0));
            }
            "tool" => {
                let count = domain::safe_integer(self.display.get("toolCount")).unwrap_or(0) + 1;
                let _ = domain::put(self.display, "toolCount", Value::Number(count as f64));
                if clean_display(event.get("name"), "") == "run_test" {
                    let _ = domain::put(self.display, "state", Value::string("testing"));
                    let _ = domain::put(self.display, "lastTestName", Value::string("node --test"));
                }
            }
            "http" => {
                if clean_display(event.get("phase"), "") == "start" {
                    if let Some(th) = event.get("thinking").cloned() {
                        let _ = domain::put(self.display, "thinking", th);
                    }
                    if let Some(mt) = domain::safe_integer(event.get("maxTokens")) {
                        let _ = domain::put(self.display, "maxTokens", Value::Number(mt as f64));
                    }
                }
            }
            "finish-reason" | "completion" => {
                let reason = clean_display(event.get("reason"), "");
                if !reason.is_empty() {
                    let _ = domain::put(self.display, "lastFinishReason", Value::string(&reason));
                    *self.last_finish_reason = Some(reason);
                }
            }
            "seat-measurement" => {
                *self.last_measured_seat = Some(event.clone());
                if let Some(inp) = domain::safe_integer(event.get("input")) {
                    let _ = domain::put(self.display, "contextUsed", Value::Number(inp as f64));
                }
                if let Some(out) = domain::safe_integer(event.get("output")) {
                    let _ = domain::put(self.display, "outputTokens", Value::Number(out as f64));
                }
                if let Some(cm) = domain::safe_integer(event.get("contextMax")) {
                    let _ = domain::put(self.display, "contextMax", Value::Number(cm as f64));
                }
            }
            "usage" => {
                if let Some(inp) = domain::safe_integer(event.get("input")) {
                    let _ = domain::put(self.display, "contextUsed", Value::Number(inp as f64));
                }
                if let Some(out) = domain::safe_integer(event.get("output")) {
                    let _ = domain::put(self.display, "outputTokens", Value::Number(out as f64));
                }
                if let Some(ca) = domain::safe_integer(event.get("cached")) {
                    let _ = domain::put(self.display, "cachedTokens", Value::Number(ca as f64));
                }
            }
            "seat-end" => {
                if let Some(inp) = domain::safe_integer(event.get("input")) {
                    let _ = domain::put(self.display, "contextUsed", Value::Number(inp as f64));
                }
                if let Some(out) = domain::safe_integer(event.get("output")) {
                    let _ = domain::put(self.display, "outputTokens", Value::Number(out as f64));
                }
                let verdict = clean_display(event.get("verdict"), "");
                if !verdict.is_empty() {
                    let _ = domain::put(
                        self.display,
                        "state",
                        Value::string(if verdict == "pass" {
                            "passed"
                        } else {
                            "failed"
                        }),
                    );
                    let _ = domain::put(self.display, "review", Value::string(&verdict));
                }
            }
            "seat-error" => {
                let _ = domain::put(self.display, "state", Value::string("failed"));
            }
            _ => {}
        }

        self.sink.receive(&event, self.issue, now_ms);
        for step in self.sink.take_emitted() {
            let skind = clean_display(step.get("kind"), "");
            match skind.as_str() {
                "phase" => {
                    self.stream_acc.clear();
                    let issue_part = match step
                        .get("issue")
                        .and_then(|v| domain::safe_integer(Some(v)))
                    {
                        Some(n) => format!("#{n}"),
                        None => "local".to_owned(),
                    };
                    let phase_part = clean_display(step.get("phase"), "");
                    let line = format!("{issue_part} {phase_part}");
                    let painted = ansi_paint(&line, "white", self.color);
                    self.write_live(&format!("{painted}\n"), false, Some("phase"));
                }
                "tool" => {
                    self.stream_acc.clear();
                    let name = clean_display(step.get("name"), "");
                    let target = clean_display(step.get("target"), "");
                    let count = domain::safe_integer(step.get("count")).unwrap_or(1);
                    let head = if target.is_empty() {
                        name.clone()
                    } else {
                        format!("{name} {target}")
                    };
                    let count_str = count.to_string();
                    let line = if count > 1 {
                        format_step(&[&head, &count_str])
                    } else {
                        format_step(&[&head])
                    };
                    let key = format!("tool:{name}:{target}");
                    let replace = self.last_key.as_deref() == Some(key.as_str());
                    let painted = ansi_paint(&line, "dim", self.color);
                    self.write_live(&format!("{painted}\n"), replace, Some(&key));
                }
                "delta" => {
                    let start = step.get("start") == Some(&Value::Boolean(true));
                    if start {
                        self.stream_acc.clear();
                    }
                    let text_chunk = step
                        .get("text")
                        .and_then(Value::as_string)
                        .and_then(|t| t.to_utf8().ok())
                        .unwrap_or_default();
                    self.stream_acc.push_str(&text_chunk);
                    let wrapped = wrap_narration(&self.stream_acc, terminal_columns()).join("\n");
                    if !wrapped.is_empty() {
                        let replace = !start && self.last_key.as_deref() == Some("stream");
                        let painted = ansi_paint(&wrapped, "white", self.color);
                        self.write_live(&format!("{painted}\n"), replace, Some("stream"));
                    }
                }
                "wait" => {
                    let secs = domain::safe_integer(step.get("seconds")).unwrap_or(0);
                    let secs_part = format!("{}s", secs);
                    let local = step.get("local") == Some(&Value::Boolean(true));
                    let line = if local {
                        format_step(&[
                            "waiting",
                            &secs_part,
                            "local hardware can take minutes after idle",
                        ])
                    } else {
                        format_step(&["waiting", &secs_part])
                    };
                    let replace = self.last_key.as_deref() == Some("wait");
                    let painted = ansi_paint(&line, "yellow", self.color);
                    self.write_live(&format!("{painted}\n"), replace, Some("wait"));
                }
                "verdict" => {
                    self.stream_acc.clear();
                    let issue_part = match step
                        .get("issue")
                        .and_then(|v| domain::safe_integer(Some(v)))
                    {
                        Some(n) => format!("#{n}"),
                        None => "local".to_owned(),
                    };
                    let phase_part = clean_display(step.get("phase"), "");
                    let head = if phase_part.is_empty() || phase_part == "-" {
                        issue_part
                    } else {
                        format!("{issue_part} {phase_part}")
                    };
                    let v = clean_display(step.get("verdict"), "");
                    let reason = clean_display(step.get("reason"), "");
                    let files_str = step
                        .get("files")
                        .and_then(domain::items)
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|i| i.as_string()?.to_utf8().ok())
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                        .unwrap_or_default();
                    let mut parts = vec![head.as_str(), v.as_str()];
                    if !reason.is_empty() && reason != "-" {
                        parts.push(reason.as_str());
                    }
                    if !files_str.is_empty() {
                        parts.push(files_str.as_str());
                    }
                    let line = format_step(&parts);
                    let style = if v == "pass" { "green" } else { "red" };
                    let painted = ansi_paint(&line, style, self.color);
                    self.write_live(&format!("{painted}\n"), false, None);
                }
                "usage" => {
                    self.repaint_rail();
                }
                _ => {}
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveEntry {
    pub mount: String,
    pub kind: String,
    pub total_gib: String,
    pub free_gib: String,
    pub used_pct: String,
    pub dev_roots: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoEntry {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub branch: String,
    pub state: String,
    pub remote: String,
    pub last_commit: String,
    pub modified_epoch: u64,
    pub modified_utc: String,
    pub linked_worktrees: Vec<String>,
}

fn format_gib(bytes: u64) -> String {
    let gib_tenths = (bytes as u128 * 10) / (1024 * 1024 * 1024);
    format!("{}.{} GiB", gib_tenths / 10, gib_tenths % 10)
}

pub fn is_workspace_inventory_ask(text: &str) -> Result<bool, Error> {
    let reqs = crate::planning::intake::requirements(&Value::string(text), true)?;
    let has_drive_terms = regex_lite::Regex::new(
        r"(?i)\b(?:dev\s+drives?|drives?|disks?|mounts?|volumes?|filesystems?)\b",
    )
    .map_err(|e| Error::new("Error", e.to_string()))?
    .is_match(text);
    let has_repo_plural = regex_lite::Regex::new(r"(?i)\b(?:repositories|repos|worktrees)\b")
        .map_err(|e| Error::new("Error", e.to_string()))?
        .is_match(text);
    let has_machine_context = regex_lite::Regex::new(
        r"(?i)\b(?:what|which|list|state|status|on\s+this|windows\s+pc|machine|pc|wsl|current\s+.*directory|current\s+folder)\b",
    )
    .map_err(|e| Error::new("Error", e.to_string()))?
    .is_match(text);
    if has_drive_terms && (has_repo_plural || has_machine_context) {
        return Ok(true);
    }
    if has_repo_plural && has_machine_context && reqs.files.is_empty() {
        return Ok(true);
    }
    Ok(false)
}

pub fn discover_drives(cwd: &Path, env: &Value) -> Vec<DriveEntry> {
    let mut entries = Vec::new();
    let home = config::environment_value(env, if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .ok()
        .flatten()
        .and_then(|t| t.to_utf8().ok())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
        });

    let push_mount = |entries: &mut Vec<DriveEntry>,
                      mount_path: &Path,
                      label: String,
                      kind: &str,
                      candidate_subdirs: &[PathBuf]| {
        let Ok(total) = fs4::total_space(mount_path) else {
            return;
        };
        if total == 0 {
            return;
        }
        let free = fs4::available_space(mount_path).unwrap_or(0);
        let used = total.saturating_sub(free);
        let pct = ((used as u128 * 100) / (total as u128).max(1)) as u64;
        let mut dev_roots = Vec::new();
        for sub in candidate_subdirs {
            if sub.is_dir() {
                dev_roots.push(sub.display().to_string());
            }
        }
        entries.push(DriveEntry {
            mount: label,
            kind: kind.to_owned(),
            total_gib: format_gib(total),
            free_gib: format_gib(free),
            used_pct: format!("{pct}%"),
            dev_roots,
        });
    };

    if cfg!(windows) {
        for letter in b'A'..=b'Z' {
            let root_str = format!("{}:\\", letter as char);
            let root_path = PathBuf::from(&root_str);
            if !root_path.is_dir() {
                continue;
            }
            let mut candidates = vec![
                root_path.join("oss"),
                root_path.join("repos"),
                root_path.join("dev"),
                root_path.join("src"),
                root_path.join("projects"),
            ];
            if let Some(ref h) = home {
                if h.starts_with(&root_path) {
                    candidates.push(h.join("repos"));
                    candidates.push(h.join("source").join("repos"));
                    candidates.push(h.join(".roster").join("installation"));
                }
            }
            let kind = if root_path.join("oss").is_dir() || root_path.join("repos").is_dir() {
                "Windows Volume (Dev Workspace)"
            } else {
                "Windows Volume"
            };
            push_mount(&mut entries, &root_path, root_str, kind, &candidates);
        }
    } else {
        let root_path = Path::new("/");
        let mut root_candidates = Vec::new();
        if let Some(ref h) = home {
            root_candidates.push(h.join("repos"));
            root_candidates.push(h.join("src"));
            root_candidates.push(h.join("projects"));
            root_candidates.push(h.join(".roster").join("installation"));
        }
        let is_wsl = Path::new("/proc/sys/fs/binfmt_misc/WSLInterop").exists()
            || std::env::var_os("WSL_DISTRO_NAME").is_some();
        push_mount(
            &mut entries,
            root_path,
            "/ (Linux root)".to_owned(),
            if is_wsl {
                "WSL 2 Virtual Disk (ext4)"
            } else {
                "POSIX Root Filesystem"
            },
            &root_candidates,
        );
        for letter in b'a'..=b'z' {
            let mnt_str = format!("/mnt/{}", letter as char);
            let mnt_path = PathBuf::from(&mnt_str);
            if !mnt_path.is_dir() {
                continue;
            }
            let candidates = vec![
                mnt_path.join("oss"),
                mnt_path.join("repos"),
                mnt_path.join("dev"),
                mnt_path.join("projects"),
            ];
            let label = format!("{mnt_str} ({}:\\)", (letter as char).to_ascii_uppercase());
            push_mount(
                &mut entries,
                &mnt_path,
                label,
                "Windows Host Drive (WSL mount)",
                &candidates,
            );
        }
    }

    if entries.is_empty() {
        let candidates = vec![cwd.to_path_buf()];
        push_mount(
            &mut entries,
            cwd,
            cwd.display().to_string(),
            "Workspace Volume",
            &candidates,
        );
    }
    entries
}

fn inspect_git_repo(path: &Path, is_worktree: bool) -> RepoEntry {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("repo")
        .to_owned();
    let kind = if is_worktree {
        "Worktree"
    } else {
        "Repository"
    }
    .to_owned();

    let mut latest_mtime = 0_u64;
    if let Ok(meta) = fs::metadata(path) {
        if let Ok(modified) = meta.modified() {
            if let Ok(dur) = modified.duration_since(std::time::UNIX_EPOCH) {
                latest_mtime = latest_mtime.max(dur.as_secs());
            }
        }
    }
    if let Ok(rd) = fs::read_dir(path) {
        for entry in rd.flatten().take(48) {
            if entry.file_name() == ".git" {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                if let Ok(modified) = meta.modified() {
                    if let Ok(dur) = modified.duration_since(std::time::UNIX_EPOCH) {
                        latest_mtime = latest_mtime.max(dur.as_secs());
                    }
                }
            }
        }
    }

    let run_git = |args: &[&str]| -> Option<String> {
        let out = StdCommand::new("git")
            .arg("--no-optional-locks")
            .arg("-c")
            .arg("core.autocrlf=false")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        String::from_utf8(out.stdout)
            .ok()
            .map(|s| s.trim().to_owned())
    };
    let branch = run_git(&["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_else(|| {
        if is_worktree {
            "linked-worktree".to_owned()
        } else {
            "-".to_owned()
        }
    });
    let state = match run_git(&["status", "--porcelain=v1"]) {
        Some(s) if s.is_empty() => "clean".to_owned(),
        Some(s) => {
            let mut modified = 0_usize;
            let mut untracked = 0_usize;
            for line in s.lines() {
                if line.starts_with("??") {
                    untracked += 1;
                } else if !line.trim().is_empty() {
                    modified += 1;
                }
            }
            match (modified, untracked) {
                (0, u) => format!("dirty ({u} untracked)"),
                (m, 0) => format!("dirty ({m} modified)"),
                (m, u) => format!("dirty ({m} modified, {u} untracked)"),
            }
        }
        None => "cross-host worktree".to_owned(),
    };
    let remote =
        run_git(&["config", "--get", "remote.origin.url"]).unwrap_or_else(|| "-".to_owned());
    let last_commit =
        run_git(&["log", "-1", "--format=%h (%cr) %s"]).unwrap_or_else(|| "-".to_owned());
    let commit_epoch = run_git(&["log", "-1", "--format=%ct"])
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    latest_mtime = latest_mtime.max(commit_epoch);
    let modified_utc = time::OffsetDateTime::from_unix_timestamp(latest_mtime as i64)
        .map(|dt| {
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
                dt.year(),
                dt.month() as u8,
                dt.day(),
                dt.hour(),
                dt.minute(),
                dt.second()
            )
        })
        .unwrap_or_else(|_| "-".to_owned());

    let mut linked_worktrees = Vec::new();
    if !is_worktree {
        if let Some(wt_out) = run_git(&["worktree", "list", "--porcelain"]) {
            let norm_self = path.display().to_string().replace('\\', "/");
            for line in wt_out.lines() {
                if let Some(wt_path) = line.strip_prefix("worktree ") {
                    let trimmed = wt_path.trim();
                    let norm_wt = trimmed.replace('\\', "/");
                    if !norm_wt.eq_ignore_ascii_case(&norm_self) {
                        linked_worktrees.push(trimmed.to_owned());
                    }
                }
            }
        }
    }

    RepoEntry {
        name,
        path: path.display().to_string(),
        kind,
        branch,
        state,
        remote,
        last_commit,
        modified_epoch: latest_mtime,
        modified_utc,
        linked_worktrees,
    }
}

pub fn discover_repositories(cwd: &Path, env: &Value) -> Vec<RepoEntry> {
    let mut scan_roots = Vec::new();
    let mut push_root = |p: PathBuf| {
        if p.is_dir() && !scan_roots.iter().any(|existing: &PathBuf| existing == &p) {
            scan_roots.push(p);
        }
    };
    push_root(cwd.to_path_buf());
    let home = config::environment_value(env, if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .ok()
        .flatten()
        .and_then(|t| t.to_utf8().ok())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
        });
    if let Some(ref h) = home {
        push_root(h.join("repos"));
        push_root(h.join("source").join("repos"));
        push_root(h.join("projects"));
        push_root(h.join("dev"));
        push_root(h.join("workspace"));
        push_root(h.join(".roster").join("installation"));
    }
    if cfg!(windows) {
        for letter in b'C'..=b'Z' {
            let root = PathBuf::from(format!("{}:\\", letter as char));
            if root.is_dir() {
                push_root(root.join("oss"));
                push_root(root.join("repos"));
                push_root(root.join("dev"));
                push_root(root.join("projects"));
            }
        }
    } else {
        for letter in b'c'..=b'z' {
            let mnt = PathBuf::from(format!("/mnt/{}", letter as char));
            if mnt.is_dir() {
                push_root(mnt.join("oss"));
                push_root(mnt.join("repos"));
                push_root(mnt.join("dev"));
                push_root(mnt.join("projects"));
            }
        }
    }

    let skip_names = [
        "AppData",
        "Application Data",
        "Local Settings",
        "Windows",
        "Program Files",
        "Program Files (x86)",
        "ProgramData",
        "$Recycle.Bin",
        "System Volume Information",
        "node_modules",
        "target",
        ".cargo",
        ".rustup",
        ".nvm",
        ".local",
        ".cache",
        ".dotnet",
        ".vscode",
        ".vscode-insiders",
        ".copilot",
        ".hermes",
        "vendor",
    ];

    let mut seen_paths = std::collections::BTreeSet::new();
    let mut repos = Vec::new();

    fn walk_dir(
        dir: &Path,
        depth: usize,
        max_depth: usize,
        skip_names: &[&str],
        seen: &mut std::collections::BTreeSet<String>,
        repos: &mut Vec<RepoEntry>,
    ) {
        if repos.len() >= 64 {
            return;
        }
        let git_marker = dir.join(".git");
        if let Ok(meta) = fs::symlink_metadata(&git_marker) {
            let canonical_key = dir.display().to_string();
            if seen.insert(canonical_key) {
                repos.push(inspect_git_repo(dir, meta.is_file()));
            }
            let wt_container = dir.join(".worktrees");
            if wt_container.is_dir() {
                if let Ok(rd) = fs::read_dir(&wt_container) {
                    for entry in rd.flatten() {
                        let p = entry.path();
                        if p.join(".git").exists() {
                            let k = p.display().to_string();
                            if seen.insert(k) {
                                repos.push(inspect_git_repo(&p, true));
                            }
                        }
                    }
                }
            }
            return;
        }
        if depth >= max_depth {
            return;
        }
        let Ok(rd) = fs::read_dir(dir) else {
            return;
        };
        let mut children = rd.flatten().map(|e| e.path()).collect::<Vec<_>>();
        children.sort();
        for child in children {
            let Some(name) = child.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if skip_names.iter().any(|s| s.eq_ignore_ascii_case(name)) {
                continue;
            }
            if name.starts_with('.') && name != ".roster" && !name.ends_with(".worktrees") {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(&child) else {
                continue;
            };
            if meta.is_dir() && !secure::linked(&meta) {
                walk_dir(&child, depth + 1, max_depth, skip_names, seen, repos);
            }
        }
    }

    for root in scan_roots {
        let max_d = if home.as_ref().is_some_and(|h| h == &root) {
            1
        } else {
            2
        };
        walk_dir(&root, 0, max_d, &skip_names, &mut seen_paths, &mut repos);
    }
    repos
}

pub fn resolve_inventory_target_path(text: &str, cwd: &Path, env: &Value) -> PathBuf {
    let home = config::environment_value(env, if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .ok()
        .flatten()
        .and_then(|t| t.to_utf8().ok())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
        });

    let filename = regex_lite::Regex::new(r"\b([A-Za-z0-9_.-]+\.md)\b")
        .ok()
        .and_then(|re| re.captures(text))
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_owned())
        .unwrap_or_else(|| "DEV_DRIVES_AND_REPOSITORIES.md".to_owned());

    if let Some(ref h) = home {
        if let Ok(re) = regex_lite::Regex::new(r"(?:^|\s)~(?:/([A-Za-z0-9_./-]+))?") {
            if let Some(caps) = re.captures(text) {
                let candidate = match caps.get(1) {
                    Some(sub) => h.join(sub.as_str().trim_end_matches(['.', ',', ';'])),
                    None => h.clone(),
                };
                if candidate.is_dir() {
                    return candidate.join(&filename);
                }
            }
        }
    }

    if cfg!(windows) {
        if let Ok(re) = regex_lite::Regex::new(r"([A-Za-z]:\\[A-Za-z0-9_.\\-]+)") {
            for caps in re.captures_iter(text) {
                if let Some(m) = caps.get(1) {
                    let p = PathBuf::from(m.as_str().trim_end_matches(['.', ',', ';']));
                    if p.is_dir() {
                        return p.join(&filename);
                    }
                }
            }
        }
    } else if let Ok(re) = regex_lite::Regex::new(r"(/(?:home|mnt|tmp|var|opt)/[A-Za-z0-9_./-]+)") {
        for caps in re.captures_iter(text) {
            if let Some(m) = caps.get(1) {
                let p = PathBuf::from(m.as_str().trim_end_matches(['.', ',', ';']));
                if p.is_dir() {
                    return p.join(&filename);
                }
            }
        }
    }

    cwd.join(filename)
}

pub fn render_workspace_inventory_markdown(
    ask_text: &str,
    cwd: &Path,
    drives: &[DriveEntry],
    repos: &[RepoEntry],
    llm_summary: Option<&str>,
) -> String {
    let platform = if cfg!(windows) {
        "Windows"
    } else if Path::new("/proc/sys/fs/binfmt_misc/WSLInterop").exists()
        || std::env::var_os("WSL_DISTRO_NAME").is_some()
    {
        "WSL 2 (Linux)"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else {
        "Linux"
    };
    let mut md = String::new();
    md.push_str("# Development Drives & Git Repositories Inventory\n\n");
    md.push_str(&format!("- **Environment**: {platform}\n"));
    md.push_str(&format!("- **Working Directory**: `{}`\n", cwd.display()));
    md.push_str(&format!(
        "- **Discovered Drives / Mounts**: {}\n",
        drives.len()
    ));
    md.push_str(&format!(
        "- **Discovered Git Repositories & Worktrees**: {}\n\n",
        repos.len()
    ));

    if let Some(summary) = llm_summary {
        md.push_str("## Executive Summary\n\n");
        md.push_str(summary);
        md.push_str("\n\n");
    } else {
        md.push_str("## Overview\n\n");
        md.push_str(&format!(
            "Scanned development drives and workspace roots for: _{}_\n\n",
            ask_text.replace('\n', " ")
        ));
    }

    md.push_str("## Development Drives & Filesystem Mounts\n\n");
    md.push_str(
        "| Mount / Drive | Type | Total Capacity | Free Space | Used | Active Dev Roots |\n",
    );
    md.push_str("| --- | --- | --- | --- | --- | --- |\n");
    for d in drives {
        let roots = if d.dev_roots.is_empty() {
            "-".to_owned()
        } else {
            d.dev_roots
                .iter()
                .map(|r| format!("`{r}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        md.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} |\n",
            d.mount, d.kind, d.total_gib, d.free_gib, d.used_pct, roots
        ));
    }
    md.push('\n');

    md.push_str("## Git Repositories & Worktrees\n\n");
    if repos.is_empty() {
        md.push_str("_No Git repositories found in the scanned roots._\n");
    } else {
        md.push_str("| Repository | Kind | Branch | State | Last Modified (UTC) | Latest Commit | Path | Remote |\n");
        md.push_str("| --- | --- | --- | --- | --- | --- | --- | --- |\n");
        for r in repos {
            let commit_clean = r.last_commit.replace('|', "/");
            md.push_str(&format!(
                "| **{}** | {} | `{}` | {} | `{}` | {} | `{}` | `{}` |\n",
                r.name, r.kind, r.branch, r.state, r.modified_utc, commit_clean, r.path, r.remote
            ));
        }
    }
    md
}

pub fn render_targeted_workspace_answer(
    ask_text: &str,
    cwd: &Path,
    drives: &[DriveEntry],
    repos: &[RepoEntry],
) -> String {
    let lower = ask_text.to_lowercase();
    let mut filtered = repos.to_vec();

    if let Some(caps) = regex_lite::Regex::new(r"(?i)\b([a-z])\s*(?:drive|:\\)\b")
        .ok()
        .and_then(|re| re.captures(ask_text))
    {
        if let Some(letter) = caps.get(1).and_then(|m| m.as_str().chars().next()) {
            let win_prefix = format!("{}:\\", letter.to_ascii_uppercase());
            let wsl_prefix = format!("/mnt/{}/", letter.to_ascii_lowercase());
            let drive_matches = filtered
                .iter()
                .filter(|r| {
                    r.path
                        .to_uppercase()
                        .starts_with(&win_prefix.to_uppercase())
                        || r.path.to_lowercase().starts_with(&wsl_prefix)
                })
                .cloned()
                .collect::<Vec<_>>();
            if !drive_matches.is_empty() {
                filtered = drive_matches;
            }
        }
    } else if lower.contains("this wsl directory") || lower.contains("current directory") {
        let cwd_prefix = cwd.display().to_string();
        let cwd_matches = filtered
            .iter()
            .filter(|r| r.path.starts_with(&cwd_prefix))
            .cloned()
            .collect::<Vec<_>>();
        if !cwd_matches.is_empty() {
            filtered = cwd_matches;
        }
    }

    let wants_most_recent_single = regex_lite::Regex::new(
        r"(?i)\b(?:which|what)\b.*\b(?:most\s+recently|latest)\b.*\b(?:modif\w*|updated|changed|touched)\b|\b(?:modif\w*|updated|changed)\s+most\s+recently\b",
    )
    .is_ok_and(|re| re.is_match(ask_text))
        && !lower.contains("list out")
        && !lower.contains("descending");

    let wants_sorted_recent = regex_lite::Regex::new(
        r"(?i)\b(?:descending|most\s+recently\s+modif\w*|order\s+of\s+.*modif\w*|by\s+.*modif\w*)\b",
    )
    .is_ok_and(|re| re.is_match(ask_text));

    let wants_worktrees = regex_lite::Regex::new(r"(?i)\b(?:working\s+trees?|worktrees?)\b")
        .is_ok_and(|re| re.is_match(ask_text));

    filtered.sort_by_key(|a| std::cmp::Reverse(a.modified_epoch));

    if wants_most_recent_single {
        if let Some(top) = filtered.first() {
            return format!(
                "Most recently modified repository directory:\n- **{}** (`{}`)\n  - **Last Modified**: `{}`\n  - **Kind**: {} | **Branch**: `{}` | **State**: {}\n  - **Latest Commit**: {}\n",
                top.name,
                top.path,
                top.modified_utc,
                top.kind,
                top.branch,
                top.state,
                top.last_commit
            );
        }
    }

    if wants_sorted_recent {
        let mut out = String::from(
            "Repositories in descending order of most recently modified:\n\n| # | Repository | Kind | Last Modified (UTC) | Branch | State | Path |\n| --- | --- | --- | --- | --- | --- | --- |\n",
        );
        for (idx, r) in filtered.iter().enumerate() {
            out.push_str(&format!(
                "| {} | **{}** | {} | `{}` | `{}` | {} | `{}` |\n",
                idx + 1,
                r.name,
                r.kind,
                r.modified_utc,
                r.branch,
                r.state,
                r.path
            ));
        }
        return out;
    }

    if wants_worktrees {
        let mut out = format!("Repositories and Working Trees in `{}`:\n\n", cwd.display());
        for r in &filtered {
            if r.kind == "Repository" {
                let wt_info = if r.linked_worktrees.is_empty() {
                    "no linked worktrees".to_owned()
                } else {
                    format!(
                        "{} linked worktree(s): {}",
                        r.linked_worktrees.len(),
                        r.linked_worktrees
                            .iter()
                            .map(|w| format!("`{w}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                out.push_str(&format!(
                    "- **{}** (`{}`) — branch `{}` ({}) — {}\n",
                    r.name, r.path, r.branch, r.state, wt_info
                ));
            } else {
                out.push_str(&format!(
                    "- **{}** [Worktree] (`{}`) — branch `{}` ({})\n",
                    r.name, r.path, r.branch, r.state
                ));
            }
        }
        return out;
    }

    let mut out = String::new();
    out.push_str("| Repository | Kind | Branch | State | Last Modified (UTC) | Path |\n");
    out.push_str("| --- | --- | --- | --- | --- | --- |\n");
    for r in &filtered {
        out.push_str(&format!(
            "| **{}** | {} | `{}` | {} | `{}` | `{}` |\n",
            r.name, r.kind, r.branch, r.state, r.modified_utc, r.path
        ));
    }
    if filtered.is_empty() && !drives.is_empty() {
        out.push_str("\nNo repositories matched the filter.\n");
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NaturalLanguageTranslation {
    ShellCommand {
        command: String,
        reason: String,
    },
    WorkspaceInventory,
    DirectFileTask {
        target_file: String,
        summary: String,
    },
    Question {
        question: String,
        files: Vec<String>,
    },
    DeliveryTask {
        structured_ask: String,
        outcome: String,
        files: Vec<String>,
        checks: Vec<String>,
    },
}

pub fn translate_natural_language_command(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.starts_with('/') || trimmed.contains('\n') {
        return None;
    }
    let has_file_creation = regex_lite::Regex::new(
        r"(?i)\b(?:make|write|create|generate|save|place|put|update|edit|modify|refactor|fix|implement)\b.*\b(?:file|markdown|\.md\b|\.rs\b|\.mjs\b|\.js\b|\.ts\b|code|readme)\b",
    )
    .ok()?
    .is_match(trimmed);
    if has_file_creation {
        return None;
    }

    let patterns: &[(&str, &str)] = &[
        (
            r"(?i)^(?:please\s+|can\s+you\s+|could\s+you\s+)*(?:show|list|display|view|what\s+are|what\s+models\s+are\s+in)\s+(?:me\s+)?(?:my\s+|the\s+|current\s+|configured\s+|available\s+)*fleet(?:\s+(?:models|profiles|list|setup|catalog))?[?.!]*$",
            "/fleet list",
        ),
        (
            r"(?i)^(?:please\s+|can\s+you\s+)*(?:discover|scan\s+for)\s+(?:local\s+|lan\s+|wsl\s+)*(?:fleet|model|llm)\s+endpoints?[?.!]*$",
            "/fleet discover",
        ),
        (
            r"(?i)^(?:please\s+|can\s+you\s+)*(?:run\s+)?(?:a\s+|an\s+)?(?:openshell\s+(?:doctor|health\s*check|readiness|check)|check\s+openshell(?:\s+status|\s+readiness)?)[?.!]*$",
            "/doctor openshell",
        ),
        (
            r"(?i)^(?:please\s+|can\s+you\s+)*(?:run\s+)?(?:a\s+|an\s+)?(?:copilot\s+(?:doctor|health\s*check|readiness|check)|check\s+copilot(?:\s+status|\s+readiness)?)[?.!]*$",
            "/doctor copilot",
        ),
        (
            r"(?i)^(?:please\s+|can\s+you\s+)*(?:run\s+)?(?:a\s+|an\s+)?(?:doctor|health\s*check|prerequisite\s+check|system\s+check|check\s+prerequisites)[?.!]*$",
            "/doctor",
        ),
        (
            r"(?i)^(?:please\s+|can\s+you\s+)*(?:show|check|what\s+is)\s+(?:me\s+)?(?:the\s+|my\s+|current\s+|session\s+|run\s+)*status[?.!]*$",
            "/status",
        ),
        (
            r"(?i)^(?:please\s+|can\s+you\s+)*(?:show|check|what\s+is)\s+(?:me\s+)?(?:the\s+|my\s+|current\s+)*(?:token\s+usage|context\s+usage|usage)[?.!]*$",
            "/usage",
        ),
        (
            r"(?i)^(?:please\s+|can\s+you\s+)*(?:show|list|check|what\s+are)\s+(?:me\s+)?(?:the\s+|open\s+|github\s+)*issues[?.!]*$",
            "/issues",
        ),
        (
            r"(?i)^(?:please\s+|can\s+you\s+)*(?:show|generate|write|print)\s+(?:me\s+)?(?:the\s+|a\s+)*(?:repo|repository)\s+map[?.!]*$",
            "/map",
        ),
        (
            r"(?i)^(?:please\s+|can\s+you\s+)*(?:show|list|view)\s+(?:me\s+)?(?:the\s+|my\s+|command\s+)*history[?.!]*$",
            "/history",
        ),
        (
            r"(?i)^(?:please\s+|can\s+you\s+)*(?:show|view|check)\s+(?:me\s+)?(?:the\s+|my\s+|delivery\s+)*(?:stats|statistics|metrics)[?.!]*$",
            "/stats",
        ),
        (
            r"(?i)^(?:please\s+|can\s+you\s+)*(?:check\s+for\s+(?:roster\s+)?updates?|is\s+roster\s+up\s+to\s+date)[?.!]*$",
            "/update --check",
        ),
        (
            r"(?i)^(?:please\s+|can\s+you\s+)*(?:update|upgrade)\s+roster[?.!]*$",
            "/update",
        ),
    ];
    for (pat, cmd) in patterns {
        if regex_lite::Regex::new(pat).ok()?.is_match(trimmed) {
            return Some((*cmd).to_owned());
        }
    }
    if let Some(caps) = regex_lite::Regex::new(
        r"(?i)^(?:please\s+)?(?:switch\s+to|use|select)\s+(?:fleet\s+|profile\s+|model\s+)?([a-z0-9][a-z0-9._-]{1,63})[?.!]*$",
    )
    .ok()?
    .captures(trimmed)
    {
        if let Some(id) = caps.get(1).map(|m| m.as_str()) {
            if !matches!(
                id.to_lowercase().as_str(),
                "it" | "this" | "that" | "them" | "default"
            ) || id.eq_ignore_ascii_case("default")
            {
                return Some(format!("/fleet use {id}"));
            }
        }
    }
    None
}

fn is_direct_file_creation_request(text: &str) -> bool {
    let has_verb = regex_lite::Regex::new(
        r"(?i)\b(?:make|write|create|generate|save|place|put|produce|export|draft)\b",
    )
    .is_ok_and(|re| re.is_match(text));
    let has_noun = regex_lite::Regex::new(
        r"(?i)\b(?:markdown|\.md\b|file|report|document|checklist|guide|summary|notes?|txt|json|yaml|yml)\b",
    )
    .is_ok_and(|re| re.is_match(text));
    has_verb && has_noun
}

fn collect_workspace_file_map(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let skip_dirs = [
        ".git",
        ".roster",
        ".roster-state",
        ".worktrees",
        "target",
        "node_modules",
        "vendor",
        "AppData",
        ".cargo",
        ".rustup",
        ".nvm",
    ];
    fn walk_rel(base: &Path, dir: &Path, depth: usize, skip_dirs: &[&str], out: &mut Vec<String>) {
        if out.len() >= 48 || depth > 2 {
            return;
        }
        let Ok(rd) = fs::read_dir(dir) else {
            return;
        };
        let mut entries = rd.flatten().map(|e| e.path()).collect::<Vec<_>>();
        entries.sort();
        for p in entries {
            if out.len() >= 48 {
                break;
            }
            let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if name.starts_with('.') || skip_dirs.iter().any(|s| s.eq_ignore_ascii_case(name)) {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(&p) else {
                continue;
            };
            if secure::linked(&meta) {
                continue;
            }
            if meta.is_file() {
                if let Ok(rel) = p.strip_prefix(base) {
                    let norm = rel.to_string_lossy().replace('\\', "/");
                    if !crate::planning::forbidden_read(&norm)
                        && !crate::execution::policy::managed(&norm)
                        && crate::execution::policy::scope_expansion_file(&norm)
                    {
                        out.push(norm);
                    }
                }
            } else if meta.is_dir() {
                walk_rel(base, &p, depth + 1, skip_dirs, out);
            }
        }
    }
    walk_rel(root, root, 0, &skip_dirs, &mut files);
    files
}

pub fn parse_natural_language_translation_json(
    raw: &str,
    original_ask: &str,
    has_git: bool,
    workspace_files: &[String],
) -> Option<NaturalLanguageTranslation> {
    let trimmed = raw.trim();
    let json_slice = if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if start < end {
            &trimmed[start..=end]
        } else {
            return None;
        }
    } else {
        return None;
    };
    let parsed = crate::json::parse(&Text::from(json_slice)).ok()?;
    let intent = parsed
        .get("intent")
        .and_then(Value::as_string)
        .and_then(|t| t.to_utf8().ok())
        .unwrap_or_default();
    match intent.as_str() {
        "command" => {
            let cmd = parsed
                .get("command")
                .and_then(Value::as_string)
                .and_then(|t| t.to_utf8().ok())?
                .trim()
                .to_owned();
            if !cmd.starts_with('/') {
                return None;
            }
            let cmd_name = cmd[1..].split_whitespace().next().unwrap_or("");
            let known = commands()
                .iter()
                .any(|c| c.name == cmd_name || c.aliases.contains(&cmd_name));
            if !known {
                return None;
            }
            Some(NaturalLanguageTranslation::ShellCommand {
                command: cmd,
                reason: "LLM command translation".to_owned(),
            })
        }
        "workspace_inventory" => Some(NaturalLanguageTranslation::WorkspaceInventory),
        "direct_file_task" => {
            let target_file = parsed
                .get("target_file")
                .and_then(Value::as_string)
                .and_then(|t| t.to_utf8().ok())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "OUTPUT.md".to_owned());
            let summary = parsed
                .get("outcome")
                .and_then(Value::as_string)
                .and_then(|t| t.to_utf8().ok())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| original_ask.to_owned());
            Some(NaturalLanguageTranslation::DirectFileTask {
                target_file,
                summary,
            })
        }
        "question" => {
            let mut files = parsed
                .get("files")
                .and_then(domain::items)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_string()?.to_utf8().ok())
                        .filter(|f| {
                            crate::planning::task::allowed_file(&Value::string(f)).is_ok()
                                && !crate::planning::forbidden_read(f)
                                && !crate::execution::policy::managed(f)
                        })
                        .take(6)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if files.is_empty() {
                files = workspace_files.iter().take(4).cloned().collect();
            }
            if files.is_empty() {
                return None;
            }
            Some(NaturalLanguageTranslation::Question {
                question: original_ask.to_owned(),
                files,
            })
        }
        "delivery_task" | "task" => {
            let outcome = parsed
                .get("outcome")
                .and_then(Value::as_string)
                .and_then(|t| t.to_utf8().ok())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| {
                    original_ask
                        .lines()
                        .next()
                        .unwrap_or(original_ask)
                        .to_owned()
                });
            let target_file = parsed
                .get("target_file")
                .and_then(Value::as_string)
                .and_then(|t| t.to_utf8().ok())
                .filter(|s| !s.trim().is_empty());
            if !has_git {
                return Some(NaturalLanguageTranslation::DirectFileTask {
                    target_file: target_file.unwrap_or_else(|| "OUTPUT.md".to_owned()),
                    summary: outcome,
                });
            }
            let mut files = parsed
                .get("files")
                .and_then(domain::items)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_string()?.to_utf8().ok())
                        .filter_map(|f| {
                            crate::planning::task::allowed_file(&Value::string(&f)).ok()
                        })
                        .take(8)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if files.is_empty() {
                if let Some(ref tf) = target_file {
                    if let Ok(valid) = crate::planning::task::allowed_file(&Value::string(tf)) {
                        files.push(valid);
                    }
                }
            }
            if files.is_empty() {
                if workspace_files.iter().any(|f| f == "README.md") {
                    files.push("README.md".to_owned());
                } else if let Some(first) = workspace_files.first() {
                    files.push(first.clone());
                }
            }
            if files.is_empty() {
                return None;
            }
            let mut checks = parsed
                .get("checks")
                .and_then(domain::items)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_string()?.to_utf8().ok())
                        .filter(|c| !c.trim().is_empty())
                        .take(4)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if checks.is_empty() {
                checks.push(format!("{} is updated to satisfy: {}", files[0], outcome));
            }
            let file_bullets = files
                .iter()
                .map(|f| format!("- `{f}`"))
                .collect::<Vec<_>>()
                .join("\n");
            let check_bullets = checks
                .iter()
                .map(|c| format!("- {c}"))
                .collect::<Vec<_>>()
                .join("\n");
            let structured_ask = format!(
                "{outcome}\n\nOriginal request: {original_ask}\n\n## Allowed Files\n{file_bullets}\n\n## Acceptance checks\n{check_bullets}"
            );
            Some(NaturalLanguageTranslation::DeliveryTask {
                structured_ask,
                outcome,
                files,
                checks,
            })
        }
        _ => None,
    }
}

pub fn run_self_update(
    installation: &Path,
    environment: &Value,
    check_only: bool,
) -> Result<String, Error> {
    let install_dir =
        if installation.join("Cargo.toml").is_file() && installation.join(".git").exists() {
            installation.to_path_buf()
        } else {
            config::default_installation_root(environment)?
        };
    if !install_dir.join("Cargo.toml").is_file() || !install_dir.join(".git").exists() {
        return Err(Error::new(
            "Error",
            format!(
                "Roster installation directory {} is not a complete Git source checkout; run install.sh or install.ps1 first.",
                install_dir.display()
            ),
        ));
    }
    let git_bin = config::environment_value(environment, "ROSTER_GIT")
        .ok()
        .flatten()
        .and_then(|t| t.to_utf8().ok())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "git".to_owned());

    let run_git = |dir: &Path, args: &[&str]| -> Result<String, Error> {
        let out = StdCommand::new(&git_bin)
            .arg("-c")
            .arg("core.autocrlf=false")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .map_err(|e| Error::new("Error", format!("git {}: {e}", args.join(" "))))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
            return Err(Error::new(
                "Error",
                if stderr.is_empty() {
                    format!("git {} failed", args.join(" "))
                } else {
                    stderr
                },
            ));
        }
        String::from_utf8(out.stdout)
            .map(|s| s.trim().to_owned())
            .map_err(|e| Error::new("Error", e.to_string()))
    };

    let before_full = run_git(&install_dir, &["rev-parse", "HEAD"])?;
    let before_short = run_git(&install_dir, &["rev-parse", "--short", "HEAD"])?;

    let fetch_ok = run_git(&install_dir, &["fetch", "--prune", "origin", "main"]).is_ok();
    let target_ref = if fetch_ok && run_git(&install_dir, &["rev-parse", "FETCH_HEAD"]).is_ok() {
        "FETCH_HEAD"
    } else if run_git(&install_dir, &["rev-parse", "refs/remotes/origin/main"]).is_ok() {
        "refs/remotes/origin/main"
    } else {
        "HEAD"
    };
    let remote_full = run_git(&install_dir, &["rev-parse", target_ref])?;
    let remote_short = run_git(&install_dir, &["rev-parse", "--short", target_ref])?;

    if check_only {
        if before_full == remote_full {
            return Ok(format!(
                "Roster is up to date at {before_short} ({}).\n",
                install_dir.display()
            ));
        }
        let behind = run_git(
            &install_dir,
            &["rev-list", "--count", &format!("HEAD..{remote_full}")],
        )
        .unwrap_or_else(|_| "1+".to_owned());
        return Ok(format!(
            "Update available: {before_short} -> {remote_short} ({behind} commit(s) behind origin/main). Run `roster update` or `/update` to install.\n"
        ));
    }

    if before_full != remote_full
        && run_git(&install_dir, &["merge", "--ff-only", &remote_full]).is_err()
    {
        let _ = run_git(&install_dir, &["checkout", "-B", "main", &remote_full])?;
    }
    let _ = run_git(&install_dir, &["config", "core.autocrlf", "false"]);
    let _ = run_git(
        &install_dir,
        &["submodule", "update", "--init", "--recursive"],
    );
    let sub_dir = install_dir.join("vendor").join("github-agent-contracts");
    if sub_dir.is_dir() {
        let _ = run_git(&sub_dir, &["config", "core.autocrlf", "false"]);
        let _ = run_git(&sub_dir, &["rm", "--cached", "-r", "."]);
        let _ = run_git(&sub_dir, &["reset", "--hard", "HEAD"]);
    }

    let home = config::environment_value(
        environment,
        if cfg!(windows) { "USERPROFILE" } else { "HOME" },
    )
    .ok()
    .flatten()
    .and_then(|t| t.to_utf8().ok())
    .map(PathBuf::from)
    .or_else(|| {
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
    })
    .unwrap_or_else(|| install_dir.clone());

    let mut cargo_bin = config::environment_value(environment, "ROSTER_CARGO")
        .ok()
        .flatten()
        .and_then(|t| t.to_utf8().ok())
        .filter(|s| {
            !s.is_empty()
                && Path::new(s).is_file()
                && !s.ends_with("rustup")
                && !s.ends_with("rustup.exe")
        })
        .map(PathBuf::from);
    if cargo_bin.is_none() {
        let user_cargo =
            home.join(".cargo")
                .join("bin")
                .join(if cfg!(windows) { "cargo.exe" } else { "cargo" });
        if user_cargo.is_file() {
            cargo_bin = Some(user_cargo);
        }
    }
    let mut extra_cargo_home = None;
    let mut extra_rustup_home = None;
    let mut extra_rustflags = None;
    if cargo_bin.is_none() && cfg!(windows) {
        let session_states = home.join(".copilot").join("session-state");
        if let Ok(rd) = fs::read_dir(&session_states) {
            for entry in rd.flatten() {
                let tc_root = entry.path().join("files").join("rust-toolchain");
                let candidate = tc_root.join("cargo").join("bin").join("cargo.exe");
                if candidate.is_file() {
                    cargo_bin = Some(candidate);
                    break;
                }
            }
        }
    }
    let cargo_exe = cargo_bin
        .unwrap_or_else(|| PathBuf::from(if cfg!(windows) { "cargo.exe" } else { "cargo" }));

    if let Some(cargo_dir) = cargo_exe.parent().and_then(Path::parent) {
        if let Some(tc_root) = cargo_dir.parent() {
            if tc_root.join("cargo").is_dir() && tc_root.join("rustup").is_dir() {
                extra_cargo_home = Some(tc_root.join("cargo"));
                extra_rustup_home = Some(tc_root.join("rustup"));
                let dlltool = tc_root
                    .join("llvm-dlltool-20261006")
                    .join("llvm-dlltool.exe");
                if dlltool.is_file() {
                    extra_rustflags = Some(format!("-C dlltool={}", dlltool.display()));
                }
            }
        }
    }

    let mut build_cmd = StdCommand::new(&cargo_exe);
    build_cmd
        .current_dir(&install_dir)
        .args(["build", "--release", "--locked"]);
    if let Some(ch) = extra_cargo_home {
        build_cmd.env("CARGO_HOME", &ch);
        if let Some(bin_parent) = cargo_exe.parent() {
            let cur_path = std::env::var_os("PATH").unwrap_or_default();
            let mut paths = vec![bin_parent.to_path_buf()];
            paths.extend(std::env::split_paths(&cur_path));
            if let Ok(joined) = std::env::join_paths(paths) {
                build_cmd.env("PATH", joined);
            }
        }
    }
    if let Some(rh) = extra_rustup_home {
        build_cmd.env("RUSTUP_HOME", rh);
    }
    if let Some(rf) = extra_rustflags {
        build_cmd.env("RUSTFLAGS", rf);
    }
    let build_out = build_cmd
        .output()
        .map_err(|e| Error::new("Error", format!("cargo build failed: {e}")))?;
    if !build_out.status.success() {
        let stderr = String::from_utf8_lossy(&build_out.stderr).trim().to_owned();
        return Err(Error::new(
            "Error",
            format!("cargo build --release --locked failed: {stderr}"),
        ));
    }

    let built_bin = install_dir
        .join("target")
        .join("release")
        .join(if cfg!(windows) {
            "roster.exe"
        } else {
            "roster"
        });
    if !built_bin.is_file() {
        return Err(Error::new(
            "Error",
            format!(
                "Expected release binary at {} was not produced",
                built_bin.display()
            ),
        ));
    }

    let roster_home = config::machine_config_root(Some(environment))?
        .unwrap_or_else(|| home.clone())
        .join(".roster");
    let targets = if cfg!(windows) {
        vec![
            roster_home.join("bin").join("roster.exe"),
            home.join(".dotnet").join("tools").join("roster.exe"),
        ]
    } else {
        vec![
            home.join(".local").join("bin").join("roster"),
            roster_home.join("bin").join("roster"),
        ]
    };

    for dest in &targets {
        if let Some(parent) = dest.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if cfg!(windows) {
            if let Some(parent) = dest.parent() {
                if let Ok(rd) = fs::read_dir(parent) {
                    for entry in rd.flatten() {
                        let p = entry.path();
                        if p.extension().and_then(|e| e.to_str()) == Some("old") {
                            let _ = fs::remove_file(&p);
                        }
                    }
                }
            }
            let old_dest = PathBuf::from(format!("{}.{}.old", dest.display(), std::process::id()));
            if fs::copy(&built_bin, dest).is_err() {
                let _ = fs::rename(dest, &old_dest);
                fs::copy(&built_bin, dest)
                    .map_err(|e| Error::new("Error", format!("{}: {e}", dest.display())))?;
            }
        } else {
            let _ = fs::remove_file(dest);
            fs::copy(&built_bin, dest)
                .map_err(|e| Error::new("Error", format!("{}: {e}", dest.display())))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(dest, fs::Permissions::from_mode(0o755));
            }
        }
    }

    let primary = &targets[0];
    if before_full == remote_full {
        Ok(format!(
            "Roster is up to date at {remote_short}; verified and refreshed binary at {}.\n",
            primary.display()
        ))
    } else {
        Ok(format!(
            "Updated Roster {before_short} -> {remote_short} and installed {}.\n",
            primary.display()
        ))
    }
}

fn auto_discover_question_files(root: &Path, question: &str) -> Vec<String> {
    let mut selected = Vec::new();
    let lower_q = question.to_lowercase();
    let candidates = [
        "README.md",
        "Cargo.toml",
        "package.json",
        "AGENTS.md",
        "docs/FEATURE_SPEC.md",
        "docs/INSTALL.md",
        "docs/RUST.md",
        "src/index.mjs",
        "src/index.js",
        "src/main.rs",
        "rust/lib.rs",
        "rust/main.rs",
    ];
    for cand in candidates {
        let p = root.join(cand);
        if let Ok(meta) = fs::symlink_metadata(&p) {
            if meta.is_file() && !secure::linked(&meta) && meta.len() <= 65_536 {
                let fname = Path::new(cand)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(cand)
                    .to_lowercase();
                if lower_q.contains(&fname) || selected.len() < 4 {
                    selected.push(cand.to_owned());
                }
            }
        }
        if selected.len() >= 6 {
            break;
        }
    }
    if selected.is_empty() {
        if let Ok(rd) = fs::read_dir(root) {
            let mut entries = rd.flatten().map(|e| e.path()).collect::<Vec<_>>();
            entries.sort();
            for p in entries {
                let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                if name.starts_with('.')
                    || crate::planning::forbidden_read(name)
                    || crate::execution::policy::managed(name)
                {
                    continue;
                }
                if let Ok(meta) = fs::symlink_metadata(&p) {
                    if meta.is_file()
                        && !secure::linked(&meta)
                        && meta.len() <= 65_536
                        && crate::execution::policy::scope_expansion_file(name)
                    {
                        selected.push(name.to_owned());
                        if selected.len() >= 6 {
                            break;
                        }
                    }
                }
            }
        }
    }
    selected
}

fn terminal_columns() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&c| c >= 20)
        .unwrap_or(80)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorStep {
    Continue { changed: bool },
    Submit(String),
    ClearScreen,
    CancelLine,
    Eof,
}

pub struct LineEditor {
    chars: Vec<char>,
    cursor: usize,
    history: Vec<String>,
    history_index: Option<usize>,
    saved_draft: String,
    secret_mode: bool,
}

impl LineEditor {
    pub fn new(history: Vec<String>, secret_mode: bool) -> Self {
        Self {
            chars: Vec::new(),
            cursor: 0,
            history,
            history_index: None,
            saved_draft: String::new(),
            secret_mode,
        }
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn move_left(&mut self) -> bool {
        if self.cursor > 0 {
            self.cursor -= 1;
            true
        } else {
            false
        }
    }

    pub fn move_right(&mut self) -> bool {
        if self.cursor < self.chars.len() {
            self.cursor += 1;
            true
        } else {
            false
        }
    }

    pub fn move_home(&mut self) -> bool {
        if self.cursor > 0 {
            self.cursor = 0;
            true
        } else {
            false
        }
    }

    pub fn move_end(&mut self) -> bool {
        if self.cursor < self.chars.len() {
            self.cursor = self.chars.len();
            true
        } else {
            false
        }
    }

    pub fn move_word_left(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        let mut idx = self.cursor;
        while idx > 0 && self.chars[idx - 1].is_whitespace() {
            idx -= 1;
        }
        while idx > 0 && !self.chars[idx - 1].is_whitespace() {
            idx -= 1;
        }
        self.cursor = idx;
        true
    }

    pub fn move_word_right(&mut self) -> bool {
        if self.cursor >= self.chars.len() {
            return false;
        }
        let mut idx = self.cursor;
        while idx < self.chars.len() && !self.chars[idx].is_whitespace() {
            idx += 1;
        }
        while idx < self.chars.len() && self.chars[idx].is_whitespace() {
            idx += 1;
        }
        self.cursor = idx;
        true
    }

    pub fn insert_char(&mut self, ch: char) -> bool {
        self.chars.insert(self.cursor, ch);
        self.cursor += 1;
        self.history_index = None;
        true
    }

    pub fn backspace(&mut self) -> bool {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
            self.history_index = None;
            true
        } else {
            false
        }
    }

    pub fn delete_forward(&mut self) -> bool {
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
            self.history_index = None;
            true
        } else {
            false
        }
    }

    pub fn delete_word_left(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        let end = self.cursor;
        self.move_word_left();
        let start = self.cursor;
        self.chars.drain(start..end);
        self.history_index = None;
        true
    }

    pub fn kill_to_end(&mut self) -> bool {
        if self.cursor < self.chars.len() {
            self.chars.truncate(self.cursor);
            self.history_index = None;
            true
        } else {
            false
        }
    }

    pub fn kill_to_start(&mut self) -> bool {
        if self.cursor > 0 {
            self.chars.drain(0..self.cursor);
            self.cursor = 0;
            self.history_index = None;
            true
        } else {
            false
        }
    }

    pub fn history_up(&mut self) -> bool {
        if self.secret_mode || self.history.is_empty() {
            return false;
        }
        let next_idx = match self.history_index {
            None => {
                self.saved_draft = self.text();
                self.history.len() - 1
            }
            Some(0) => return false,
            Some(idx) => idx - 1,
        };
        self.history_index = Some(next_idx);
        self.chars = self.history[next_idx].chars().collect();
        self.cursor = self.chars.len();
        true
    }

    pub fn history_down(&mut self) -> bool {
        if self.secret_mode {
            return false;
        }
        let Some(idx) = self.history_index else {
            return false;
        };
        if idx + 1 < self.history.len() {
            let next_idx = idx + 1;
            self.history_index = Some(next_idx);
            self.chars = self.history[next_idx].chars().collect();
            self.cursor = self.chars.len();
        } else {
            self.history_index = None;
            self.chars = self.saved_draft.chars().collect();
            self.cursor = self.chars.len();
        }
        true
    }

    pub fn tab_complete(&mut self) -> bool {
        if self.secret_mode {
            return false;
        }
        let current = self.text();
        if !current.starts_with('/') || current.contains(' ') {
            return false;
        }
        let (matches, _) = complete_command(&current);
        match matches.as_slice() {
            [only] => {
                let completed = format!("{only} ");
                self.chars = completed.chars().collect();
                self.cursor = self.chars.len();
                true
            }
            [first, rest @ ..] => {
                let mut prefix_len = first.len();
                for m in rest {
                    prefix_len = first
                        .bytes()
                        .zip(m.bytes())
                        .take(prefix_len)
                        .take_while(|(a, b)| a == b)
                        .count();
                }
                if prefix_len > current.len() {
                    self.chars = first[..prefix_len].chars().collect();
                    self.cursor = self.chars.len();
                    true
                } else {
                    false
                }
            }
            [] => false,
        }
    }

    pub fn feed_bytes(&mut self, pending: &mut Vec<u8>, chunk: &[u8]) -> EditorStep {
        pending.extend_from_slice(chunk);
        let mut changed = false;
        while !pending.is_empty() {
            let b = pending[0];
            match b {
                b'\r' | b'\n' => {
                    if pending.starts_with(b"\r\n") {
                        pending.drain(0..2);
                    } else {
                        pending.drain(0..1);
                    }
                    return EditorStep::Submit(self.text());
                }
                0x03 => {
                    pending.drain(0..1);
                    if !self.chars.is_empty() {
                        self.chars.clear();
                        self.cursor = 0;
                        self.history_index = None;
                        return EditorStep::CancelLine;
                    }
                    return EditorStep::Eof;
                }
                0x04 => {
                    pending.drain(0..1);
                    if self.chars.is_empty() {
                        return EditorStep::Eof;
                    }
                    changed |= self.delete_forward();
                }
                0x7f | 0x08 => {
                    pending.drain(0..1);
                    changed |= self.backspace();
                }
                0x01 => {
                    pending.drain(0..1);
                    changed |= self.move_home();
                }
                0x05 => {
                    pending.drain(0..1);
                    changed |= self.move_end();
                }
                0x02 => {
                    pending.drain(0..1);
                    changed |= self.move_left();
                }
                0x06 => {
                    pending.drain(0..1);
                    changed |= self.move_right();
                }
                0x10 => {
                    pending.drain(0..1);
                    changed |= self.history_up();
                }
                0x0e => {
                    pending.drain(0..1);
                    changed |= self.history_down();
                }
                0x0b => {
                    pending.drain(0..1);
                    changed |= self.kill_to_end();
                }
                0x15 => {
                    pending.drain(0..1);
                    changed |= self.kill_to_start();
                }
                0x17 => {
                    pending.drain(0..1);
                    changed |= self.delete_word_left();
                }
                0x0c => {
                    pending.drain(0..1);
                    return EditorStep::ClearScreen;
                }
                b'\t' => {
                    pending.drain(0..1);
                    changed |= self.tab_complete();
                }
                0x1b => {
                    if pending.len() == 1 {
                        break;
                    }
                    if pending[1] == b'[' {
                        let final_pos =
                            pending.iter().enumerate().skip(2).find_map(|(idx, &byte)| {
                                (0x40..=0x7e).contains(&byte).then_some(idx)
                            });
                        let Some(end_idx) = final_pos else {
                            if pending.len() < 16 {
                                break;
                            }
                            pending.clear();
                            continue;
                        };
                        let seq = pending[2..=end_idx].to_vec();
                        pending.drain(0..=end_idx);
                        match seq.as_slice() {
                            b"A" => changed |= self.history_up(),
                            b"B" => changed |= self.history_down(),
                            b"C" => changed |= self.move_right(),
                            b"D" => changed |= self.move_left(),
                            b"H" | b"1~" | b"7~" => changed |= self.move_home(),
                            b"F" | b"4~" | b"8~" => changed |= self.move_end(),
                            b"3~" => changed |= self.delete_forward(),
                            b"1;5D" | b"1;3D" | b"5D" => changed |= self.move_word_left(),
                            b"1;5C" | b"1;3C" | b"5C" => changed |= self.move_word_right(),
                            b"3;5~" => changed |= self.delete_word_left(),
                            _ => {}
                        }
                    } else if pending[1] == b'O' {
                        if pending.len() < 3 {
                            break;
                        }
                        let code = pending[2];
                        pending.drain(0..3);
                        match code {
                            b'A' => changed |= self.history_up(),
                            b'B' => changed |= self.history_down(),
                            b'C' => changed |= self.move_right(),
                            b'D' => changed |= self.move_left(),
                            b'H' => changed |= self.move_home(),
                            b'F' => changed |= self.move_end(),
                            _ => {}
                        }
                    } else {
                        let code = pending[1];
                        pending.drain(0..2);
                        match code {
                            b'b' | b'B' => changed |= self.move_word_left(),
                            b'f' | b'F' => changed |= self.move_word_right(),
                            0x7f | 0x08 => changed |= self.delete_word_left(),
                            _ => {}
                        }
                    }
                }
                0x00..=0x1f => {
                    pending.drain(0..1);
                }
                _ => {
                    let char_len = if b < 0x80 {
                        1
                    } else if (b & 0xe0) == 0xc0 {
                        2
                    } else if (b & 0xf0) == 0xe0 {
                        3
                    } else if (b & 0xf8) == 0xf0 {
                        4
                    } else {
                        1
                    };
                    if pending.len() < char_len {
                        break;
                    }
                    if let Ok(s) = std::str::from_utf8(&pending[..char_len]) {
                        if let Some(ch) = s.chars().next() {
                            changed |= self.insert_char(ch);
                        }
                    }
                    pending.drain(0..char_len);
                }
            }
        }
        EditorStep::Continue { changed }
    }
}

#[cfg(windows)]
struct RawTerminalGuard {
    in_handle: winsafe::HSTD,
    orig_in_mode: winsafe::co::CONSOLE,
    out_handle: Option<(winsafe::HSTD, winsafe::co::CONSOLE)>,
}

#[cfg(windows)]
impl RawTerminalGuard {
    fn enter() -> Option<Self> {
        use winsafe::{self as w, co, prelude::*};
        let mut in_guard = w::HSTD::GetStdHandle(co::STD_HANDLE::INPUT).ok()?;
        let in_handle = in_guard.leak();
        let orig_in_mode = in_handle.GetConsoleMode().ok()?;
        let mut raw_in = orig_in_mode;
        raw_in &= !(co::CONSOLE::ENABLE_LINE_INPUT
            | co::CONSOLE::ENABLE_ECHO_INPUT
            | co::CONSOLE::ENABLE_PROCESSED_INPUT);
        raw_in |= co::CONSOLE::ENABLE_VIRTUAL_TERMINAL_INPUT;
        in_handle.SetConsoleMode(raw_in).ok()?;
        let out_handle = w::HSTD::GetStdHandle(co::STD_HANDLE::OUTPUT)
            .ok()
            .and_then(|mut g| {
                let h = g.leak();
                let orig_out = h.GetConsoleMode().ok()?;
                let _ = h.SetConsoleMode(
                    orig_out
                        | co::CONSOLE::ENABLE_PROCESSED_OUTPUT
                        | co::CONSOLE::ENABLE_VIRTUAL_TERMINAL_PROCESSING,
                );
                Some((h, orig_out))
            });
        Some(Self {
            in_handle,
            orig_in_mode,
            out_handle,
        })
    }

    fn read_chunk(&self) -> Result<Vec<u8>, Error> {
        use winsafe::{self as w, prelude::*};
        let mut wbuf = w::WString::new_alloc_buf(64);
        let n = self
            .in_handle
            .ReadConsole(&mut wbuf, None)
            .map_err(|e| Error::new("Error", e.to_string()))?;
        if n == 0 {
            return Ok(Vec::new());
        }
        let slice = &wbuf.as_slice()[..n as usize];
        Ok(String::from_utf16_lossy(slice).into_bytes())
    }
}

#[cfg(windows)]
impl Drop for RawTerminalGuard {
    fn drop(&mut self) {
        use winsafe::prelude::*;
        let _ = self.in_handle.SetConsoleMode(self.orig_in_mode);
        if let Some((ref out_h, orig_out)) = self.out_handle {
            let _ = out_h.SetConsoleMode(orig_out);
        }
    }
}

#[cfg(unix)]
struct RawTerminalGuard {
    saved_stty: String,
}

#[cfg(unix)]
impl RawTerminalGuard {
    fn enter() -> Option<Self> {
        use std::io::IsTerminal;
        use std::process::Stdio;
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            return None;
        }
        let out = StdCommand::new("stty")
            .arg("-g")
            .stdin(Stdio::inherit())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let saved_stty = String::from_utf8(out.stdout).ok()?.trim().to_owned();
        if saved_stty.is_empty() {
            return None;
        }
        let status = StdCommand::new("stty")
            .args(["-icanon", "-echo", "-isig", "min", "1", "time", "0"])
            .stdin(Stdio::inherit())
            .status()
            .ok()?;
        if !status.success() {
            return None;
        }
        Some(Self { saved_stty })
    }

    fn read_chunk(&self) -> Result<Vec<u8>, Error> {
        let mut buf = [0_u8; 64];
        let n = std::io::stdin()
            .read(&mut buf)
            .map_err(|e| Error::new("Error", e.to_string()))?;
        Ok(buf[..n].to_vec())
    }
}

#[cfg(unix)]
impl Drop for RawTerminalGuard {
    fn drop(&mut self) {
        use std::process::Stdio;
        let _ = StdCommand::new("stty")
            .arg(&self.saved_stty)
            .stdin(Stdio::inherit())
            .status();
    }
}

fn format_multiline_editor_repaint(
    prompt_str: &str,
    editor: &LineEditor,
    prev_cursor_row: &mut usize,
    cols: usize,
) -> String {
    let wrap_width = cols.saturating_sub(1).max(40);
    let prompt_len = strip_vt_only(prompt_str).chars().count();
    let mut seq = String::new();
    if *prev_cursor_row > 0 {
        seq.push_str(&format!("\x1b[{}A", *prev_cursor_row));
    }
    seq.push_str("\r\x1b[J");
    seq.push_str(prompt_str);
    let mut pos = prompt_len;
    for &ch in &editor.chars {
        let row_idx = pos / wrap_width;
        let col_idx = pos - (row_idx * wrap_width);
        if pos > 0 && col_idx == 0 {
            seq.push_str("\r\n");
        }
        seq.push(ch);
        pos += 1;
    }
    let final_row_idx = pos / wrap_width;
    let final_col_idx = pos - (final_row_idx * wrap_width);
    if pos > 0 && final_col_idx == 0 {
        seq.push_str("\r\n");
    }
    let end_row = pos / wrap_width;
    let target_pos = prompt_len + editor.cursor();
    let target_row = target_pos / wrap_width;
    let target_col = target_pos - (target_row * wrap_width);
    if end_row > target_row {
        seq.push_str(&format!("\x1b[{}A", end_row - target_row));
    }
    seq.push('\r');
    if target_col > 0 {
        seq.push_str(&format!("\x1b[{target_col}C"));
    }
    *prev_cursor_row = target_row;
    seq
}

fn repaint_editor_line(
    stdout: &mut std::io::Stdout,
    prompt_str: &str,
    editor: &LineEditor,
    prev_cursor_row: &mut usize,
    cols: usize,
) -> Result<(), Error> {
    if editor.secret_mode {
        return Ok(());
    }
    let seq = format_multiline_editor_repaint(prompt_str, editor, prev_cursor_row, cols);
    stdout
        .write_all(seq.as_bytes())
        .and_then(|()| stdout.flush())
        .map_err(|e| Error::new("Error", e.to_string()))?;
    Ok(())
}

fn read_interactive_line(
    stdout: &mut std::io::Stdout,
    prompt_str: &str,
    history_lines: &[String],
    secret_mode: bool,
    cols: usize,
) -> Result<Option<String>, Error> {
    use std::io::{BufRead, IsTerminal};
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        if let Some(guard) = RawTerminalGuard::enter() {
            let mut editor = LineEditor::new(history_lines.to_vec(), secret_mode);
            let mut prev_cursor_row = 0_usize;
            let mut pending = Vec::new();
            loop {
                let chunk = guard.read_chunk()?;
                if chunk.is_empty() {
                    let _ = stdout.write_all(b"\r\n");
                    let _ = stdout.flush();
                    return Ok(None);
                }
                match editor.feed_bytes(&mut pending, &chunk) {
                    EditorStep::Continue { changed } => {
                        if changed {
                            repaint_editor_line(
                                stdout,
                                prompt_str,
                                &editor,
                                &mut prev_cursor_row,
                                cols,
                            )?;
                        }
                    }
                    EditorStep::CancelLine => {
                        repaint_editor_line(
                            stdout,
                            prompt_str,
                            &editor,
                            &mut prev_cursor_row,
                            cols,
                        )?;
                    }
                    EditorStep::ClearScreen => {
                        stdout
                            .write_all(b"\x1b[2J\x1b[H")
                            .and_then(|()| stdout.flush())
                            .map_err(|e| Error::new("Error", e.to_string()))?;
                        prev_cursor_row = 0;
                        repaint_editor_line(
                            stdout,
                            prompt_str,
                            &editor,
                            &mut prev_cursor_row,
                            cols,
                        )?;
                    }
                    EditorStep::Submit(line) => {
                        if !secret_mode {
                            let _ = editor.move_end();
                            let _ = repaint_editor_line(
                                stdout,
                                prompt_str,
                                &editor,
                                &mut prev_cursor_row,
                                cols,
                            );
                        }
                        stdout
                            .write_all(b"\r\n")
                            .and_then(|()| stdout.flush())
                            .map_err(|e| Error::new("Error", e.to_string()))?;
                        return Ok(Some(line));
                    }
                    EditorStep::Eof => {
                        stdout
                            .write_all(b"\r\n")
                            .and_then(|()| stdout.flush())
                            .map_err(|e| Error::new("Error", e.to_string()))?;
                        return Ok(None);
                    }
                }
            }
        }
    }
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let mut line = String::new();
    let read_bytes = reader
        .read_line(&mut line)
        .map_err(|e| Error::new("Error", e.to_string()))?;
    if read_bytes == 0 {
        stdout
            .write_all(b"\n")
            .and_then(|()| stdout.flush())
            .map_err(|e| Error::new("Error", e.to_string()))?;
        return Ok(None);
    }
    Ok(Some(line.trim_end_matches(['\r', '\n']).to_owned()))
}

pub async fn start_repl(
    cwd: &Path,
    installation: &Path,
    environment: &Value,
    debug_enabled: bool,
) -> Result<u8, Error> {
    let mut session = ShellSession::new(cwd, installation, environment, debug_enabled)?;
    let color = std::env::var_os("NO_COLOR").is_none();
    session.live_output = true;
    session.color = color;
    let facts = collect_banner_facts(cwd, installation, environment, &session.config);
    let banner = format_banner(&facts, color);
    let mut stdout = std::io::stdout();
    stdout
        .write_all(banner.as_bytes())
        .and_then(|()| stdout.flush())
        .map_err(|e| Error::new("Error", e.to_string()))?;

    let mut current_prompt = String::from("roster> ");
    loop {
        let cols = terminal_columns();
        if !session.tray_visible {
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
            current_prompt = prompt.clone();
            session.bar_lines = bar_lines;
            session.tray_visible = true;
            stdout
                .write_all(format!("{tray_block}{prompt}").as_bytes())
                .and_then(|()| stdout.flush())
                .map_err(|e| Error::new("Error", e.to_string()))?;
        }

        let Some(trimmed_line) = read_interactive_line(
            &mut stdout,
            &current_prompt,
            session.history.lines(),
            session.pending_secret.is_some(),
            cols,
        )?
        else {
            break;
        };
        session.tray_visible = false;
        if session.pending_secret.is_some() && color {
            let _ = stdout.write_all(b"\x1b[1A\r\x1b[2KSecret (input hidden): ********\n");
            let _ = stdout.flush();
        }
        match session.dispatch_line(&trimmed_line).await {
            Ok(ShellAction::Continue(out)) => {
                if !out.is_empty() {
                    if session.tray_visible {
                        let _ = stdout.write_all(b"\r\x1b[2K");
                        if session.bar_lines > 0 {
                            let _ = stdout.write_all(
                                format!("\x1b[{}A\r\x1b[J", session.bar_lines).as_bytes(),
                            );
                        }
                        session.tray_visible = false;
                    }
                    stdout
                        .write_all(out.as_bytes())
                        .and_then(|()| stdout.flush())
                        .map_err(|e| Error::new("Error", e.to_string()))?;
                }
            }
            Ok(ShellAction::ClearScreen) => {
                session.tray_visible = false;
                stdout
                    .write_all(b"\x1b[2J\x1b[H")
                    .and_then(|()| stdout.flush())
                    .map_err(|e| Error::new("Error", e.to_string()))?;
            }
            Ok(ShellAction::Redraw) => {
                if session.tray_visible {
                    let _ = stdout.write_all(b"\r\x1b[2K");
                    if session.bar_lines > 0 {
                        let _ = stdout
                            .write_all(format!("\x1b[{}A\r\x1b[J", session.bar_lines).as_bytes());
                    }
                    session.tray_visible = false;
                }
            }
            Ok(ShellAction::PromptSecret(prompt_str)) => {
                session.tray_visible = false;
                stdout
                    .write_all(prompt_str.as_bytes())
                    .and_then(|()| stdout.flush())
                    .map_err(|e| Error::new("Error", e.to_string()))?;
                let Some(sec_trimmed) =
                    read_interactive_line(&mut stdout, &prompt_str, &[], true, cols)?
                else {
                    break;
                };
                if let Ok(ShellAction::Continue(out)) = session.dispatch_line(&sec_trimmed).await {
                    let _ = stdout.write_all(out.as_bytes());
                    let _ = stdout.flush();
                }
            }
            Ok(ShellAction::Exit) => break,
            Err(err) => {
                if session.tray_visible {
                    let _ = stdout.write_all(b"\r\x1b[2K");
                    if session.bar_lines > 0 {
                        let _ = stdout
                            .write_all(format!("\x1b[{}A\r\x1b[J", session.bar_lines).as_bytes());
                    }
                    session.tray_visible = false;
                }
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
    let project_root = config::resolve_config_root_with_env(installation, cwd, Some(environment))
        .or_else(|_| paths::resolve_project_root(cwd))?;
    let config_path = project_root.join(".roster").join("config.yml");
    let fleet_path = project_root.join(".roster").join("fleet.yml");
    paths::ensure_local_path(&config_path, &project_root)?;
    let previous = settings::read_private_file(&project_root, settings::Name::Config)?;
    let base_source = match &previous {
        Some(src) => src.clone(),
        None => settings::read_config_file(&project_root.join("roster.config.example.yml"))
            .or_else(|_| {
                settings::read_config_file(&installation.join("roster.config.example.yml"))
            })
            .unwrap_or_else(|_| Text::from(include_str!("../roster.config.example.yml"))),
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
    let discovered_endpoints = found
        .iter()
        .map(|(url_str, models)| {
            Value::object(vec![
                ("baseUrl", Value::string(url_str)),
                ("models", Value::Array(models.clone())),
            ])
        })
        .collect::<Vec<_>>();
    let merged_fleet = crate::fleet::merge_discovered_profiles(
        &Value::object(vec![("profiles", Value::Array(seeded_profiles))]),
        &discovered_endpoints,
    )?;
    let seeded_fleet = crate::fleet::format(&merged_fleet.as_value())?;

    if publish {
        let app_status = check_app_identity(cwd, Some(environment))?;
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
    if project_root.join(".git").exists() || project_root.join(".gitignore").exists() {
        settings::ensure_private_files_ignored(
            &project_root,
            &[settings::Name::Config, settings::Name::Fleet],
            Some(&source.token()),
        )
        .await?;
    }
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
    let checked = crate::local_cli::doctor(installation, cwd, environment, &source.token()).await?;
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

        let plan_act = session
            .dispatch_line("/plan Update README.md with a status table.")
            .await?;
        assert!(
            matches!(plan_act, ShellAction::Continue(ref s) if s.contains("Plan ready. Press Enter to accept and continue"))
        );
        assert!(session.pending_confirm);
        let stop_act = session.dispatch_line("/stop").await?;
        assert_eq!(
            stop_act,
            ShellAction::Continue("Run cancelled.\n".to_owned())
        );
        assert!(!session.pending_confirm);

        {
            use crate::llm::control::Observer;
            let mut obs = ShellLiveObserver::new(
                &mut session.display,
                &mut session.last_measured_seat,
                &mut session.last_finish_reason,
                &session.ask_queue,
                session.statusbar,
                session.debug.enabled(),
                false,
                false,
                &mut session.tray_visible,
                &mut session.bar_lines,
                Some(42),
            );
            obs.emit(Value::object(vec![
                ("type", Value::string("seat-start")),
                ("seat", Value::string("planner")),
            ]))
            .await?;
            obs.emit(Value::object(vec![
                ("type", Value::string("delta")),
                ("text", Value::string("## Outcome\nUpdate README.md\n")),
            ]))
            .await?;
            obs.emit(Value::object(vec![
                ("type", Value::string("usage")),
                ("input", Value::Number(1200.0)),
                ("output", Value::Number(300.0)),
            ]))
            .await?;
            obs.emit(Value::object(vec![
                ("type", Value::string("seat-end")),
                ("verdict", Value::string("pass")),
            ]))
            .await?;
        }
        assert_eq!(session.display.get("state"), Some(&Value::string("passed")));
        assert_eq!(
            session.display.get("contextUsed"),
            Some(&Value::Number(1200.0))
        );

        assert_eq!(
            session.dispatch_line("/resume").await?,
            ShellAction::Continue("No local issue runs.\n".to_owned())
        );
        assert!(session.dispatch_line("/resume 42 --invalid").await.is_err());
        let issue_wt = root.join(".worktrees").join("issue-42");
        let cp_dir = issue_wt.join(".roster").join("checkpoints").join("42");
        files::create_private_directories(&cp_dir)?;
        fs::write(
            issue_wt.join("ASSIGNMENT.md"),
            "# Assignment\n\n- Issue URL: https://github.com/example/project/issues/42\n- Issue number: 42\n- Title: Add status\n\n## Ask\n\nAdd a Status section to README.md.\n",
        )
        .map_err(|e| Error::new("Error", e.to_string()))?;
        fs::write(
            issue_wt.join("TASK.md"),
            "# Task: Add status\n\ndifficulty: 1\ntask_class: docs\n\n## Acceptance checks\n- README has Status\n\n## Files allowed\n- `README.md`\n\n## Ask\nAdd a Status section to README.md.\n",
        )
        .map_err(|e| Error::new("Error", e.to_string()))?;
        fs::write(
            cp_dir.join("1"),
            "{\"number\":1,\"task\":\"issue-42\",\"seat\":\"coder\",\"status\":\"before-write\",\"time\":\"2026-10-20T00:00:00.000Z\",\"ref\":\"refs/roster/checkpoints/issue-42/1\",\"tree\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"scope\":[\"README.md\"]}\n",
        )
        .map_err(|e| Error::new("Error", e.to_string()))?;
        fs::write(
            cp_dir.join("2"),
            "{\"number\":2,\"task\":\"issue-42\",\"seat\":\"coder\",\"status\":\"verified\",\"time\":\"2026-10-20T00:05:00.000Z\",\"ref\":\"refs/roster/checkpoints/issue-42/2\",\"tree\":\"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\",\"scope\":[\"README.md\"]}\n",
        )
        .map_err(|e| Error::new("Error", e.to_string()))?;
        let resume_list = session.dispatch_line("/resume").await?;
        assert_eq!(
            resume_list,
            ShellAction::Continue("#42 | Add status | coder | idle | issue-42\n".to_owned())
        );
        assert!(session.dispatch_line("/resume 42 --green").await.is_err());
        let resume_act = session.dispatch_line("/resume 42").await?;
        assert_eq!(
            resume_act,
            ShellAction::Continue("Resumed local issue #42 (idle).\n".to_owned())
        );
        let cp_act = session.dispatch_line("/checkpoints").await?;
        assert_eq!(
            cp_act,
            ShellAction::Continue(
                "1 | coder | before-write | 2026-10-20T00:00:00.000Z\n2 | coder | verified | 2026-10-20T00:05:00.000Z\n".to_owned()
            )
        );
        assert!(session.dispatch_line("/undo 1").await.is_err());
        assert!(session.dispatch_line("/rewind invalid").await.is_err());
        assert!(session.dispatch_line("/rewind 1").await.is_err());
        assert!(session.dispatch_line("/undo").await.is_err());

        assert_eq!(
            session.dispatch_line("/clear").await?,
            ShellAction::ClearScreen
        );
        assert_eq!(session.dispatch_line("/redraw").await?, ShellAction::Redraw);
        assert_eq!(session.dispatch_line("/quit").await?, ShellAction::Exit);
        let _ = fs::remove_dir_all(&root);
        Ok(())
    }

    #[test]
    fn line_editor_supports_arrow_navigation_midline_edits_history_and_tab_completion()
    -> Result<(), Error> {
        let history = vec!["/status".to_owned(), "/fleet list".to_owned()];
        let mut editor = LineEditor::new(history, false);
        let mut pending = Vec::new();

        let step = editor.feed_bytes(&mut pending, b"hello world");
        assert_eq!(step, EditorStep::Continue { changed: true });
        assert_eq!(editor.text(), "hello world");
        assert_eq!(editor.cursor(), 11);

        let _ = editor.feed_bytes(&mut pending, b"\x1b[D\x1b[D\x1b[D\x1b[D\x1b[D");
        assert_eq!(editor.cursor(), 6);
        let _ = editor.feed_bytes(&mut pending, b"brave ");
        assert_eq!(editor.text(), "hello brave world");
        assert_eq!(editor.cursor(), 12);

        let _ = editor.feed_bytes(&mut pending, b"\x1b[H");
        assert_eq!(editor.cursor(), 0);
        let _ = editor.feed_bytes(&mut pending, b"\x1b[3~H");
        assert_eq!(editor.text(), "Hello brave world");
        assert_eq!(editor.cursor(), 1);

        let _ = editor.feed_bytes(&mut pending, b"\x1b[F");
        assert_eq!(editor.cursor(), 17);
        let _ = editor.feed_bytes(&mut pending, b"\x1b[1;5D");
        assert_eq!(editor.cursor(), 12);
        let _ = editor.feed_bytes(&mut pending, b"\x17");
        assert_eq!(editor.text(), "Hello world");
        assert_eq!(editor.cursor(), 6);

        let _ = editor.feed_bytes(&mut pending, b"\x1b[A");
        assert_eq!(editor.text(), "/fleet list");
        let _ = editor.feed_bytes(&mut pending, b"\x1b[A");
        assert_eq!(editor.text(), "/status");
        let _ = editor.feed_bytes(&mut pending, b"\x1b[B");
        assert_eq!(editor.text(), "/fleet list");
        let _ = editor.feed_bytes(&mut pending, b"\x1b[B");
        assert_eq!(editor.text(), "Hello world");

        let cancel = editor.feed_bytes(&mut pending, b"\x03");
        assert_eq!(cancel, EditorStep::CancelLine);
        assert_eq!(editor.text(), "");
        assert_eq!(editor.cursor(), 0);

        let _ = editor.feed_bytes(&mut pending, b"/chek\x7fckp\t");
        assert_eq!(editor.text(), "/checkpoints ");

        let submit = editor.feed_bytes(&mut pending, b"\r");
        assert_eq!(submit, EditorStep::Submit("/checkpoints ".to_owned()));

        let long_cmd = "What dev drives and repositories are on this windows PC, make me a nice markdown file with the list and state of them and place it in the current C:\\Users\\harrr directory.";
        let mut ml_editor = LineEditor::new(vec![long_cmd.to_owned()], false);
        let mut prev_row = 0_usize;
        let _ = ml_editor.feed_bytes(&mut pending, b"\x1b[A");
        assert_eq!(ml_editor.text(), long_cmd);
        let painted_up = format_multiline_editor_repaint("roster> ", &ml_editor, &mut prev_row, 80);
        assert!(painted_up.contains("\r\n"));
        assert_eq!(prev_row, 2);
        let _ = ml_editor.feed_bytes(&mut pending, b"\x1b[B");
        assert_eq!(ml_editor.text(), "");
        let painted_down =
            format_multiline_editor_repaint("roster> ", &ml_editor, &mut prev_row, 80);
        assert!(painted_down.starts_with("\x1b[2A\r\x1b[J"));
        assert_eq!(prev_row, 0);
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn workspace_inventory_delivers_markdown_file_for_windows_and_wsl_prompts()
    -> Result<(), Error> {
        let install = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = std::env::temp_dir().join(format!(
            "roster-inventory-test-{}-{}",
            std::process::id(),
            files::nonce()?
        ));
        let repos_dir = root.join("repos");
        files::create_private_directories(&repos_dir)?;
        let sample_repo = repos_dir.join("sample-repo");
        files::create_private_directories(&sample_repo.join(".git"))?;

        let env = Value::object(vec![
            ("HOME", Value::string(&root.display().to_string())),
            ("USERPROFILE", Value::string(&root.display().to_string())),
        ]);
        let mut session = ShellSession::new(&root, install, &env, false)?;
        session.config = config::defaults()?.as_value().clone();

        let win_prompt = format!(
            "What dev drives and repositories are on this windows PC, make me a nice markdown file with the list and state of them and place it in the current {} directory.",
            root.display()
        );
        assert!(is_workspace_inventory_ask(&win_prompt)?);
        let win_act = session.dispatch_line(&win_prompt).await?;
        let ShellAction::Continue(win_out) = win_act else {
            return Err(Error::new("TestError", "expected Continue"));
        };
        assert!(win_out.contains("Wrote markdown report:"));
        let root_report = root.join("DEV_DRIVES_AND_REPOSITORIES.md");
        assert!(root_report.is_file());
        let report_text =
            fs::read_to_string(&root_report).map_err(|e| Error::new("TestError", e.to_string()))?;
        assert!(report_text.contains("# Development Drives & Git Repositories Inventory"));
        assert!(report_text.contains("sample-repo"));

        let wsl_prompt = "What dev drives and repositories are on this windows PC, make me a nice markdown file with the list and state of them and place it in the current ~/repos directory in wsl";
        assert!(is_workspace_inventory_ask(wsl_prompt)?);
        let wsl_act = session.dispatch_line(wsl_prompt).await?;
        let ShellAction::Continue(wsl_out) = wsl_act else {
            return Err(Error::new("TestError", "expected Continue"));
        };
        assert!(wsl_out.contains("Wrote markdown report:"));
        let repos_report = repos_dir.join("DEV_DRIVES_AND_REPOSITORIES.md");
        assert!(repos_report.is_file());

        let nl_status = session
            .dispatch_line("Please show me the current status")
            .await?;
        assert!(
            matches!(nl_status, ShellAction::Continue(ref s) if s.contains("Translated natural language -> /status") && s.contains("State:"))
        );

        let nl_file_act = session
            .dispatch_line("Make me a markdown file called DEPLOY_CHECKLIST.md in this directory")
            .await?;
        assert!(
            matches!(nl_file_act, ShellAction::Continue(ref s) if s.contains("Wrote file:") && s.contains("DEPLOY_CHECKLIST.md"))
        );
        assert!(root.join("DEPLOY_CHECKLIST.md").is_file());

        let upd_check = session.dispatch_line("/update --check").await?;
        assert!(
            matches!(upd_check, ShellAction::Continue(ref s) if s.contains("Roster is up to date") || s.contains("Update available:"))
        );

        let _ = fs::remove_dir_all(&root);
        Ok(())
    }

    #[test]
    fn natural_language_translation_maps_commands_questions_and_delivery_tasks() -> Result<(), Error>
    {
        assert_eq!(
            translate_natural_language_command("Show me my fleet models"),
            Some("/fleet list".to_owned())
        );
        assert_eq!(
            translate_natural_language_command("Can you check openshell readiness?"),
            Some("/doctor openshell".to_owned())
        );
        assert_eq!(
            translate_natural_language_command("switch to aperture-dsv4"),
            Some("/fleet use aperture-dsv4".to_owned())
        );
        assert_eq!(
            translate_natural_language_command("list open github issues"),
            Some("/issues".to_owned())
        );
        assert_eq!(
            translate_natural_language_command("check for updates"),
            Some("/update --check".to_owned())
        );
        assert_eq!(
            translate_natural_language_command("update roster"),
            Some("/update".to_owned())
        );

        let ws_files = vec!["README.md".to_owned(), "rust/shell.rs".to_owned()];
        let parsed_task = parse_natural_language_translation_json(
            r#"{"intent":"delivery_task","outcome":"Document natural language translation in README.md","files":["README.md"],"checks":["README.md documents natural language translation"]}"#,
            "Add documentation on natural language translation",
            true,
            &ws_files,
        );
        let Some(NaturalLanguageTranslation::DeliveryTask {
            structured_ask,
            files,
            ..
        }) = parsed_task
        else {
            return Err(Error::new("TestError", "expected DeliveryTask"));
        };
        assert_eq!(files, vec!["README.md".to_owned()]);
        assert!(structured_ask.contains("## Allowed Files\n- `README.md`"));
        assert!(structured_ask.contains("## Acceptance checks\n- README.md documents"));
        let classified =
            crate::planning::intake::classify(&Value::string(&structured_ask), None, &[])?;
        assert_eq!(classified.get("kind"), Some(&Value::string("slice")));

        let parsed_cmd = parse_natural_language_translation_json(
            r#"{"intent":"command","command":"/fleet list"}"#,
            "what models do I have",
            true,
            &ws_files,
        );
        assert_eq!(
            parsed_cmd,
            Some(NaturalLanguageTranslation::ShellCommand {
                command: "/fleet list".to_owned(),
                reason: "LLM command translation".to_owned(),
            })
        );
        Ok(())
    }
}
