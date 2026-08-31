# TASKS.md

## Project objective

Develop a Windows desktop application for registering, organizing, previewing, classifying, searching, grouping, and safely deduplicating media files represented as image-oriented containers, including images, videos, folders, and archives.

## Scope and constraints

- In scope: registration, metadata persistence, thumbnails, search, classification, container hierarchy, maintenance, and user-driven duplicate management.
- Out of scope: automatic duplicate deletion and unconfirmed destructive cleanup.
- Deployment target: Windows desktop using Tauri v2, SvelteKit, TypeScript, Rust, and SQLite.
- Important constraints: ffmpeg.exe and ffprobe.exe are available in PATH; archive extraction depends on 7z or 7za; long-running operations must preserve UI responsiveness; executable and UI milestones require user verification when not fully testable by the agent.

## Current status

The project has a buildable desktop baseline with completed registration, persistence, classification search, container modeling, registration progress controls, image/video/archive previews, thumbnail maintenance, background thumbnail jobs, archive diagnostics, and archive-derived virtual container hierarchy. Combined-container UX and nested archive policy are in progress. Dedupe flow, classification schema evolution, scale hardening, release reliability, datetime filtering, archive video previews, and migration cleanup remain planned.

## Active task

- Task: T011
- Current step: Continue combined-container creation/editing and search semantics beyond current search-only integration.
- Next verification: Run configured frontend and Rust checks, build executable path as needed, and obtain user confirmation for affected UI behavior.
- Waiting on: agent
- Required action: none

## Tasks

Use stable IDs. Append newly discovered work using the next unused ID. Never reuse an ID or renumber existing tasks without explicit user approval.

### T001 - Establish project scaffold

- Status: completed
- Objective: Produce a buildable desktop application baseline.
- Scope:
  - Initialize Rust and Tauri v2 backend with SvelteKit and TypeScript frontend.
  - Initialize SQLite and local application data storage.
  - Wire basic frontend interaction to backend commands.
- Acceptance criteria:
  - The objective is implemented without violating project constraints.
  - Relevant configured checks pass, and user verification is recorded where required.
- Implemented:
  - Rust and Tauri backend plus SvelteKit frontend were initialized.
  - SQLite initialization and local storage were implemented.
  - Basic UI to backend command wiring was implemented.
- Verification:
  - Source project record marks executable baseline completed.
- Remaining:
  - None.
- Notes:
  - None.

### T002 - Implement registration and persistence

- Status: completed
- Objective: Register media files from folders and persist stable core metadata.
- Scope:
  - Scan folders and register files.
  - Persist hash, path, filename, size, and timestamps.
  - Detect moves by hash while preserving non-path metadata.
- Acceptance criteria:
  - The objective is implemented without violating project constraints.
  - Relevant configured checks pass, and user verification is recorded where required.
- Implemented:
  - Folder scanning and DB registration were implemented.
  - Core metadata persistence and hash-based move handling were implemented.
- Verification:
  - Source project record marks workflow completed.
- Remaining:
  - None.
- Notes:
  - Registration runs in a blocking worker context to keep UI responsive.

### T003 - Implement search and classification

- Status: completed
- Objective: Allow users to find files and edit tags and ratings.
- Scope:
  - Search by path, filename, tags, and minimum rating.
  - Edit classification metadata.
- Acceptance criteria:
  - The objective is implemented without violating project constraints.
  - Relevant configured checks pass, and user verification is recorded where required.
- Implemented:
  - Search and classification editing were implemented.
- Verification:
  - Source project record marks workflow completed.
- Remaining:
  - None.
- Notes:
  - None.

### T004 - Model containers and grouping

- Status: completed
- Objective: Represent media through containers and parent-child group relationships.
- Scope:
  - Persist containers and relationships.
  - Create and maintain groups.
  - Show recent containers and expandable children.
- Acceptance criteria:
  - The objective is implemented without violating project constraints.
  - Relevant configured checks pass, and user verification is recorded where required.
