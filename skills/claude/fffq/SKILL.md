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

3. When searching for several exact terms in the same root, prefer one
   `fffq multi-grep term1 term2 ...` call over repeated
   `fffq grep term1 || true; fffq grep term2 || true; ...`. Use separate
   `grep` calls only when each term needs different constraints/output, or when
   independent per-term ranking/absence is important. Do not add `|| true`
   unless masking a non-zero exit is explicitly intended and explained.

4. Use repo-relative constraints where possible.

5. Do not rely on stdio MCP for normal FFF usage. `fffq` starts/reuses a
   per-project `fff-mcp --transport streamable-http` sidecar.

6. If FFF is unavailable, stale, or outside the current root, report that and
   fall back honestly.

## Commands

```bash
fffq ensure
fffq doctor
fffq find Cargo.toml -n 10
fffq grep SomeIdentifier -n 20
fffq grep SomeIdentifier --output-mode path -n 50
fffq multi-grep MemoryBank claurst omx codex -n 50
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
