---
name: review-triad
description: Run a read-only code or specification review with three independent Pi review children covering intent conformance, correctness, and simplification, then synthesize only material findings. Use for larger completed chunks, holistic or multi-agent reviews, and combined correctness plus simplification review.
---

# Review triad

Run one coordinated report-only review. The parent establishes scope,
delegates three lenses in parallel, checks the evidence, and synthesizes the
result. Neither parent nor children edit files during the review.

## Workflow

1. Establish the review scope: relevant commits, working-tree changes, direct
   targets, related code, and the specifications or invariants defining intent.
   Review the whole completed slice rather than only its last commit.
2. Launch exactly three fresh `review` children concurrently with the same base
   context and one lens each:
   - **Intent:** Verify the change satisfies its stated goal without drift,
     overbuilding, or missing the central product or architecture intention.
   - **Correctness:** Find runtime failures, invariant violations, lifecycle or
     data-contract mistakes, unsafe edge cases, and meaningful missing tests.
   - **Simplification:** Find unnecessary concepts, duplication, wrong-layer
     fixes, avoidable work, and existing primitives that should be reused.
3. While they run, inspect enough locally to understand scope and prepare the
   synthesis. Do not perform a redundant fourth full review.
4. Deduplicate results and independently verify every surviving finding against
   current files, lines, diffs, specifications, or observable behavior.
5. Report findings first, ordered by severity or leverage. Include location,
   issue, why it matters, and the smallest robust fix direction. Add brief open
   questions and verification notes. Say plainly when nothing material survives.

Use a prompt shaped like:

```text
Review this scope read-only with the <intent|correctness|simplification> lens.
Repo: <path>
Scope: <commits, diff, or direct targets>
Related code/docs: <paths>
Intent and requirements: <specification paths>

Return only actionable findings with file/line evidence, why each matters, and
the smallest robust fix direction. Do not edit anything. If nothing material
exists, say so.
```

Run all three through `review`; the lens belongs in the task, not in another
agent preset. If delegation is unavailable or this agent is already a child,
perform the three lenses sequentially and state that fan-out was unavailable.
