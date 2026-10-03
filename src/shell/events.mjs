const seatPhases = { planner: 'plan', coder: 'draft', reviewer: 'review' };

const count = (value) => (Number.isSafeInteger(value) && value >= 0 ? value : undefined);

export function createEventSink({ emit, issue = () => null, clock = Date.now, coalesceMs = 200 } = {}) {
  if (typeof emit !== 'function') throw new TypeError('An event sink needs an emit function');
  let phase = null;
  let finishReason = null;
  let current = null;
  let dirty = false;
  let lastToolAt = -Infinity;
  let written = [];
  let streaming = false;

  // Any other printed line ends the growing stream line, so the next delta starts a new one.
  const send = (event) => {
    if (['phase', 'tool', 'verdict', 'wait'].includes(event.kind)) streaming = false;
    return emit(event);
  };

  function flushTool() {
    if (!dirty || current === null) return;
    dirty = false;
    send({ kind: 'tool', name: current.name, target: current.target, count: current.count });
  }

  function sendPhase(name) {
    if (!name || name === phase) return;
    phase = name;
    flushTool();
    current = null;
    send({ kind: 'phase', issue: issue(), phase: name });
  }

  function tool(name, target) {
    const at = clock();
    if (current && current.name === name && current.target === target) current.count += 1;
    else {
      flushTool();
      current = { name, target, count: 1 };
    }
    dirty = true;
    if (at - lastToolAt >= coalesceMs) {
      lastToolAt = at;
      flushTool();
    }
  }

  function verdict(result) {
    flushTool();
    send({ kind: 'verdict', issue: issue(), phase, verdict: result, reason: finishReason,
      files: written.length ? [...written] : null });
    written = [];
    current = null;
    phase = null;
  }

  return {
    get phase() { return phase; },
    flush: flushTool,
    receive(event) {
      if (!event || typeof event !== 'object') return;
      switch (event.type) {
        case 'seat-start':
          written = [];
          current = null;
          sendPhase(seatPhases[event.seat]);
          return;
        case 'tool':
          if (event.name === 'run_test') sendPhase('test');
          tool(event.name, event.path ?? '');
          return;
        case 'wrote':
          if (typeof event.path === 'string' && !written.includes(event.path)) written.push(event.path);
          return;
        case 'waiting':
          flushTool();
          send({ kind: 'wait', seconds: count(event.elapsedSeconds) ?? 0, local: event.local === true });
          return;
        case 'completion':
        case 'finish-reason':
          finishReason = typeof event.reason === 'string' ? event.reason : null;
          return;
        case 'http':
          if (event.phase !== 'start') return;
          streaming = false;
          emit({ kind: 'usage', thinking: typeof event.thinking === 'boolean' ? event.thinking : undefined,
            maxTokens: count(event.maxTokens) });
          return;
        case 'delta': {
          if (typeof event.text !== 'string' || !event.text) return;
          if (!streaming) flushTool();
          const start = !streaming;
          streaming = true;
          send({ kind: 'delta', text: event.text, start });
          return;
        }
        case 'seat-measurement':
          emit({ kind: 'usage', input: count(event.input), output: count(event.output),
            contextMax: count(event.contextMax),
            finishReason: typeof event.finishReason === 'string' ? event.finishReason : undefined });
          return;
        case 'usage':
          emit({ kind: 'usage', input: count(event.input), output: count(event.output),
            cached: count(event.cached) });
          return;
        case 'seat-end':
          if (event.verdict) verdict(event.verdict === 'pass' ? 'pass' : 'fail');
          else flushTool();
          return;
        case 'seat-error':
          verdict('fail');
          return;
        default:
      }
    },
  };
}

export function createShellPainter({ transcript, display, notify = () => {} } = {}) {
  const target = () => (typeof transcript === 'function' ? transcript() : transcript);
  return (event) => {
    switch (event.kind) {
      case 'phase':
        target()?.phase(event.issue, event.phase);
        return;
      case 'tool':
        target()?.tool(event.name, event.target, event.count);
        return;
      case 'delta':
        target()?.stream(event.text, { start: event.start === true });
        return;
      case 'wait':
        target()?.waiting([`${event.seconds}s`,
          event.local ? 'local hardware can take minutes after idle' : '']);
        return;
      case 'verdict':
        target()?.verdict([`${event.issue === null || event.issue === undefined ? 'local' : `#${event.issue}`} ` +
          `${event.phase ?? 'run'}`, event.verdict, event.reason ?? '',
        event.files ? event.files.join(' ') : 'no write'], { pass: event.verdict === 'pass' });
        return;
      case 'usage': {
        if (!display) return;
        if (event.input !== undefined) display.contextUsed = event.input;
        if (event.output !== undefined) display.outputTokens = event.output;
        if (event.cached !== undefined) display.cachedTokens = event.cached;
        if (event.contextMax !== undefined) display.contextMax = event.contextMax;
        if (event.finishReason !== undefined) display.lastFinishReason = event.finishReason;
        if (event.thinking !== undefined) display.thinking = event.thinking;
        if (event.maxTokens !== undefined) display.maxTokens = event.maxTokens;
        notify();
        return;
      }
      default:
    }
  };
}
