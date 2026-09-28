# Start here in VS Code Insiders (R4yHarris profile)

1. Create empty public repo R4yHarris/github-agent-roster on GitHub.
2. Copy this folder to D:\oss\github-agent-roster
3. Local git identity like contracts:

```powershell
Set-Location D:\oss\github-agent-roster
git init -b main
git config --local user.name "r4yharris"
git config --local user.email "r4yharris@users.noreply.github.com"
git add .
git commit -m "feat: roster scaffold and implementation prompts"
gh auth switch --user R4yHarris
gh repo create github-agent-roster --public --source=. --remote=origin --push
```

4. Open this folder only. Paste **prompts/00-bootstrap.md** into Copilot Agent.
5. Publish that work with contracts `agent-pr.mjs` from a feature branch (sibling clone).
6. Then Prompt 01, 02, 03 in order. One prompt per PR.

Do not install a second App until a Riddle/Hermes box needs its own principal.
Use r4yharris-agent-coder on this public repo only.