- Implemented:
  - Container records, relationships, group maintenance, and UI listing were implemented.
- Verification:
  - Source project record marks workflow completed.
- Remaining:
  - None.
- Notes:
  - None.

### T005 - Add registration progress and cancellation

- Status: completed
- Objective: Make long-running registration transparent and controllable.
- Scope:
  - Report overall and per-file progress.
  - Support cancellation.
  - Display ETA.
- Acceptance criteria:
  - The objective is implemented without violating project constraints.
  - Relevant configured checks pass, and user verification is recorded where required.
- Implemented:
  - Overall progress, large-file sub-progress, cancellation, and ETA were implemented.
- Verification:
  - Source project record marks workflow completed.
- Remaining:
  - None.
- Notes:
  - None.

### T006 - Generate image and video thumbnails

- Status: completed
- Objective: Produce reliable visual previews for image and video content.
- Scope:
  - Generate image thumbnails.
  - Generate up to 16 interval-sampled video slots.
  - Deliver previews safely to the frontend.
- Acceptance criteria:
  - The objective is implemented without violating project constraints.
  - Relevant configured checks pass, and user verification is recorded where required.
- Implemented:
  - Image thumbnails, video slot sampling, and data URL delivery were implemented.
- Verification:
  - Source project record marks workflow completed.
- Remaining:
  - None.
- Notes:
  - None.

### T007 - Support archive previews and grids

- Status: completed
- Objective: Generate and display preview slots for image archives.
- Scope:
  - Detect 7z or 7za.
  - Extract candidate images and generate up to 16 slots.
  - Render compact and expanded grids.
  - Support archive backfill for existing records.
- Acceptance criteria:
  - The objective is implemented without violating project constraints.
  - Relevant configured checks pass, and user verification is recorded where required.
- Implemented:
  - Archive extraction detection, thumbnail generation, grid rendering, extra comic archive extensions, backfill command, and per-container action were implemented.
- Verification:
  - Executable verification was recorded for archive backfill, status flow, and existing archive data.
- Remaining:
  - None.
- Notes:
  - None.

### T008 - Maintain thumbnail cache consistency

- Status: completed
- Objective: Inspect and repair stale thumbnail records and orphaned cache files.
- Scope:
  - Provide targeted archive backfill.
  - Inspect missing records and orphaned files.
  - Clean identified stale data and refresh the UI.
- Acceptance criteria:
  - The objective is implemented without violating project constraints.
  - Relevant configured checks pass, and user verification is recorded where required.
- Implemented:
  - Inspection, cleanup, targeted backfill, and UI summaries were implemented.
- Verification:
  - Normal-data verification reported no stale data.
  - Injected stale-data verification reported and removed missing-record and orphaned-file cases.
- Remaining:
  - None.
- Notes:
  - None.

### T009 - Run thumbnail work as background jobs

- Status: completed
- Objective: Separate heavy thumbnail processing from interactive registration.
- Scope:
  - Queue and serialize thumbnail jobs.
  - Report task progress and persistent last-error information.
  - Refresh previews after jobs finish.
  - Support deterministic debug failure injection.
- Acceptance criteria:
  - The objective is implemented without violating project constraints.
  - Relevant configured checks pass, and user verification is recorded where required.
- Implemented:
  - Background queueing, progress events, frontend summaries, group-thumbnail rebuild, preview refresh, failure injection, and final error preservation were implemented.
- Verification:
  - Success-path executable verification was recorded.
  - Deterministic failure-path executable verification was recorded.
- Remaining:
  - None.
- Notes:
  - None.

### T010 - Implement safe duplicate management

- Status: pending
- Objective: Allow users to review and explicitly act on duplicate candidates without automatic deletion.
- Scope:
  - List candidates by hash and metadata.
  - Provide side-by-side preview and metadata comparison.
  - Require explicit confirmation before move or delete.
  - Prefer Recycle Bin or an equivalent safe target initially.
- Acceptance criteria:
  - The objective is implemented without violating project constraints.
  - Relevant configured checks pass, and user verification is recorded where required.
