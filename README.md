# ThumbsContainer

ThumbsContainer is a Windows desktop app for organizing media files as container-based records.
The application targets images, videos, archives, and archive-derived virtual hierarchies,
with search, classification, thumbnailing, and user-driven duplicate management.

## Stack

- Frontend: SvelteKit + TypeScript
- Backend: Rust + Tauri v2
- Storage: SQLite + local thumbnail/cache files
- External tools: ffmpeg.exe and ffprobe.exe in PATH, plus 7z or 7za for archive extraction

## Project tracking files

- AGENTS.md: repository-specific operating rules for coding agents.
- TASKS.md: authoritative objective, task status, verification, and decisions.
- project.md: deprecated pointer kept for migration compatibility.

## Development workflow

1. Read AGENTS.md and TASKS.md before making changes.
2. Implement the smallest change that satisfies the active task.
3. Run relevant checks.
4. Update TASKS.md in the same work cycle with implemented behavior and verification evidence.

## Commands

- Install frontend dependencies: npm install
- Run development app: npm run tauri dev
- Frontend checks: npm run check
- Frontend build: npm run build
- Desktop build: npm run tauri build
- Rust checks: cargo check (run in src-tauri)
- Rust tests: cargo test (run in src-tauri)

## Notes

- Duplicate handling is advisory; destructive actions must remain explicit user decisions.
- UI and Windows integration behavior should be user-verified for milestone completion when not fully testable in agent environments.
