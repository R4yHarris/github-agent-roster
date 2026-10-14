import assert from 'node:assert/strict';
import test from 'node:test';
import { discoverEndpoints, localCandidates, wslCandidates } from '../src/onboard/discover.mjs';
import { mergeDiscoveredProfiles } from '../src/lib/fleet-discovery.mjs';

const profile = { id: 'preserved', base_url: 'http://127.0.0.1:8000/v1', model: 'old-model',
  provider: 'vllm', context_max: 32768, concurrency: 2, hardware: 'declared-gpu', notes: 'keep' };

test('WSL candidates enumerate running distributions and return routable IPv4 candidates without curl', async () => {
  const calls = [];
  const urls = await wslCandidates({ run: async (program, args, options) => {
    calls.push(args);
    assert.equal(program, 'wsl.exe');
    if (args.includes('--list')) {
      assert.ok(args.includes('--running'));
      assert.equal(options.encoding, 'utf16le');
      return { stdout: 'Ubuntu\r\nUbuntu-22.04\r\ndocker-desktop\r\n' };
    }
    assert.deepEqual(args.slice(-3), ['--exec', 'hostname', '-I']);
    return { stdout: args[1] === 'Ubuntu' ? '172.20.0.2 fd00::2\n' : '192.168.1.159\n' };
  } });
  assert.equal(calls.length, 3);
  assert.ok(urls.includes('http://172.20.0.2:11435/v1'));
  assert.ok(urls.includes('http://192.168.1.159:8080/v1'));
  assert.ok(!urls.some((url) => url.includes('fd00')));
  assert.ok(localCandidates().includes('http://127.0.0.1:8080/v1'));
});

test('discovery retains a success beside refused ports, bounds deadlines and never sends ambient credentials', async () => {
  const failed = [];
  const found = await discoverEndpoints({
    env: { ROSTER_API_KEY: 'secret-sentinel' },
    candidates: [profile.base_url, 'http://127.0.0.1:8001/v1', profile.base_url],
    onFailure: (error) => failed.push(error),
    fetchImpl: async (url, options) => {
      assert.equal(options.method, 'GET');
      assert.equal(options.headers.Authorization, undefined);
      assert.equal(options.redirect, 'error');
      if (url.includes(':8001')) throw new Error('refused');
      return Response.json({ data: [{ id: 'new-model', max_model_len: 65536 }] });
    },
  });
  assert.equal(found.length, 1);
  assert.equal(found[0].models[0].context_max, 65536);
  assert.equal(failed.length, 1);
});

test('merge is additive, idempotent, preserves operator profiles and does not invent capacity', () => {
  const endpoints = [{ baseUrl: profile.base_url, models: [
    { id: profile.model, context_max: 1 }, { id: 'new', context_max: 65536 }, { id: 'unknown' },
  ] }];
  const merged = mergeDiscoveredProfiles({ profiles: [profile] }, endpoints);
  assert.equal(merged.profiles.length, 2);
  assert.deepEqual(merged.profiles[0], profile);
  assert.equal(merged.profiles[1].hardware, 'unspecified');
  assert.deepEqual(mergeDiscoveredProfiles(merged, endpoints), merged);
});

test('Ollama configured num_ctx wins over architecture metadata; missing setting remains unknown', async () => {
  const found = await discoverEndpoints({
    candidates: ['http://127.0.0.1:11435/v1'],
    fetchImpl: async (url, options) => {
      assert.equal(options.headers.Authorization, undefined);
      if (url.endsWith('/v1/models')) return Response.json({ data: [{ id: 'configured' }, { id: 'unreported-model' }] });
      assert.equal(options.method, 'POST');
      const { model } = JSON.parse(options.body);
      return Response.json({ parameters: model === 'configured' ? 'num_ctx 65536\n' : '',
        model_info: { 'architecture.context_length': 262144, 'architecture.parameter_count': 27000000000 } });
    },
  });
  assert.equal(found[0].models[0].context_max, 65536);
  assert.equal(found[0].models[1].context_max, undefined);
});

test('discovery deadline aborts a server that never answers', async () => {
  const failed = [];
  const started = Date.now();
  const found = await discoverEndpoints({
    candidates: [profile.base_url], onFailure: (failure) => failed.push(failure),
    fetchImpl: (_url, { signal }) => new Promise((_resolve, reject) => {
      signal.addEventListener('abort', () => reject(new Error('aborted')), { once: true });
    }),
  });
  assert.equal(found.length, 0);
  assert.equal(failed[0].reason, 'timeout');
  assert.ok(Date.now() - started < 10_000);
});
