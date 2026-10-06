// node --test reporter used by scripts/run-tests.mjs: one JSON summary of counts and failing test names.
export default async function* summary(source) {
  const result = { tests: 0, pass: 0, fail: 0, skipped: 0, todo: 0, failures: [] };
  for await (const event of source) {
    if (event.type !== 'test:pass' && event.type !== 'test:fail') continue;
    if (event.data.details?.type === 'suite') continue;
    result.tests += 1;
    if (event.data.skip !== undefined) result.skipped += 1;
    else if (event.data.todo !== undefined) result.todo += 1;
    else if (event.type === 'test:pass') result.pass += 1;
    else {
      result.fail += 1;
      result.failures.push(event.data.name);
    }
  }
  yield `${JSON.stringify(result)}\n`;
}
