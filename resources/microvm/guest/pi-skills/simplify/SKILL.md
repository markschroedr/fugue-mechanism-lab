---
name: simplify
description: Review changed code for reuse, simplification, efficiency, and correct abstraction level, reporting concrete simpler forms and their costs without editing. Use for cleanup review, complexity reduction, reuse review, or identifying AI-generated code slop after an implementation.
---

# Simplify

Review quality, not correctness. Report improvements and apply nothing; the
parent decides which findings warrant implementation.

## Establish scope

Inspect the requested branch, commit range, files, or current diff. Include
working-tree changes when present. Adjacent support code is relevant only when
needed to understand the changed mechanism.

## Review

Perform the review yourself in one read-only pass. Do not invoke `explore`,
`review`, or any other child agent.

Check all four angles:

1. **Reuse:** Find new code duplicating an existing helper, primitive, or
   pattern. Name the existing thing to reuse.
2. **Simplification:** Find redundant state, copy-paste, deep nesting, dead
   concepts, and avoidable indirection. Name the simpler form.
3. **Efficiency:** Find repeated computation or I/O, unnecessary serialization,
   sequential independent work, and new hot-path or startup waste. Name the
   cheaper form.
4. **Altitude:** Find special cases and shallow fixes that belong in a deeper
   shared mechanism. Name the correct layer and simpler general form.

## Report

Deduplicate and verify findings. For each survivor provide:

- `file:line`
- one-line issue summary
- the simpler form that preserves intended behavior
- the concrete cost of the current form

Drop style-only preferences, correctness bugs, speculative abstractions, and
suggestions whose fix would change intended behavior or open an unrelated
bucket. If the change is already clean, say so. Do not edit files.