- Implemented:
  - None.
- Verification:
  - Not run.
- Remaining:
  - All implementation and verification remain.
- Notes:
  - None.

### T011 - Complete combined-container UX and search semantics

- Status: in progress
- Objective: Make combined containers first-class creation, browsing, and search entities.
- Scope:
  - Search containers directly.
  - Create and edit combined containers.
  - Control whether children of combined containers appear in search.
  - Improve previews and parent-child navigation.
- Acceptance criteria:
  - The objective is implemented without violating project constraints.
  - Relevant configured checks pass, and user verification is recorded where required.
- Implemented:
  - Container search, include or exclude toggle, result listing, quick-open behavior, and conditional auto-expansion were implemented.
- Verification:
  - Implemented search behavior is recorded in source project history.
- Remaining:
  - Combined-container creation and editing, child visibility semantics, result thumbnails, and navigation improvements remain.
- Notes:
  - Awaiting user verification for UI and workflow semantics after additional implementation.

### T012 - Define nested archive traversal policy

- Status: in progress
- Objective: Handle nested archives and mixed structures predictably and reproducibly.
- Scope:
  - Add configurable traversal depth with a safe default.
  - Persist traversal decisions.
  - Improve corrupt and unsupported archive diagnostics.
  - Keep behavior deterministic across archive-processing paths.
- Acceptance criteria:
  - The objective is implemented without violating project constraints.
  - Relevant configured checks pass, and user verification is recorded where required.
- Implemented:
  - Fixed nested-archive expansion safety limit in registration-time virtual hierarchy construction.
  - Deterministic skip behavior for nested extraction failures was implemented.
- Verification:
  - Source project record marks partial implementation completed.
- Remaining:
  - Configurable depth, persisted decisions, broader diagnostics, and deterministic parity across all paths remain.
- Notes:
  - Current depth limit is backend-fixed and not user-facing.

### T013 - Evolve classification schemas safely

- Status: pending
- Objective: Allow user-defined rating dimensions and scalable tag management without unsafe schema rewrites.
- Scope:
  - Support multiple rating dimensions.
  - Add tag suggestions, normalization, and bulk operations.
  - Define migration-safe classification versioning.
- Acceptance criteria:
  - The objective is implemented without violating project constraints.
  - Relevant configured checks pass, and user verification is recorded where required.
- Implemented:
  - None.
- Verification:
  - Not run.
- Remaining:
  - All implementation and verification remain.
- Notes:
  - None.

### T014 - Harden performance at scale

- Status: pending
- Objective: Keep registration, browsing, and search responsive for large media libraries.
- Scope:
  - Review indexes and tune common queries.
  - Optimize incremental scans and consider watch-based updates.
  - Define baselines and profiling checkpoints.
- Acceptance criteria:
  - The objective is implemented without violating project constraints.
  - Relevant configured checks pass, and user verification is recorded where required.
- Implemented:
  - None.
- Verification:
  - Not run.
- Remaining:
  - All implementation and verification remain.
- Notes:
  - None.

### T015 - Improve testing and release reliability

- Status: pending
- Objective: Establish regression coverage and a repeatable Windows release process.
- Scope:
  - Add backend tests for registration, dedupe candidates, and thumbnail paths.
  - Add frontend smoke or regression tests.
  - Create a Windows packaging, upgrade, and release checklist.
- Acceptance criteria:
  - The objective is implemented without violating project constraints.
  - Relevant configured checks pass, and user verification is recorded where required.
- Implemented:
  - None.
- Verification:
  - Not run.
- Remaining:
  - All implementation and verification remain.
- Notes:
  - None.

### T016 - Explain archive backfill failures in the UI

- Status: completed
- Objective: Return actionable reasons when archive previews cannot be generated.
- Scope:
  - Distinguish missing extractor, extraction failure, no images, and generation failure.
  - Surface skipped reasons without changing successful messaging.
- Acceptance criteria:
  - The objective is implemented without violating project constraints.
  - Relevant configured checks pass, and user verification is recorded where required.
