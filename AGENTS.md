# AGENTS.md

Operating instructions for coding agents working in ThumbsContainer. Read this file and TASKS.md before changing the repository.

## 1. Non-negotiable rules

1. Do not fabricate repository structure, Tauri APIs, database behavior, file-format support, command results, or verification claims.
2. Make the smallest task-related change and preserve unrelated user work.
3. Do not add, remove, replace, or upgrade npm packages, Rust crates, Tauri plugins, capabilities, or permissions without prior user approval.
4. Do not commit unless the user explicitly requests it for the current work.
5. Update TASKS.md with every meaningful implementation step in the same work cycle.
6. Never automatically delete suspected duplicate media. Destructive actions require an explicit user-driven confirmation flow.
7. Treat external tools and Windows integration as unverified unless exercised in an appropriate environment.

## 2. Project context

- Project: ThumbsContainer
- Objective: Organize image, video, and archive media containers; maintain metadata, thumbnails, classifications, grouping, search, and user-controlled duplicate management.
- Primary languages: Rust and TypeScript
- Frameworks: Tauri v2, SvelteKit, Svelte
- Storage: SQLite and local thumbnail and cache files
- Runtime and deployment target: Windows desktop
- External runtime requirements: ffmpeg.exe and ffprobe.exe are available through PATH; archive extraction uses an available 7z or 7za executable.

## 3. Repository orientation

Before editing:

1. Read TASKS.md and identify the affected stable task.
2. Inspect both frontend and backend paths for commands, events, data structures, and UI state involved in the task.
3. Inspect package.json, package-lock.json, Cargo manifests, Cargo.lock, Tauri configuration, capabilities, and database setup where relevant.
4. Check the working tree and preserve unrelated user changes.
5. State a brief plan with executable and user-verification criteria.
6. Inspect existing error, job, cache, and database patterns before introducing new ones.

Repository layout and no-edit zones:

- Source: src, src-tauri/src
- Configuration: package.json, tsconfig.json, svelte.config.js, vite.config.js, src-tauri/Cargo.toml, src-tauri/tauri.conf.json, src-tauri/capabilities/default.json
- Generated or vendored paths not to edit manually: build, .svelte-kit, node_modules, src-tauri/target

## 4. Product constraints

- Media files are represented through container concepts, including videos, archives, image folders, virtual groups, combined containers, and archive-derived virtual containers.
- Duplicate detection is advisory. Deletion or movement must remain an explicit user decision.
- Registration and thumbnail work must not freeze the interactive UI.
- Existing metadata should survive file moves when identity can be established safely.
- Preserve database compatibility unless a task explicitly authorizes a migration or rebuild strategy.
- Error reporting for external tools, corrupt media, unsupported formats, and background jobs must remain visible and actionable.
- Keep thumbnail and cache records and files consistent; cleanup must target only identified stale or orphaned data.

## 5. Dependency and permission policy

Prefer the standard libraries and dependencies already present. Prior user approval is required for dependency, feature, plugin, capability, or permission changes.

Before requesting approval, explain:

1. the need,
2. alternatives,
3. maintenance and binary impact,
4. security and capability scope,
5. exact manifests or lockfiles affected.

Synchronizing declared dependencies is allowed. After an approved change, update the applicable manifest and lockfile together and run both frontend and Rust checks.

## 6. Tauri, Svelte, TypeScript, and Rust

- Treat each Tauri interaction as a synchronized contract between Rust and TypeScript.
- Keep command names, argument casing, serialization formats, events, error payloads, and frontend listeners consistent.
- Grant the narrowest Tauri permissions and capabilities required by the task.
- Keep blocking filesystem, database, hashing, extraction, FFmpeg, and thumbnail operations off the UI thread and avoid blocking asynchronous runtime workers.
- Infer the frontend package manager from the existing lockfile; do not create another lockfile.
- Use scripts from package.json for checks and builds.
- Use the repository Rust toolchain and workspace configuration.
- Preserve existing Svelte component, routing, state, styling, and accessibility patterns.
- Do not claim cross-platform support from a Windows-only or host-only check.

## 7. Verification and completion

Run all relevant configured checks. A typical final pass may include frontend check and build scripts plus Rust check and test commands, but inspect the repository rather than assuming exact commands.

For executable milestones:

1. Produce a buildable application or focused executable path.
2. Run available automated checks.
3. Record the produced build and checks in TASKS.md.
4. Keep UI, archive-tool, FFmpeg, Windows integration, performance, and subjective behavior in progress with Awaiting user verification until the user confirms the stated procedure, unless the agent environment can verify it adequately.

Do not infer UI correctness from compilation alone.

## 8. Task management

For every meaningful implementation step:

1. Update the affected task in TASKS.md during the same work cycle.
2. Record implemented behavior, checks, manual evidence, remaining work, and risks factually.
3. Append newly discovered mainline work with the next unused stable ID.
4. Preserve IDs and do not renumber or substantially reorganize tasks without explicit approval.
5. Use exactly one status: pending, in progress, blocked, completed, or cancelled.
6. Add a short decision record when a durable architectural or product choice would otherwise be reopened.
7. Review task status and active-task metadata before the final response.

## 9. Git safety

- Do not commit by default.
- Do not stage, discard, or overwrite unrelated changes.
- When a commit is explicitly requested, include the related TASKS.md update and follow the repository established message style.
- Pushing, tagging, packaging a release, or publishing artifacts requires an explicit request.

## 10. Commands

- Install or synchronize: npm install and cargo fetch
- Run locally: npm run tauri dev
- Build frontend: npm run build
- Build desktop app: npm run tauri build
- Frontend checks: npm run check
- Rust checks: cargo check (run in src-tauri)
- Rust tests: cargo test (run in src-tauri)
- Lint: N/A
- Format check: N/A

## 11. Project learnings

Maintain short, concrete rules learned from corrections or repository-specific failures. Tighten an existing rule instead of duplicating it. Remove obsolete rules.

- None yet.

## 12. User handoff and action requests

Make the required next action explicit whenever work is not fully complete.

Use exactly one of these outcomes:

- No user action required: the work is complete, or the agent can continue autonomously.
- Permission required: state the exact action requiring approval and why.
- Decision required: ask a specific question, provide relevant options and trade-offs, and identify the recommended option when appropriate.
- User verification required: provide concrete verification steps and describe what result to report.
- Blocked: state the blocking condition and what would unblock the work.

Do not present an explanation, possible future step, or rhetorical question in a way that could be mistaken for a request. When requesting user action, end the response with a clearly labelled User action required section. Distinguish the agent next action from the user next action.

## 13. Final response checklist

- List changed files.
- Report frontend and backend checks actually run.
- State task and decision changes in TASKS.md.
- Identify behavior awaiting Windows, UI, external-tool, data-set, performance, or user verification.
- Mention database migration, cache consistency, and destructive-operation risks when applicable.
