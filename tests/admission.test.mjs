import test from "node:test";
import assert from "node:assert/strict";
import { configure, acquire, release, depth } from "../src/runtime/admission.mjs";

// 1. acquire resolves only after a slot frees; rejects cleanly on AbortSignal.
test("acquire resolves after slot frees", async () => {
  configure("a1", 1);
  const p1 = acquire("a1");
  const p2 = acquire("a1");
  assert.equal(depth("a1"), 1);
  let resolved = false;
  p2.then(() => { resolved = true; });
  await new Promise((r) => setTimeout(r, 10));
  assert.equal(resolved, false, "p2 must not resolve before release");
  release("a1");
  await p2;
  assert.equal(resolved, true);
  release("a1");
});

test("acquire rejects cleanly on already-aborted signal", async () => {
  configure("a2", 1);
  await acquire("a2");
  const ctrl = new AbortController();
  ctrl.abort();
  await assert.rejects(
    acquire("a2", { signal: ctrl.signal }),
    (err) => err.name === "AbortError",
  );
  // Slot was not consumed by the aborted acquire.
  assert.equal(depth("a2"), 0);
  release("a2");
});

// 2. FIFO wake; double release is a no-op that never over-admits.
test("release wakes FIFO order; double release is a no-op", async () => {
  configure("b1", 1);
  const p1 = acquire("b1");
  const p2 = acquire("b1");
  const p3 = acquire("b1");
  release("b1"); // wakes p2
  const order = [];
  p2.then(() => order.push("p2"));
  p3.then(() => order.push("p3"));
  await new Promise((r) => setTimeout(r, 10));
  assert.deepEqual(order, ["p2"], "FIFO: p2 must be first");
  release("b1"); // wakes p3
  await p3;
  assert.deepEqual(order, ["p2", "p3"]);
  // Double release on an empty profile: inFlight is 0, must not go negative
  // and must not create phantom capacity.
  release("b1"); // legitimate
  release("b1"); // double: no-op
  const p4 = acquire("b1"); // should resolve immediately (1 free slot)
  assert.equal(depth("b1"), 0, "after double release no over-admission");
  release("b1");
});

test("concurrency 2 runs four requests in FIFO order with at most two in flight (#345)", async () => {
  configure("fifo-four", 2);
  let inFlight = 0;
  let maxInFlight = 0;
  const started = [];
  const finished = [];
  const gates = new Map();
  const work = async (name) => {
    const done = await acquire("fifo-four");
    inFlight += 1;
    maxInFlight = Math.max(maxInFlight, inFlight);
    started.push(name);
    await new Promise((resolve) => gates.set(name, resolve));
    inFlight -= 1;
    finished.push(name);
    done();
  };
  const runs = ["r1", "r2", "r3", "r4"].map(work);
  await new Promise((r) => setTimeout(r, 10));
  assert.deepEqual(started, ["r1", "r2"]);
  assert.equal(depth("fifo-four"), 2);
  for (const name of ["r1", "r2", "r3", "r4"]) {
    gates.get(name)();
    await new Promise((r) => setTimeout(r, 10));
  }
  await Promise.all(runs);
  assert.deepEqual(started, ["r1", "r2", "r3", "r4"]);
  assert.deepEqual(finished, ["r1", "r2", "r3", "r4"]);
  assert.equal(maxInFlight, 2);
  assert.equal(depth("fifo-four"), 0);
});

// 3. depth never negative; correct formula.
test("depth returns pending + inFlight - capacity, never negative", () => {
  configure("c1", 3);
  assert.equal(depth("c1"), 0);
  assert.equal(depth("unknown-profile"), 0);
  const p1 = acquire("c1");
  const p2 = acquire("c1");
  const p3 = acquire("c1");
  const p4 = acquire("c1");
  const p5 = acquire("c1");
  assert.equal(depth("c1"), 2); // 5 total, capacity 3 -> 2 pending
  p1; p2; p3; p4; p5; // keep refs
  release("c1");
  assert.equal(depth("c1"), 1);
  release("c1");
  assert.equal(depth("c1"), 0);
  release("c1");
  // Even after all in-flight done, depth must not go negative.
  release("c1");
  release("c1");
  release("c1");
  release("c1");
  assert.equal(depth("c1"), 0);
});

