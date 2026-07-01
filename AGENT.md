# AGENT Operating Guide for This Repository

## Purpose

Keep the development plan in project.md aligned with the actual implementation state.

## Primary rule

For every implementation step, update the Tasks section in project.md in the same work cycle.
Do not defer task documentation updates to a later step.

## Update workflow (required each step)

1. Identify which mainline task number is affected.
2. Update only the impacted task entries:
   - Status: pending | in progress | completed
   - Implemented or Planned bullets
   - Remaining verification if needed
3. If a new stream of work appears, add a new numbered mainline task at the end.
4. Keep wording short, factual, and testable.
5. Preserve existing numbering for unchanged tasks.

## Task state policy

Each task entry in project.md MUST use exactly this field name:
- Status: <pending|in progress|completed>

- pending: not started yet
- in progress: partially implemented or under validation
- completed: implemented and validated at least by build/check, and by user confirmation when available

## Definition of done for a step

A step is considered done only if all are true:
- Code or document change is applied.
- Relevant checks are run (for example npm run check and/or cargo check when code changed).
- project.md Tasks is updated to reflect the latest state.

## Standard checklist before final response

- Confirm changed files list.
- Confirm task status changes in project.md.
- Mention any remaining risks or follow-up explicitly.

## Scope note

This file defines process behavior for the coding agent in this repository.
It does not change product requirements; project.md remains the source of product plan and scope.
