---
name: fffq
description: Use FFF from Claude Code for repository file discovery and exact text lookup; prefer the short-lived fffq CLI before shell find/grep/rg.
---

# fffq for Claude Code

Use this skill for repository or codebase inspection tasks: file discovery,
exact text lookup, symbol-name lookup, and quick grep-style context.

## Rules

1. At the start of serious repo work, run:

   ```bash
   fffq ensure
   ```

2. Prefer `fffq find`, `fffq grep`, and `fffq multi-grep` before shell search.

3. Use repo-relative constraints where possible.

4. Do not rely on stdio MCP for normal FFF usage. `fffq` starts/reuses a
   per-project `fff-mcp --transport streamable-http` sidecar.

5. If FFF is unavailable, stale, or outside the current root, report that and
   fall back honestly.

## Commands

```bash
fffq ensure
fffq doctor
fffq find Cargo.toml -n 10
fffq grep SomeIdentifier -n 20
fffq grep SomeIdentifier --output-mode path -n 50
fffq multi-grep SomeIdentifier some_identifier --constraints '*.rs' -n 20
fffq multi-grep foo bar baz --constraints 'src/**/*.ts' --context 2 -n 50
```

## When to use shell fallback

Only after `fffq` is unavailable or insufficient. Good reasons:

- FFF binary missing.
- FFF root does not include the requested path.
- Sidecar is stale and cannot restart.
- The task is non-repository filesystem surgery rather than code search.

When falling back, state the reason briefly.
