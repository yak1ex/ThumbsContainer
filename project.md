## Objectives

### Main objectives

* We want to develop a tool to organize media files, such as images and videos, including archive files of them.
  * Typical use cases are as follows: First, register media files under the specified folders in database. Then, users add information for the files. Users search media files by the information.
  * Provide a feature to help dedupe of the same files. This MUST not automatically done. Decision of removal should be made by user.

### Details

#### Target files

* Target media files are treated as containers of image files.
  * A video is a time series container of images.
  * An archive of image files is a container of the containing image files.
  * Images files need special handling. If a folder contains image files only and does not have sub folders, the folder is considered as a container. Otherwise, image files in a filer is grouped and the group is consiered as a virutal container.
* Containers can be combined as a new container. In search function, described at later, provide an option to hide containers in combined containers.
* Each container is related to corresponding thumbnails and user-defined classifications.
  * User-defined classifications can be added later by users themselves.
    * User-defined classifications mainly consist from 2 parts. One is textual description, that is tag, and another is rating for various perspecitves.
* The following file types should be supported. It is not limited to them. Try file type implied by its extension, auto-detect if failed.
  * Video files: mp4, mkv, avi
  * Image files: jpg, png, gif, webp, avif
  * Archive files: rar, 7z, zip, lzh

#### Register files

* There are automatic registering and manual regisetring.
  * Automatic registering is triggered by users, then it collects hash, creation date and time, file name, path and file size.
    * Probably hash should be a part of primary key. Why hash itself is not a primary key is preparation for hash collision.
      * The tool can detect file moves. Corresponding information, except for path-specific information, should be maintained.
  * At manual registering, users add and edit tags and ratings during seeing existing information such as thumbnails.
    * All tags and ratings are optional and may be added at any time.

#### Search

* Users can search media files by combination of path, filename, rating, and tags.

## Development

* Develop step-by-step. At each step, an executable should be build, then ask me to check the executable.
* Use Rust + Typescript, Tauri and Svelte for frameworks.
* Use SQLite for database.
* You can assume FFmpeg executables `ffmpeg.exe` and `ffprobe.exe` are available in executable search PATH.

## Tasks

This section summarizes the current development state and the mainline implementation steps.

### 1. Foundation and project scaffold

* Goal: establish a buildable desktop app baseline.
* Implemented:
  * Rust + Tauri v2 backend, SvelteKit + TypeScript frontend.
  * SQLite initialization and local app data storage.
  * Basic UI wiring to backend commands.
* Current state: completed and stable.

### 2. Registration pipeline and persistence

* Goal: register media files from folders and persist core metadata.
* Implemented:
  * Folder scan and file registration to DB.
  * Metadata capture: hash, path, filename, size, timestamps.
  * Move detection by hash to update path while preserving existing metadata.
* Current state: completed.
* Notes:
  * Registration runs in blocking worker context to keep UI responsive.

### 3. Search and classification workflow

* Goal: allow users to find and classify files.
* Implemented:
  * Search by path, filename, tags, and minimum rating.
  * Classification editing (tags and rating).
* Current state: completed.

### 4. Container model and grouping

* Goal: represent videos/images/archives as containers and support grouping.
* Implemented:
  * Container records and parent/child relationships.
  * Group container creation and maintenance logic.
  * Recent containers and expandable child listing in UI.
* Current state: completed.

### 5. Registration progress, sub-progress, cancel, ETA

* Goal: make long-running registration transparent and controllable.
* Implemented:
  * Overall progress events.
  * Per-file sub-progress for large files.
  * Cancel support and ETA display.
* Current state: completed.

### 6. Thumbnail generation for image/video

* Goal: produce reliable visual previews for files and containers.
* Implemented:
  * Image thumbnails.
  * Video multi-slot thumbnails (up to 16 slots).
  * Data URL delivery from backend to avoid frontend file loading issues.
* Current state: completed.

### 7. Archive thumbnail support and grid display

* Goal: support archive previews similarly to video containers.
* Implemented:
  * Archive extraction detection using `7z`/`7za`.
  * Archive thumbnail slot generation (up to 16) from extracted images.
  * Container preview grid rendering in UI (compact row preview + expanded view).
  * Additional archive extensions recognized: cbz, cbr, cb7.
* Current state: implemented and compilable.
* Remaining verification:
  * Re-register existing archive entries to backfill multi-slot thumbnails.

### 8. Next mainline tasks

* Goal: harden and operationalize thumbnail pipeline at scale.
* Planned:
  * Add targeted backfill command for archive thumbnails without full re-scan.
  * Run thumbnail generation in a dedicated background queue.
  * Add thumbnail cache maintenance commands (cleanup/orphan removal).
  * Add tests for archive slot generation and container thumbnail query behavior.
* Current state: pending.
