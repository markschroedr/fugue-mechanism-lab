---
name: pi-delegate-readonly
description: Delegate bounded read-only exploration and review from a parent Pi session through pi-subagents while keeping all implementation in the parent. Use when Pi should investigate a codebase or external evidence, review code or plans, challenge assumptions, or gather up to three independent opinions without allowing child agents to edit.
---

# Pi read-only delegation

Use only two child roles:

- `explore`: Luna at high thinking for codebase discovery, architecture,
  documentation, external research, and unfamiliar problem spaces.
- `review`: Sol at medium thinking for correctness, tests, simplification,
  security, plan review, and adversarial challenges. Put the desired review
  angle directly in the task.

The parent Pi agent owns every edit, state-changing command, implementation
decision, integration step, and final verification. All packaged roles are
disabled. Do not re-enable or invoke them.

## Boundaries

- Run at most three independent children concurrently.
- Keep delegation depth at one; children never delegate.
- Give each child one bounded question and require evidence and file paths.
- Use fresh context for independent review. Fork context only when the child
  genuinely needs the parent conversation.
- Do not use worktrees or tmux for ordinary delegation. The extension already
  manages subprocesses, status, interruption, and results.
- Treat child output as advice. The parent resolves disagreements against the
  current repository and implements accepted changes itself.

## Examples

```text
Use explore to map the ingestion flow without editing anything.
Use review to challenge the assumptions in this plan.
Run three fresh review agents in parallel: correctness, tests, and simplicity.
```

```text
/run explore "Map the ingestion flow; cite files and remain read-only"
/run review "Challenge this design from first principles; remain read-only"
/parallel review "Correctness review; read-only" -> review "Test-gap review; read-only" -> review "Simplification review; read-only"
```

Append `--bg` for background work. Wait for actual results and never invent a
child's output.
