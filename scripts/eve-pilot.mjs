#!/usr/bin/env node
import { openSync, readSync, closeSync, fstatSync } from 'node:fs';
import { PILOT_LIMITS, PilotsError, evaluatePilotPair, buildComparisonSummary } from '../src/lib/eve-pilot.mjs';

const usage = `Usage: node scripts/eve-pilot.mjs --manifest <local.json> [--records <local.json>]
Offline paired comparison; --records overrides the manifest's embedded records.
--help  Show this help successfully. No network or persistence operations.
`;

function load(path) {
  let fd;
  let text;
  try {
    fd = openSync(path, 'r');
    const stat = fstatSync(fd);
    if (!stat.isFile()) throw new PilotsError('io-error', `${path}: expected a regular local JSON file`);
    if (stat.size > PILOT_LIMITS.bytes) throw new PilotsError('invalid-input', `${path}: file exceeds byte bound`);
    const buffer = Buffer.alloc(PILOT_LIMITS.bytes + 1);
    let length = 0;
    while (length < buffer.length) {
      const count = readSync(fd, buffer, length, buffer.length - length, null);
      if (count === 0) break;
      length += count;
    }
    if (length > PILOT_LIMITS.bytes) throw new PilotsError('invalid-input', `${path}: file exceeds byte bound`);
    text = buffer.subarray(0, length).toString('utf8');
  } catch (error) {
    if (error instanceof PilotsError) throw error;
    throw new PilotsError('io-error', `${path}: could not read local JSON (${error.code ?? 'I/O failure'})`);
  } finally {
    if (fd !== undefined) closeSync(fd);
  }
  try {
    return JSON.parse(text);
  } catch {
    throw new PilotsError('invalid-json', `${path}: malformed JSON`);
  }
}

try {
  const args = process.argv.slice(2);
  if (args.length === 1 && args[0] === '--help') {
    process.stdout.write(usage);
  } else {
    const options = {};
    for (let index = 0; index < args.length; index += 2) {
      const flag = args[index];
      if (!['--manifest', '--records'].includes(flag) || options[flag] !== undefined ||
          !args[index + 1] || args[index + 1].startsWith('--')) {
        throw new PilotsError('invalid-arguments', `unknown, duplicate or missing option: ${flag}`);
      }
      options[flag] = args[index + 1];
    }
    if (!options['--manifest']) throw new PilotsError('invalid-arguments', '--manifest is required; use --help');
    const manifest = load(options['--manifest']);
    const records = options['--records'] ? load(options['--records']) : manifest?.records;
    process.stdout.write(`${JSON.stringify(buildComparisonSummary(evaluatePilotPair(manifest, records)), null, 2)}\n`);
  }
} catch (error) {
  process.stderr.write(`eve-pilot: ${error.code ?? 'invalid-input'}: ${error.message}\n`);
  process.exitCode = error.code === 'io-error' ? 2 : 1;
}