// 4. Abort during wait removes waiter, does not consume a slot.
test("abort during wait removes waiter and does not consume slot", async () => {
  configure("d1", 1);
  const p1 = acquire("d1");
  const ctrl = new AbortController();
  const p2 = acquire("d1", { signal: ctrl.signal });
  // Waiter is queued.
  assert.equal(depth("d1"), 1);
  // Abort the waiter; p2 will reject with AbortError.
  const aborted = p2.then(
    () => assert.fail("should have rejected"),
    (err) => { assert.equal(err.name, "AbortError"); },
  );
  ctrl.abort();
  await aborted;
  await new Promise((r) => setTimeout(r, 10));
  // Depth should drop: waiter removed, inFlight still 1, capacity 1 -> 0.
  assert.equal(depth("d1"), 0);
  // Release the holder; no phantom waiter should be woken.
  release("d1");
  // Profile is now free: a new acquire should resolve immediately.
  const p3 = acquire("d1");
  await p3;
  assert.equal(depth("d1"), 0);
  release("d1");
  await p1;
});

// 5. Independent queues and limits per profileId.
test("distinct profileIds maintain independent queues and limits", async () => {
  configure("e1", 1);
  configure("e2", 2);
  const pa1 = acquire("e1");
  const pa2 = acquire("e1");
  const pb1 = acquire("e2");
  const pb2 = acquire("e2");
  const pb3 = acquire("e2"); // queued on e2 (capacity 2)
  assert.equal(depth("e1"), 1);
  assert.equal(depth("e2"), 1);
  // Release e1 -> wakes only pa2.
  release("e1");
  await pa2;
  // e2 unaffected: pb1, pb2 still in-flight, pb3 still queued.
  assert.equal(depth("e2"), 1);
  release("e1");
  // Release e2 -> wakes pb3 only.
  release("e2");
  await pb3;
  assert.equal(depth("e2"), 0);
  release("e2");
  release("e2");
});

test("holder release handle is one-shot and cannot free another holder's slot", async () => {
  configure("f1", 2);
  const releaseA = await acquire("f1");
  await acquire("f1");
  const third = acquire("f1");
  let admitted = false;
  third.then(() => { admitted = true; });
  releaseA();
  releaseA();
  await third;
  assert.equal(admitted, true);
  const fourth = acquire("f1");
  let overAdmitted = false;
  fourth.then(() => { overAdmitted = true; });
  await new Promise((r) => setImmediate(r));
  assert.equal(overAdmitted, false, "a repeated holder release must not admit beyond capacity");
  assert.equal(depth("f1"), 1);
});

test("admitted waiter detaches its abort listener", async () => {
  configure("g1", 1);
  const releaseFirst = await acquire("g1");
  const ctrl = new AbortController();
  let removed = 0;
  const remove = ctrl.signal.removeEventListener.bind(ctrl.signal);
  ctrl.signal.removeEventListener = (type, fn) => { removed += 1; remove(type, fn); };
  const waiting = acquire("g1", { signal: ctrl.signal });
  releaseFirst();
  const releaseSecond = await waiting;
  assert.equal(removed, 1);
  ctrl.abort();
  releaseSecond();
  assert.equal(depth("g1"), 0);
});

test("profileId must be a non-empty string", () => {
  assert.throws(() => acquire(""), TypeError);
  assert.throws(() => configure(undefined, 1), TypeError);
});

// 6. No external dependencies beyond an in-process Map — verified by
//    inspecting the module's import list (only node builtins in this test).
test("module has no external imports", async () => {
  const { readFile } = await import("node:fs/promises");
  const { fileURLToPath } = await import("node:url");
  const src = await readFile(fileURLToPath(new URL("../src/runtime/admission.mjs", import.meta.url)), "utf8");
  const importRe = /^import\s+.*from\s+['"]([^'"]+)['"]/gm;
  const imports = [...src.matchAll(importRe)].map((m) => m[1]);
  for (const dep of imports) {
    assert.ok(
      dep.startsWith("node:") || dep.startsWith("."),
      `unexpected external import: ${dep}`,
    );
  }
});
