import { request as httpRequest } from 'node:http';
import { request as httpsRequest } from 'node:https';
import { Readable } from 'node:stream';
import { createBrotliDecompress, createGunzip, createInflate } from 'node:zlib';

export function nodeFetch(input, { method = 'GET', headers = {}, body, signal } = {}) {
  const url = new URL(input);
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password) {
    throw new TypeError('LLM transport requires an HTTP(S) URL without credentials');
  }
  const request = url.protocol === 'https:' ? httpsRequest : httpRequest;
  return new Promise((resolve, reject) => {
    const outgoing = request(url, { method, headers: { 'Accept-Encoding': 'identity', ...headers },
      signal, agent: false, timeout: 0 }, (incoming) => {
      try {
        const responseHeaders = new Headers();
        for (const [name, value] of Object.entries(incoming.headers)) {
          for (const entry of Array.isArray(value) ? value : value === undefined ? [] : [value]) {
            responseHeaders.append(name, entry);
          }
        }
        let stream = incoming;
        const decoder = { gzip: createGunzip, deflate: createInflate, br: createBrotliDecompress }[
          incoming.headers['content-encoding']?.toLowerCase()
        ];
        if (decoder) {
          stream = incoming.pipe(decoder());
          incoming.on('error', (error) => stream.destroy(error));
          stream.on('close', () => incoming.destroy());
        }
        resolve(new Response([204, 205, 304].includes(incoming.statusCode) ? null : Readable.toWeb(stream), {
          status: incoming.statusCode, statusText: incoming.statusMessage, headers: responseHeaders,
        }));
      } catch (error) {
        incoming.destroy();
        reject(error);
      }
    });
    outgoing.on('error', reject);
    outgoing.on('upgrade', (_response, socket) => {
      socket.destroy();
      reject(new Error('LLM HTTP upgrades are not supported'));
    });
    outgoing.end(body);
  });
}

export function defaultRequestFetch(timeoutMs) {
  // Node 20 fetch has a five-minute headers/body deadline even with a longer abort timer.
  return timeoutMs > 300_000 ? nodeFetch : globalThis.fetch;
}
