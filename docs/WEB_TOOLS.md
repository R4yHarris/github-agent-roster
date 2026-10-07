# Web research controls

`web_search` and `web_fetch` stay available when `tools.internet` is true. The page is data, not a command.

A docs-only or README-only change does not run `node --test`. A code change runs only the allowed test file or `tests/<changed-name>.test.mjs`. Failures outside that list are baseline failures. The harness asks before starting a separate remediation agent; it does not switch the current session.
- Fetch reads the first 256 KiB of a searched URL and returns the page title plus 8,000 characters. A large page is truncated, not refused.
- Fetch accepts only a URL returned by `web_search` in the same task. A guessed URL is refused.
- Redirects are followed manually, at most three hops, and only to the same host. Each hop is checked again. Private, link-local, and localhost targets are refused.
- Only `text/html` and `text/plain` are read, and only up to 256 KiB. The returned text is marked untrusted.
- A task that asks for web research does not receive `run_command`.
- The coder receives `run_command` only when TASK.md names `run_command`, `git status`, or `git diff` (and asks for no web research), or when `seat.tools` lists it. Web tools still need both `tools.internet` and the tool named in TASK.md. The coder's first live log line records the offered toolset (`toolset read_file,write_file,...`).
