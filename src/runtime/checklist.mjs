// Spec §5.4: the coder works the TASK.md checks as a checklist and cannot finish while any item is open.
const statuses = ['pending', 'in_progress', 'done', 'blocked'];
const oneLine = (value) => String(value ?? '').replace(/\s+/g, ' ').trim().slice(0, 300);

export const checklistTool = {
  type: 'function',
  function: {
    name: 'update_checklist',
    description: 'Update your checklist of TASK.md acceptance checks. Mark one item in_progress while you work on it, ' +
      'then done with evidence (the file and symbol or test that proves it), or blocked with the reason. ' +
      'You cannot finish while any item is pending or in_progress.',
    parameters: {
      type: 'object',
      properties: {
        items: {
          type: 'array',
          items: {
            type: 'object',
            properties: {
              id: { type: 'integer', description: 'Check number from TASK.md' },
              status: { type: 'string', enum: statuses },
              evidence: { type: 'string', description: 'Required for done and blocked' },
            },
            required: ['id', 'status'],
          },
        },
      },
      required: ['items'],
    },
  },
};

export function createChecklist(checks) {
  return { items: checks.map((check, index) => ({ id: index + 1, check: oneLine(check), status: 'pending', evidence: '' })) };
}

export const openItems = (checklist) => checklist.items.filter(({ status }) => ['pending', 'in_progress'].includes(status));

export function checklistProgress(checklist) {
  return { done: checklist.items.filter(({ status }) => status === 'done').length, total: checklist.items.length };
}

export function renderChecklist(checklist) {
  return checklist.items.map(({ id, check, status, evidence }) =>
    `${id}. [${status}] ${check}${evidence ? ` (evidence: ${evidence})` : ''}`).join('\n');
}

// Applies one update_checklist call; throws TypeError for invalid updates so the coder gets a correction.
export function updateChecklist(checklist, args) {
  const updates = args?.items;
  if (!Array.isArray(updates) || !updates.length || updates.length > checklist.items.length) {
    throw new TypeError('update_checklist needs items: [{id, status, evidence}] for existing check numbers');
  }
  const next = checklist.items.map((item) => ({ ...item }));
  for (const update of updates) {
    const item = next.find(({ id }) => id === update?.id);
    if (!item) throw new TypeError(`update_checklist: no check ${JSON.stringify(update?.id)}; use 1-${next.length}`);
    if (!statuses.includes(update.status)) throw new TypeError(`update_checklist: status must be one of ${statuses.join(', ')}`);
    const evidence = oneLine(update.evidence);
    if (['done', 'blocked'].includes(update.status) && !evidence) {
      throw new TypeError(`update_checklist: check ${item.id} marked ${update.status} needs evidence`);
    }
    Object.assign(item, { status: update.status, evidence: evidence || item.evidence });
  }
  if (next.filter(({ status }) => status === 'in_progress').length > 1) {
    throw new TypeError('update_checklist: work one item at a time; at most one item may be in_progress');
  }
  checklist.items = next;
  return `Checklist updated.\n${renderChecklist(checklist)}`;
}

// Closes items a final summary reports per check ("1. done: evidence", "Check 2 - blocked: reason").
export function closeFromSummary(checklist, summary) {
  for (const line of String(summary ?? '').split(/\r?\n/)) {
    const match = /^\s*(?:[-*]\s*)?(?:check\s*)?#?(\d+)\s*[.):-]?\s*\[?(done|met|blocked)\]?\s*[-:—]?\s*(.*)$/i.exec(line);
    const item = match && checklist.items.find(({ id }) => id === Number(match[1]));
    if (!item || !['pending', 'in_progress'].includes(item.status)) continue;
    item.status = /blocked/i.test(match[2]) ? 'blocked' : 'done';
    item.evidence = oneLine(match[3]) || 'stated in the final summary';
  }
  return checklist;
}

export function openChecklistCorrection(checklist) {
  return `Your checklist still has open items:\n${renderChecklist({ items: openItems(checklist) })}\n` +
    'Finish each one, then call update_checklist to mark it done with evidence (file and symbol or test), ' +
    'or blocked with the reason. Then summarize.';
}

export function checklistTable(checklist) {
  const cell = (text) => String(text || '-').replaceAll('|', '\\|');
  return '| # | Check | Status | Evidence |\n|---|---|---|---|\n' +
    checklist.items.map(({ id, check, status, evidence }) =>
      `| ${id} | ${cell(check)} | ${status} | ${cell(evidence)} |`).join('\n') + '\n';
}
