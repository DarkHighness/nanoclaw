# Harness Engineering Alignment

This note keeps `sched-claw` aligned with industrial agent harness patterns
without turning the host into a workflow engine.

## Principles

1. Prefer repository-embedded skills and scripts over host-owned workflows.
   - The agent should gather context and act through normal tools, local scripts,
     and project instructions.
2. Keep the privileged boundary narrow and typed.
   - `sched-claw` should expose a stable daemon protocol for rollout lifecycle
     only, instead of using root as a generic shell escape hatch.
3. Separate reusable harness logic from user surfaces.
   - The agent loop and daemon protocol should stay reusable across CLI and
     future integrations.
4. Put deterministic lifecycle automation in scripts or hooks, not prompt prose.
   - Environment bootstrap, perf collection wrappers, artifact shaping, and
     plotting helpers should be explicit, inspectable, and rerunnable.
5. Improve legibility instead of growing host policy.
   - When the agent struggles, add clearer scripts, better docs, or more
     inspectable artifacts before adding another hard-coded workflow command.

## External references

- OpenAI, "Harness engineering: leveraging Codex in an agent-first world", 2026-02-11
  - https://openai.com/index/harness-engineering/
- OpenAI, "Unlocking the Codex harness: how we built the App Server", 2026-02-04
  - https://openai.com/index/unlocking-the-codex-harness/
- Anthropic, "Hooks reference", accessed 2026-04-23
  - https://code.claude.com/docs/en/hooks
