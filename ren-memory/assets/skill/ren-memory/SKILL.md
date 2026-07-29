---
name: ren-memory
description: >
  Capture, retrieve, inspect, connect, curate, and diagnose project knowledge
  with the local ren memory vault. Use when the user asks to remember or capture
  information; search, list, show, relate, or export notes; sync or rebuild the
  memory index; promote, link, revise, or archive knowledge; initialize a vault;
  manage memory hooks; diagnose memory health; or run `ren memory`. Trigger
  phrases include "ren memory", "remember this", "capture a note", "search
  memory", "memory vault", "promote notes", and "memory hook".
---

# ren memory

Use `ren memory` to manage durable Markdown knowledge and its disposable search
index. Treat the installed binary as the version-matched source of truth.

## Inspect the CLI first

Run help before choosing commands or flags:

```bash
ren memory --help
ren memory <subcommand> --help
```

Do not infer syntax from this skill. If `ren` is unavailable, ask the user to
install or expose it on `PATH`.

## Work with memory

1. Resolve the requested operation from current CLI help.
2. Prefer read-only discovery (`search`, `list`, `show`, graph queries, or
   `doctor`) before making changes.
3. Use `sync` or `index` when the disposable index may be stale; keep Markdown
   as the durable source of truth.
4. Let the current directory select the registered vault when unambiguous.
   Consult help and pass an explicit vault when needed.
5. Treat JSON command output as authoritative and report exact note IDs,
   operation keys, and diagnostics that matter.

For promotion, generate and inspect the proposal before applying it. Apply a
proposal only when the user has authorized the mutation, and use the exact
operation key returned by the CLI. Apply the same care to link, revise, archive,
and hook changes.

## Keep initialization boundaries clear

- Use top-level `ren init` to install the embedded ren skills for supported
  agents and scopes.
- Use `ren memory init --user` only to initialize the memory home and register a
  project vault. It does not install agent skills.
