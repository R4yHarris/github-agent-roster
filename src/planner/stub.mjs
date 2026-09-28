import { readFileSync } from 'node:fs';
import { chatCompletion } from '../lib/llm.mjs';
import { parseRecipe } from '../lib/recipe.mjs';
import { isForbiddenWrite } from '../runtime/tools.mjs';

const templates = new Map();
const defaultChecks = ['node --test exits 0', 'The requested behavior in the Ask is implemented'];
const filename = /(?:[A-Za-z0-9_.-]+\/)*[A-Za-z0-9_.-]+\.(?:mjs|js|ts|tsx|json|yml|yaml|md|css|html|txt)/g;

function template(name) {
  if (!templates.has(name)) {
    templates.set(name, readFileSync(
      new URL(`../../templates/sdlc/${name}.${name === 'RECIPE' ? 'yml' : 'md'}`, import.meta.url), 'utf8',
    ).replace(/\r\n/g, '\n'));
  }
  return templates.get(name);
}

function render(template, values) {
  return template.replace(/\{\{([A-Z_]+)\}\}/g, (placeholder, key) => {
    if (!Object.hasOwn(values, key)) throw new Error(`Unknown SDLC template field ${placeholder}`);
    return values[key];
  });
}

function oneLine(value, label) {
  if (typeof value !== 'string' || !value.trim() ||
      /[\x00-\x1f\x7f]/.test(value) || value.length > 240) {
    throw new TypeError(`${label} must be one nonempty line (at most 240 characters)`);
  }
  return value.trim();
}

function cleanAskText(ask) {
  if (typeof ask !== 'string' || !ask.trim() || Buffer.byteLength(ask, 'utf8') > 16_384 ||
      /[\x00-\x08\x0b-\x1f\x7f]/.test(ask)) {
    throw new TypeError('Ask must be nonempty UTF-8 text of at most 16 KiB');
  }
  return ask.replace(/\r\n/g, '\n').trim();
}

function allowedFile(value) {
  const file = oneLine(value, 'Files allowed entry');
  if (!/^(?:\*\*\/\*|[A-Za-z0-9_.-]+(?:\/[A-Za-z0-9_.-]+)*(?:\/\*\*)?)$/.test(file) ||
      file.split('/').some((part) => part === '.' || part === '..') || isForbiddenWrite(file)) {
    throw new TypeError('Files allowed entries must stay inside the worktree and exclude protected files');
  }
  return file;
}

function listInAsk(ask, heading) {
  const section = new RegExp(`^#{1,3} ${heading}\\s*\\n((?:[\\s\\S]*?))(?=^#{1,3} |$(?![\\s\\S]))`, 'im').exec(ask);
  if (!section) return null;
  const items = section[1].split('\n').filter((line) => line.trim()).map((line) => {
    const item = /^\s*-\s+(.+?)\s*$/.exec(line);
    if (!item) throw new TypeError(`${heading} must contain only bullet points`);
    return item[1].replace(/^`(.*)`$/, '$1');
  });
  if (!items.length) throw new TypeError(`${heading} must contain at least one bullet point`);
  return items;
}

function checkedList(items, label, check, limit = 8) {
  if (!Array.isArray(items) || items.length < 1 || items.length > limit) {
    throw new TypeError(`${label} must contain 1-${limit} entries`);
  }
  return items.map(check);
}

function buildPlan(ask, { reference, title, acceptanceChecks, filesAllowed }) {
  const cleanAsk = cleanAskText(ask);
  const checks = checkedList(acceptanceChecks, 'Acceptance checks',
    (check) => oneLine(check, 'Acceptance check'));
  const files = checkedList(filesAllowed, 'Files allowed', allowedFile, 32);
  const recipe = template('RECIPE').replace('issue:N', reference);
  parseRecipe(recipe);
  return {
    recipe,
    task: render(template('TASK'), {
      TITLE: oneLine(title, 'Task title'),
      CHECKS: checks.map((check) => `- ${check}`).join('\n'),
      FILES: files.map((file) => `- \`${file}\``).join('\n'),
      ASK: cleanAsk,
    }),
  };
}

export function planStub(ask, { reference = 'local:draft', title } = {}) {
  const cleanAsk = cleanAskText(ask);
  const checkList = listInAsk(cleanAsk, 'Acceptance checks') ?? defaultChecks;
  const explicitFiles = listInAsk(cleanAsk, 'Files allowed');
  const inferred = [...new Set((cleanAsk.match(filename) ?? []).filter((file) => {
    try {
      allowedFile(file);
      return true;
    } catch {
      return false;
    }
  }))];
  return buildPlan(ask, {
    reference,
    title: title ?? cleanAsk.split(/\r?\n/)[0].slice(0, 200),
    acceptanceChecks: checkList,
    filesAllowed: explicitFiles ?? (inferred.length ? inferred : ['**/*']),
  });
}

export async function planAsk(ask, { config, reference = 'local:draft', title, fetchImpl, env } = {}) {
  const cleanAsk = cleanAskText(ask);
  if (!config.llm.base_url) return { ...planStub(cleanAsk, { reference, title }), usage: null };
  const response = await chatCompletion({
    config,
    fetchImpl,
    env,
    messages: [
      { role: 'system', content: 'Plan one software task. Return only JSON with title, acceptance_checks (short, verifiable strings including node --test exits 0), and files_allowed (relative files or directory/** patterns). Do not include protected files, merge, deploy, or extra seats.' },
      { role: 'user', content: cleanAsk },
    ],
  });
  const message = response?.choices?.[0]?.message;
  if (typeof message?.content !== 'string' || message.tool_calls?.length) {
    throw new Error('LLM planner did not return a JSON task plan');
  }
  let plan;
  try {
    plan = JSON.parse(message.content);
  } catch {
    throw new Error('LLM planner returned invalid JSON');
  }
  if (!plan || typeof plan !== 'object' || Array.isArray(plan) ||
      Object.keys(plan).sort().join(',') !== 'acceptance_checks,files_allowed,title') {
    throw new Error('LLM planner returned an unsupported task plan');
  }
  return {
    ...buildPlan(ask, {
      reference,
      title: title ?? plan.title,
      acceptanceChecks: plan.acceptance_checks,
      filesAllowed: plan.files_allowed,
    }),
    usage: response.usage ?? null,
  };
}

export function renderAsk(ask) {
  return render(template('ASK'), { ASK: ask.trim() });
}

export function renderAssignment(issue) {
  return render(template('ASSIGNMENT'), {
    ISSUE_URL: issue.url,
    ISSUE_NUMBER: String(issue.number),
    TITLE: issue.title,
    ASK: issue.body,
  });
}

export function taskFilesAllowed(task) {
  if (typeof task !== 'string') throw new TypeError('TASK.md must be text');
  const section = /^## Files allowed\n((?:- `[^`\r\n]+`\n)+)\n## Ask(?:\n|$)/m.exec(task);
  if (!section) throw new Error('TASK.md must contain a Files allowed list before the Ask');
  return checkedList(section[1].trimEnd().split('\n').map((line) => line.slice(3, -1)),
    'Files allowed', allowedFile, 32);
}
