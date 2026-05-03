# FFF agent skills

This directory contains ready-to-copy skills for assistants that support local
skill folders. The skill teaches agents to use `fffq` first for repository file
and content search.

## Codex

Recommended global install path:

```bash
mkdir -p ~/.codex/skills/fffq
cp skills/codex/fffq/SKILL.md ~/.codex/skills/fffq/SKILL.md
```

## Claude Code

Recommended global install path:

```bash
mkdir -p ~/.claude/skills/fffq
cp skills/claude/fffq/SKILL.md ~/.claude/skills/fffq/SKILL.md
```

## Runtime expectation

The skill uses the short-lived `fffq` CLI. `fffq ensure` starts or reuses a
per-project `fff-mcp --transport streamable-http` sidecar and records the
registry under the user's cache directory. Agents should not configure a global
long-lived FFF sidecar for normal repo work.