- Implemented:
  - Backend reason reporting and UI skip messaging were implemented.
- Verification:
  - Executable verification confirmed no-preview reason messaging and preserved success messaging.
- Remaining:
  - None.
- Notes:
  - None.

### T017 - Improve archive image candidate detection

- Status: completed
- Objective: Reduce false no-image results for nested files and extension variants.
- Scope:
  - Report extracted file counts and sampled extensions.
  - Recognize additional normalized image extensions.
  - Add regression tests for nested paths and normalization.
- Acceptance criteria:
  - The objective is implemented without violating project constraints.
  - Relevant configured checks pass, and user verification is recorded where required.
- Implemented:
  - Diagnostics, extension support, and backend regression tests were implemented.
- Verification:
  - Executable verification generated 16 preview slots for a previously failing source path.
- Remaining:
  - None.
- Notes:
  - Container ID may vary across runs; behavior is verified by source path outcome.

### T018 - Add creation datetime metadata and search

- Status: pending
- Objective: Persist and query creation datetimes for files and containers.
- Scope:
  - Persist file creation datetime.
  - Define image-folder container datetime.
  - Support after, before, and between criteria.
  - Define timezone and source-of-truth policy.
- Acceptance criteria:
  - The objective is implemented without violating project constraints.
  - Relevant configured checks pass, and user verification is recorded where required.
- Implemented:
  - None.
- Verification:
  - Not run.
- Remaining:
  - All implementation, policy decisions, and verification remain.
- Notes:
  - None.

### T019 - Preview videos contained in archives

- Status: pending
- Objective: Generate meaningful previews when extracted archive contents include videos.
- Scope:
  - Detect extracted videos.
  - Sample representative video frames.
  - Define image-versus-video fallback priority.
  - Coordinate traversal depth with T012.
- Acceptance criteria:
  - The objective is implemented without violating project constraints.
  - Relevant configured checks pass, and user verification is recorded where required.
- Implemented:
  - None.
- Verification:
  - Not run.
- Remaining:
  - All implementation and verification remain.
- Notes:
  - None.

### T020 - Create archive-derived virtual container hierarchies

- Status: completed
- Objective: Persist searchable virtual containers derived from archive paths and media types.
- Scope:
  - Build path and media-type virtual containers during registration.
  - Persist parent-child hierarchy.
  - Apply recursive single-child compression.
  - Expand nested archives to a safe maximum depth.
  - Build virtual-container thumbnail previews.
  - Integrate virtual containers into search with inclusion control.
- Acceptance criteria:
  - The objective is implemented without violating project constraints.
  - Relevant configured checks pass, and user verification is recorded where required.
- Implemented:
  - DB metadata persistence for archive virtual containers was implemented.
  - Registration-time hierarchy construction with media buckets and deterministic replacement on re-registration was implemented.
  - Nested archive expansion with deterministic per-item failure skip was implemented.
  - Schema compatibility handling for archive_virtual_container_meta during initialization was implemented.
  - Virtual-container thumbnail assembly and aggregation were implemented.
  - Search integration with include or exclude control was implemented.
  - Recent Containers top-level exclusion for archive_virtual with child sorting rules was implemented.
- Verification:
  - Live dataset executable verification confirmed searchable hierarchy paths, include or exclude behavior, and nested archive subtree construction.
- Remaining:
  - None for this task scope.
- Notes:
  - Explicit per-archive rebuild, edit, merge, or split controls remain out of scope for this task.

### T021 - Clean up temporary schema migrations

- Status: pending
- Objective: Remove development-stage compatibility migrations once schema stabilizes for release.
- Scope:
  - Identify migration paths that only address active-development DB drift.
  - Define cutover criteria for removing temporary migrations.
  - Simplify init_database schema setup after cutover.
  - Verify clean initialization and registration on fresh DB.
- Acceptance criteria:
  - The objective is implemented without violating project constraints.
  - Relevant configured checks pass, and user verification is recorded where required.
