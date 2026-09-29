# Codex orchestration

This repository uses the Pro profile from the desktop `codex-astra-luna-orchestrator` package. Start a **new** Codex session from the repository root so Codex loads the project configuration, agent roles, skill, and `AGENTS.md`:

```sh
cd /Users/kaanakin/Desktop/turk-binary-v2
codex
```

For a complex task, invoke the skill explicitly with `$astra-orchestrator` in the prompt. The project configuration selects GPT-6 Astra at medium reasoning for the root. The `explorer`, `worker`, `tester`, and `researcher` roles select GPT-5.6 Luna at max reasoning; `reviewer` selects GPT-6 Astra at low reasoning. The skill decides when to delegate and requires explicit model selection for each subagent.

The installed Codex CLI 0.135.0 rejects the package's `[agents]` scalar settings. This repository therefore registers the five roles through `.codex/config.toml` and enables `multi_agent` under `[features]`. Its role files and skill are copied from the Pro package. Generic subagents have no project-level Luna default on this CLI version; use the named roles or invoke the skill so the model is explicit.

Codex loads project configuration only for trusted projects. This repository is trusted in the current user's global Codex configuration. A running session keeps its existing model and configuration until restarted.

The instructions in `AGENTS.md` apply to root and subagents. In particular, no agent may submit a transaction to mainnet or read the root `.env` file or a keypair.
