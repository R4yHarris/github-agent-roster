# Human-owned policy note

This is guidance, **not** `agent-policy.yml`. `roster init` never creates or
rewrites the repository's policy file. A human must review and install policy
at the repository root and keep it consistent with the reviewed default
branch. Do not change policy to bypass a publisher denial.

An illustrative capability layout for review is:

```yaml
version: 1
default: deny
roles:
  coder:
    allow: [commit_branch, open_pr, comment, label]
  merger:
    allow: [merge]
  deploy:
    allow: []
```

Grant `merger.merge` only if the human approves App-backed
`--merge-when-green`; review the repository's checks and branch protections.
The human also owns `.github/workflows/*`. Never commit App PEMs, tokens,
or `.env` files. This note is not a credential or a policy override.