- Implemented:
  - None.
- Verification:
  - Not run.
- Remaining:
  - All implementation and verification remain.
- Notes:
  - None.

### T022 - Migrate project tracking docs to AGENTS and TASKS

- Status: completed
- Objective: Align repository tracking files with the agentic-coding-templates AGENTS and TASKS structure.
- Scope:
  - Replace legacy AGENTS operating notes with template-aligned repository instructions.
  - Move project objective and task ledger authority from project.md to TASKS.md.
  - Keep existing task states and evidence preserved under stable task IDs.
- Acceptance criteria:
  - AGENTS.md and TASKS.md are present and template-aligned for this repository.
  - Existing mainline task statuses are preserved.
  - Legacy project.md no longer acts as the active task ledger.
- Implemented:
  - AGENTS.md was rewritten to a template-aligned, repository-specific operating guide.
  - TASKS.md was created with migrated objective, constraints, active-task metadata, full task ledger, and decisions.
  - project.md was converted to a migration note that points to TASKS.md.
  - AGENTS.md was extended with Project learnings and User handoff and action requests sections.
  - README.md was updated from template boilerplate to repository-specific guidance aligned with AGENTS.md and TASKS.md.
- Verification:
  - Documentation consistency review completed across AGENTS.md, TASKS.md, and project.md.
  - Documentation consistency review completed across AGENTS.md, TASKS.md, project.md, and README.md after the follow-up update.
- Remaining:
  - None.
- Notes:
  - This is a documentation-process migration task.

## Decisions

### D001 - Require user-driven duplicate removal

- Date: 2026-08-02
- Status: accepted
- Decision: Duplicate detection may propose candidates, but the application must not automatically remove them.
- Rationale: Hash or metadata matches can be ambiguous, and removal is destructive.
- Consequences:
  - T010 must provide comparison and explicit confirmation.
  - Initial removal should use Recycle Bin or a similarly recoverable mechanism.
- Related tasks: T010

### D002 - Use background jobs for heavy thumbnail work

- Date: 2026-08-02
- Status: accepted
- Decision: Complete registration after metadata persistence and run heavy thumbnail work through serialized background jobs with progress and error reporting.
- Rationale: Interactive registration must remain responsive while expensive media processing continues.
- Consequences:
  - Frontend state must distinguish registration completion from thumbnail-job completion.
  - Final job events must preserve meaningful failure information.
- Related tasks: T005, T006, T007, T008, T009

### D003 - Compress single-child virtual archive paths

- Date: 2026-08-02
- Status: accepted
- Decision: Apply recursive single-child compression unconditionally to archive-derived virtual container hierarchies.
- Rationale: Linear path-only levels add navigation depth without meaningful grouping value.
- Consequences:
  - The behavior is not configurable in initial T020 implementation.
  - Display identity must still retain the full relative archive path.
- Related tasks: T012, T020

### D004 - Require approval for dependency and capability changes

- Date: 2026-08-02
- Status: accepted
- Decision: Prior user approval is required for npm packages, Rust crates, Tauri plugins, dependency upgrades, feature changes, capabilities, and permissions.
- Rationale: These changes affect maintenance, binary footprint, security scope, and reproducibility.
- Consequences:
  - Agents may inspect and propose changes but may not mutate manifests or lockfiles before approval.
- Related tasks: all future tasks

### D005 - TASKS.md is the authoritative task ledger

- Date: 2026-09-01
- Status: accepted
- Decision: Replace project.md task tracking with TASKS.md as the canonical objective and task-state record.
- Rationale: Align repository workflow with the AGENTS and TASKS template model and avoid split sources of truth.
- Consequences:
  - Agents must update TASKS.md in the same work cycle as implementation changes.
  - project.md remains a compatibility pointer and must not be used for active task updates.
- Related tasks: T022

## Blocked items

- None.

## Cancelled items

- None.

## Backlog

- Additional supported media formats may be proposed when concrete use cases and extractor or decoder behavior are known.
