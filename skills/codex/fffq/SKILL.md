---
name: fffq
description: Use FFF from Codex for repository file discovery and exact text lookup; prefer the short-lived fffq CLI before shell find/grep/rg.
---

# fffq for Codex

Use this skill for repository or codebase inspection tasks: file discovery,
exact text lookup, symbol-name lookup, and quick grep-style context.

## Rules

1. At the start of serious repo work, run:

   ```bash
   fffq ensure
   ```

   If the user names a repo/path/module outside the current shell directory,
   do not keep searching only from the broad current root. First locate the
   target with `fffq find <name>` if needed, then switch to the target source
   root with `fffq -C <target-path-or-repo> ensure` and run subsequent
   `fffq -C <target-path-or-repo> ...` searches there. `fffq -C` resolves
   nested git worktrees to their own repo root, so this avoids starting one
   broad sidecar for a parent directory like `~/Projects/playground` when the
   actual task is in a nested checkout.

2. Use `fffq` before shell `find`, `grep`, `rg`, `sed`, or `awk` for repo search.

3. When searching for several exact terms in the same root, prefer one
   `fffq multi-grep term1 term2 ...` call over repeated
   `fffq grep term1 || true; fffq grep term2 || true; ...`. Use separate
   `grep` calls only when each term needs different constraints/output, or when
   independent per-term ranking/absence is important. Do not add `|| true`
   unless masking a non-zero exit is explicitly intended and explained.

4. Use repo-relative constraints where possible.

5. Do not use stdio MCP as the normal path. `fffq` manages a per-project
   `fff-mcp --transport streamable-http` sidecar.

6. If `fffq` is unavailable or cannot see the target root, say that explicitly,
   then fall back to shell search.

## Commands

```bash
fffq ensure
fffq -C /path/to/target/repo ensure
fffq doctor
fffq find Cargo.toml -n 10
fffq -C /path/to/target/repo grep SomeIdentifier -n 20
fffq grep SomeIdentifier -n 20
fffq grep SomeIdentifier --output-mode path -n 50
fffq multi-grep MemoryBank claurst omx codex -n 50
fffq multi-grep SomeIdentifier some_identifier --constraints '*.rs' -n 20
fffq multi-grep foo bar baz --constraints 'src/**/*.ts' --context 2 -n 50
```

Use `fffq --no-start doctor` when verifying an already-running sidecar without
hiding failures by auto-starting a new one.

## Expected sidecar

The sidecar command should look like:

```bash
fff-mcp --transport streamable-http --http-bind 127.0.0.1:0 --http-path /mcp --registry-path ...
```

If process inspection shows `fff-mcp` without `--transport streamable-http`, it
is an old stdio/global session and should not be treated as the desired runtime.
