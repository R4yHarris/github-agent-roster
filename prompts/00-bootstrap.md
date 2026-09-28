# Prompt 00 — bootstrap (human + Copilot)

Stay in this workspace. Do not edit github-agent-contracts.

Create the empty package shape:

- package.json name github-agent-roster, type module, engines node >=20, MIT
- src/cli.mjs that prints help: roster --help
- src/lib/paths.mjs resolving GITHUB_AGENT_CONTRACTS or ../github-agent-contracts
- tests/paths.test.mjs
- README status line that 00 is the scaffold

Run node --test. Do not publish unless App env is set; then use sibling contracts agent-pr.mjs.
