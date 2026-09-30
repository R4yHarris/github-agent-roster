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
