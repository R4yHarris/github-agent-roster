# Onboarding wizard

1. Run `roster onboard` in a terminal. The wizard displays the platform.
   Non-TTY input or output prints `roster onboard needs a terminal` and
   exits 2 without writing files.
2. Choose the LLM endpoint and an actual model. Prompts show defaults in
   brackets; blank accepts a default. A required model has no invented
   default and must be supplied when discovery is unavailable.
3. Answer local permission questions. Advanced settings are hidden unless
   requested. These preferences are not GitHub policy grants.
4. Review the endpoint, model, permissions, and destination. Answer
   `Confirm write .roster/config.yml? [yes]` before any config or ignore
   rule is written. Existing config also requires explicit overwrite
   consent; declining either confirmation preserves it.
5. After a confirmed atomic write, read the doctor result. Ctrl+C before
   write confirmation exits 0 without a half-finished config.

For installation commands, see [INSTALL.md](INSTALL.md). The implementation
is [src/onboard/wizard.mjs](../src/onboard/wizard.mjs); neither the wizard nor
its local preferences changes App policy, vendor sources, or workflows.

## Probing /v1/models

The endpoint step defaults to `http://127.0.0.1:8000/v1`. WSL talking to a
Windows-hosted server may need the Windows host IP instead of localhost.
Enter the API base URL, not the full models or chat-completions endpoint.

The wizard GETs `<base_url>/models` with a five-second total deadline,
including JSON parsing. It reads the OpenAI list shape `data[].id`,
prints the actual IDs, and defaults to the first one. A failed probe prints
only a safe class (`timeout`, `refused`, `HTTP <status>`, `network`, or
`invalid response`) before asking for the actual served model ID.
It never displays an upstream body or error details, invents an ID, or
uses `unknown` as a fallback.

Confirmed ignored configuration records `llm.profile: vllm-local`,
the chosen `llm.base_url` and `llm.model`, `llm.provider: vllm`, and
`llm.api_key_optional: true`. A custom vLLM host is not replaced with
loopback when the config is read. Discovery is not a chat or internet-tool
probe; no network tool is added to the coder.
