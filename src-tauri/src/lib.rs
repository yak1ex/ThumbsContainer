use rusqlite::{params, Connection};
use serde::Serialize;
use sha2::{Digest, Sha256};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager};
use walkdir::WalkDir;

const REGISTER_CANCELLED: &str = "register_cancelled";
const ARCHIVE_VIRTUAL_DEFAULT_NESTED_DEPTH: usize = 2;
const ARCHIVE_VIRTUAL_MAX_NESTED_DEPTH: usize = 10;

#[derive(Serialize)]
struct RegisterResult {
    scanned_files: usize,
    inserted_files: usize,
    updated_paths: usize,
    skipped_files: usize,
    created_containers: usize,
    updated_containers: usize,
    created_group_containers: usize,
    updated_group_containers: usize,
    created_thumbnails: usize,
    updated_thumbnails: usize,
    queued_thumbnail_tasks: usize,
    background_job_id: Option<u64>,
    canceled: bool,
}

#[derive(Serialize)]
struct FileRecord {
    id: i64,
    path: String,
    filename: String,
    hash: String,
    size: i64,
    created_at: String,
    thumbnail_path: Option<String>,
    thumbnail_data_url: Option<String>,
}

#[derive(Serialize)]
struct FileClassification {
    tags: Vec<String>,
    rating: Option<i64>,
}

#[derive(Serialize)]
struct DuplicateFileRecord {
    id: i64,
    path: String,
    filename: String,
    hash: String,
    size: i64,
    created_at: String,
    thumbnail_path: Option<String>,
    thumbnail_data_url: Option<String>,
    tags: Vec<String>,
    rating: Option<i64>,
}

#[derive(Serialize)]
struct DuplicateGroupRecord {
    hash: String,
    size: i64,
    file_count: i64,
    files: Vec<DuplicateFileRecord>,
}

#[derive(Serialize)]
struct DuplicateActionResult {
    file_id: i64,
    previous_path: String,
    current_path: String,
    action: String,
}

#[derive(Serialize)]
struct QuarantinedDuplicateRecord {
    file_id: i64,
    filename: String,
    hash: String,
    size: i64,
    original_path: String,
    quarantine_path: String,
    quarantined_at: String,
    thumbnail_data_url: Option<String>,
}

#[derive(Serialize)]
struct BulkPurgeResult {
    requested: usize,
    purged: usize,
    failed: usize,
    failures: Vec<String>,
}

#[derive(Serialize)]
struct DuplicateOverview {
    total_files: i64,
    duplicate_groups: i64,
    duplicate_files: i64,
    quarantined_files: i64,
}

#[derive(Serialize)]
struct ContainerRecord {
    id: i64,
    container_type: String,
    display_name: String,
    source_path: String,
    updated_at: String,
    child_count: i64,
    include_children_in_search: Option<bool>,
}

#[derive(Serialize)]
struct ContainerChildRecord {
    id: i64,
    container_type: String,
    display_name: String,
    source_path: String,
    updated_at: String,
    child_count: i64,
    include_children_in_search: Option<bool>,
}

#[derive(Serialize)]
struct CombinedContainerDetail {
    container_id: i64,
    display_name: String,
    include_children_in_search: bool,
    child_container_ids: Vec<i64>,
}

#[derive(Serialize)]
struct ContainerThumbnailRecord {
    slot_index: i64,
    thumbnail_path: String,
    thumbnail_data_url: Option<String>,
}

#[derive(Serialize)]
struct ArchiveBackfillResult {
    container_id: i64,
    generated_slots: usize,
    updated_slots: usize,
    updated_file_thumbnail: bool,
    skipped_reason: Option<String>,
}

#[derive(Serialize)]
struct ThumbnailMaintenanceResult {
    dry_run: bool,
    referenced_thumbnail_files: usize,
    missing_file_thumbnail_records: usize,
    missing_container_thumbnail_records: usize,
    orphaned_cache_files: usize,
    removed_file_thumbnail_records: usize,
    removed_container_thumbnail_records: usize,
    removed_orphaned_cache_files: usize,
}

struct ContainerUpsertOutcome {
    created: bool,
    container_id: i64,
}

struct GroupRebuildResult {
    created: usize,
    updated: usize,
}

#[derive(Clone)]
struct ArchiveVirtualMediaEntry {
    relative_path: String,
    media_kind: &'static str,
    source_path: PathBuf,
}

#[derive(Default)]
struct ArchiveVirtualTreeNode {
    name: String,
    children: Vec<ArchiveVirtualTreeNode>,
    has_image_files: bool,
    has_video_files: bool,
}

impl ArchiveVirtualTreeNode {
    fn new(name: String) -> Self {
        Self {
            name,
            children: Vec::new(),
            has_image_files: false,
            has_video_files: false,
        }
    }
}

struct ArchiveVirtualPersistNode {
    virtual_path: String,
    parent_virtual_path: Option<String>,
    node_kind: &'static str,
    media_kind: Option<&'static str>,
    depth: i64,
}

#[derive(Serialize, Clone)]
struct RegisterProgress {
    folder_path: String,
    total_files: usize,
    scanned_files: usize,
    inserted_files: usize,
    updated_paths: usize,
    skipped_files: usize,
    current_file_path: Option<String>,
    current_file_size_bytes: u64,
    current_file_processed_bytes: u64,
    canceled: bool,
    done: bool,
}

#[derive(Default)]
struct RegisterControl {
    cancel_flag: Mutex<Option<Arc<AtomicBool>>>,
}

fn open_db(app: &tauri::AppHandle) -> Result<Connection, String> {
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("failed to resolve app data dir: {e}"))?;

    std::fs::create_dir_all(&app_data)
        .map_err(|e| format!("failed to create app data dir {:?}: {e}", app_data))?;

    let db_path = app_data.join("thumbscontainer.db");
    let conn = Connection::open(db_path).map_err(|e| format!("failed to open db: {e}"))?;
    conn.execute("PRAGMA foreign_keys = ON", [])
        .map_err(|e| format!("failed to enable foreign keys: {e}"))?;
    Ok(conn)
}

#[tauri::command]
fn init_database(app: tauri::AppHandle) -> Result<String, String> {
    let conn = open_db(&app)?;
    conn.execute("PRAGMA foreign_keys = ON", [])
        .map_err(|e| format!("failed to enable foreign keys: {e}"))?;

    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS files (
          id INTEGER PRIMARY KEY,
          hash TEXT NOT NULL,
          size INTEGER NOT NULL,
          path TEXT NOT NULL UNIQUE,
          filename TEXT NOT NULL,
          created_at TEXT NOT NULL DEFAULT (datetime('now')),
          updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_files_hash_size ON files(hash, size);

                CREATE TABLE IF NOT EXISTS duplicate_quarantine_history (
                    id INTEGER PRIMARY KEY,
                    file_id INTEGER NOT NULL,
                    original_path TEXT NOT NULL,
                    quarantine_path TEXT NOT NULL,
                    quarantined_at TEXT NOT NULL DEFAULT (datetime('now')),
                    restored_at TEXT,
                    purged_at TEXT
                );

                CREATE INDEX IF NOT EXISTS idx_duplicate_quarantine_history_file_id
                    ON duplicate_quarantine_history(file_id);

                CREATE TABLE IF NOT EXISTS containers (
                    id INTEGER PRIMARY KEY,
                    container_type TEXT NOT NULL,
                    display_name TEXT NOT NULL,
                    source_path TEXT NOT NULL,
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
                );

                CREATE TABLE IF NOT EXISTS file_containers (
                    file_id INTEGER PRIMARY KEY,
                    container_id INTEGER NOT NULL,
                    FOREIGN KEY (file_id) REFERENCES files(id) ON DELETE CASCADE,
                    FOREIGN KEY (container_id) REFERENCES containers(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_containers_updated_at ON containers(updated_at);
                CREATE UNIQUE INDEX IF NOT EXISTS idx_containers_type_source
                    ON containers(container_type, source_path);
                CREATE INDEX IF NOT EXISTS idx_file_containers_container_id ON file_containers(container_id);

                CREATE TABLE IF NOT EXISTS container_children (
                    parent_container_id INTEGER NOT NULL,
                    child_container_id INTEGER NOT NULL,
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    PRIMARY KEY (parent_container_id, child_container_id),
                    FOREIGN KEY (parent_container_id) REFERENCES containers(id) ON DELETE CASCADE,
                    FOREIGN KEY (child_container_id) REFERENCES containers(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_container_children_parent ON container_children(parent_container_id);
                CREATE INDEX IF NOT EXISTS idx_container_children_child ON container_children(child_container_id);

                CREATE TABLE IF NOT EXISTS tags (
                    id INTEGER PRIMARY KEY,
                    name TEXT NOT NULL UNIQUE,
                    created_at TEXT NOT NULL DEFAULT (datetime('now'))
                );

                CREATE TABLE IF NOT EXISTS file_tags (
                    file_id INTEGER NOT NULL,
                    tag_id INTEGER NOT NULL,
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    PRIMARY KEY (file_id, tag_id),
                    FOREIGN KEY (file_id) REFERENCES files(id) ON DELETE CASCADE,
                    FOREIGN KEY (tag_id) REFERENCES tags(id) ON DELETE CASCADE
                );

                CREATE TABLE IF NOT EXISTS file_ratings (
                    file_id INTEGER PRIMARY KEY,
                    rating INTEGER NOT NULL CHECK (rating >= 1 AND rating <= 5),
                    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                    FOREIGN KEY (file_id) REFERENCES files(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_file_tags_file_id ON file_tags(file_id);
                CREATE INDEX IF NOT EXISTS idx_file_tags_tag_id ON file_tags(tag_id);
                CREATE INDEX IF NOT EXISTS idx_file_ratings_rating ON file_ratings(rating);

                CREATE TABLE IF NOT EXISTS thumbnails (
                    file_id INTEGER PRIMARY KEY,
                    thumbnail_path TEXT NOT NULL,
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                    FOREIGN KEY (file_id) REFERENCES files(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_thumbnails_updated_at ON thumbnails(updated_at);

                CREATE TABLE IF NOT EXISTS container_thumbnails (
                    container_id INTEGER NOT NULL,
                    slot_index INTEGER NOT NULL,
                    thumbnail_path TEXT NOT NULL,
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                    PRIMARY KEY (container_id, slot_index),
                    FOREIGN KEY (container_id) REFERENCES containers(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_container_thumbnails_container_id
                    ON container_thumbnails(container_id);
        ",
    )
    .map_err(|e| format!("failed to create schema: {e}"))?;

    ensure_app_settings_schema(&conn)?;
    ensure_archive_virtual_container_meta_schema(&conn)?;
    ensure_combined_container_meta_schema(&conn)?;

    Ok("database ready".to_string())
}

fn ensure_app_settings_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS app_settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_app_settings_updated_at ON app_settings(updated_at);
        ",
    )
    .map_err(|e| format!("failed to create app settings table: {e}"))?;

    Ok(())
}

fn archive_traversal_depth_for_connection(conn: &Connection) -> Result<usize, String> {
    let stored = conn
        .query_row(
            "SELECT value FROM app_settings WHERE key = 'archive_virtual_nested_depth' LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap_or_else(|_| ARCHIVE_VIRTUAL_DEFAULT_NESTED_DEPTH.to_string());

    let parsed = stored
        .trim()
        .parse::<usize>()
        .unwrap_or(ARCHIVE_VIRTUAL_DEFAULT_NESTED_DEPTH);

    Ok(parsed.min(ARCHIVE_VIRTUAL_MAX_NESTED_DEPTH))
}

fn set_archive_traversal_depth_for_connection(
    conn: &Connection,
    requested: usize,
) -> Result<usize, String> {
    let depth = requested.min(ARCHIVE_VIRTUAL_MAX_NESTED_DEPTH).max(0);

    conn.execute(
        "
        INSERT INTO app_settings(key, value, updated_at)
        VALUES ('archive_virtual_nested_depth', ?, datetime('now'))
        ON CONFLICT(key) DO UPDATE SET
            value = excluded.value,
            updated_at = datetime('now')
        ",
        params![depth.to_string()],
    )
    .map_err(|e| format!("failed to persist archive traversal depth: {e}"))?;

    Ok(depth)
}

fn ensure_archive_virtual_container_meta_schema(conn: &Connection) -> Result<(), String> {
    // Check existing columns before CREATE TABLE IF NOT EXISTS so we can detect
    // a legacy table whose PRIMARY KEY column name differs from the current schema.
    let mut pre_stmt = conn
        .prepare("PRAGMA table_info(archive_virtual_container_meta)")
        .map_err(|e| format!("failed to inspect archive virtual metadata table: {e}"))?;
    let pre_rows = pre_stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| format!("failed to read archive virtual metadata columns: {e}"))?;
    let mut pre_columns: HashSet<String> = HashSet::new();
    for row in pre_rows {
        pre_columns.insert(
            row.map_err(|e| format!("failed to map archive virtual metadata column: {e}"))?,
        );
    }

    // Legacy table exists but uses a different primary key — drop and recreate.
    if !pre_columns.is_empty() && !pre_columns.contains("virtual_container_id") {
        conn.execute_batch(
            "DROP TABLE IF EXISTS archive_virtual_container_meta;",
        )
        .map_err(|e| format!("failed to drop legacy archive virtual metadata table: {e}"))?;
        pre_columns.clear();
    }

    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS archive_virtual_container_meta (
            virtual_container_id INTEGER PRIMARY KEY,
            archive_container_id INTEGER NOT NULL,
            parent_virtual_container_id INTEGER,
            virtual_path TEXT NOT NULL,
            node_kind TEXT NOT NULL,
            media_kind TEXT,
            depth INTEGER NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (virtual_container_id) REFERENCES containers(id) ON DELETE CASCADE,
            FOREIGN KEY (archive_container_id) REFERENCES containers(id) ON DELETE CASCADE,
            FOREIGN KEY (parent_virtual_container_id) REFERENCES containers(id) ON DELETE CASCADE
        );
        ",
    )
    .map_err(|e| format!("failed to create archive virtual metadata table: {e}"))?;

    let columns = pre_columns;

    if !columns.contains("archive_container_id") {
        conn.execute(
            "ALTER TABLE archive_virtual_container_meta ADD COLUMN archive_container_id INTEGER",
            [],
        )
        .map_err(|e| {
            format!(
                "failed to add archive_container_id to archive virtual metadata table: {e}"
            )
        })?;
    }
    if !columns.contains("virtual_path") {
        conn.execute(
            "ALTER TABLE archive_virtual_container_meta ADD COLUMN virtual_path TEXT NOT NULL DEFAULT ''",
            [],
        )
        .map_err(|e| {
            format!(
                "failed to add virtual_path to archive virtual metadata table: {e}"
            )
        })?;
    }
    if !columns.contains("parent_virtual_container_id") {
        conn.execute(
            "ALTER TABLE archive_virtual_container_meta ADD COLUMN parent_virtual_container_id INTEGER",
            [],
        )
        .map_err(|e| {
            format!(
                "failed to add parent_virtual_container_id to archive virtual metadata table: {e}"
            )
        })?;
    }
    if !columns.contains("node_kind") {
        conn.execute(
            "ALTER TABLE archive_virtual_container_meta ADD COLUMN node_kind TEXT NOT NULL DEFAULT 'path'",
            [],
        )
        .map_err(|e| format!("failed to add node_kind to archive virtual metadata table: {e}"))?;
    }
    if !columns.contains("media_kind") {
        conn.execute(
            "ALTER TABLE archive_virtual_container_meta ADD COLUMN media_kind TEXT",
            [],
        )
        .map_err(|e| format!("failed to add media_kind to archive virtual metadata table: {e}"))?;
    }
    if !columns.contains("depth") {
        conn.execute(
            "ALTER TABLE archive_virtual_container_meta ADD COLUMN depth INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(|e| format!("failed to add depth to archive virtual metadata table: {e}"))?;
    }
    if !columns.contains("created_at") {
        conn.execute(
            "ALTER TABLE archive_virtual_container_meta ADD COLUMN created_at TEXT NOT NULL DEFAULT (datetime('now'))",
            [],
        )
        .map_err(|e| format!("failed to add created_at to archive virtual metadata table: {e}"))?;
    }
    if !columns.contains("updated_at") {
        conn.execute(
            "ALTER TABLE archive_virtual_container_meta ADD COLUMN updated_at TEXT NOT NULL DEFAULT (datetime('now'))",
            [],
        )
        .map_err(|e| format!("failed to add updated_at to archive virtual metadata table: {e}"))?;
    }

    conn.execute_batch(
        "
        CREATE INDEX IF NOT EXISTS idx_archive_virtual_meta_archive_id
            ON archive_virtual_container_meta(archive_container_id);
        CREATE INDEX IF NOT EXISTS idx_archive_virtual_meta_parent_id
            ON archive_virtual_container_meta(parent_virtual_container_id);
        CREATE INDEX IF NOT EXISTS idx_archive_virtual_meta_virtual_path
            ON archive_virtual_container_meta(virtual_path);
        ",
    )
    .map_err(|e| format!("failed to create archive virtual metadata indexes: {e}"))?;

    Ok(())
}

fn ensure_combined_container_meta_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS combined_container_meta (
            container_id INTEGER PRIMARY KEY,
            include_children_in_search INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (container_id) REFERENCES containers(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_combined_container_meta_include_children
            ON combined_container_meta(include_children_in_search);
        ",
    )
    .map_err(|e| format!("failed to create combined container metadata schema: {e}"))?;

    Ok(())
}

fn detect_container_type(filename: &str) -> String {
    let lower = filename.to_lowercase();
    let extension = lower.rsplit('.').next().unwrap_or_default();

    if is_video_extension(extension) {
        return "video".to_string();
    }
    if is_image_extension(extension) {
        return "image".to_string();
    }
    if is_archive_container_extension(extension) {
        return "archive".to_string();
    }

    "other".to_string()
}

fn detect_thumbnailable_kind(filename: &str) -> Option<&'static str> {
    let lower = filename.to_lowercase();
    let extension = lower.rsplit('.').next().unwrap_or_default();

    if is_video_extension(extension) {
        return Some("video");
    }
    if is_image_extension(extension) {
        return Some("image");
    }
    None
}

fn thumbnail_cache_path(app: &tauri::AppHandle, file_hash: &str, size: i64) -> Result<PathBuf, String> {
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("failed to resolve app data dir: {e}"))?;
    let thumb_dir = app_data.join("thumbnails");
    std::fs::create_dir_all(&thumb_dir)
        .map_err(|e| format!("failed to create thumbnail dir {:?}: {e}", thumb_dir))?;

    Ok(thumb_dir.join(format!("{file_hash}_{size}.png")))
}

fn thumbnail_cache_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("failed to resolve app data dir: {e}"))?;
    let thumb_dir = app_data.join("thumbnails");
    std::fs::create_dir_all(&thumb_dir)
        .map_err(|e| format!("failed to create thumbnail dir {:?}: {e}", thumb_dir))?;
    Ok(thumb_dir)
}

fn thumbnail_cache_path_for_slot(
    app: &tauri::AppHandle,
    file_hash: &str,
    size: i64,
    slot_index: i64,
) -> Result<PathBuf, String> {
    let thumb_dir = thumbnail_cache_dir(app)?;
    Ok(thumb_dir.join(format!("{file_hash}_{size}_s{slot_index}.png")))
}

fn collect_referenced_thumbnail_paths(conn: &Connection) -> Result<HashSet<PathBuf>, String> {
    let mut paths = HashSet::new();

    let mut file_stmt = conn
        .prepare("SELECT thumbnail_path FROM thumbnails")
        .map_err(|e| format!("failed to prepare file thumbnail query: {e}"))?;
    let file_rows = file_stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| format!("failed to query file thumbnails: {e}"))?;
    for row in file_rows {
        paths.insert(PathBuf::from(
            row.map_err(|e| format!("failed to map file thumbnail row: {e}"))?,
        ));
    }

    let mut container_stmt = conn
        .prepare("SELECT thumbnail_path FROM container_thumbnails")
        .map_err(|e| format!("failed to prepare container thumbnail query: {e}"))?;
    let container_rows = container_stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| format!("failed to query container thumbnails: {e}"))?;
    for row in container_rows {
        paths.insert(PathBuf::from(
            row.map_err(|e| format!("failed to map container thumbnail row: {e}"))?,
        ));
    }

    Ok(paths)
}

fn collect_missing_file_thumbnail_record_ids(conn: &Connection) -> Result<Vec<i64>, String> {
    let mut stmt = conn
        .prepare("SELECT file_id, thumbnail_path FROM thumbnails")
        .map_err(|e| format!("failed to prepare file thumbnail scan: {e}"))?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))
        .map_err(|e| format!("failed to query file thumbnail scan: {e}"))?;

    let mut missing = Vec::new();
    for row in rows {
        let (file_id, path) = row.map_err(|e| format!("failed to map file thumbnail scan row: {e}"))?;
        if !Path::new(&path).exists() {
            missing.push(file_id);
        }
    }
    Ok(missing)
}

fn collect_missing_container_thumbnail_keys(conn: &Connection) -> Result<Vec<(i64, i64)>, String> {
    let mut stmt = conn
        .prepare("SELECT container_id, slot_index, thumbnail_path FROM container_thumbnails")
        .map_err(|e| format!("failed to prepare container thumbnail scan: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| format!("failed to query container thumbnail scan: {e}"))?;

    let mut missing = Vec::new();
    for row in rows {
        let (container_id, slot_index, path) =
            row.map_err(|e| format!("failed to map container thumbnail scan row: {e}"))?;
        if !Path::new(&path).exists() {
            missing.push((container_id, slot_index));
        }
    }
    Ok(missing)
}

fn list_thumbnail_cache_files(app: &tauri::AppHandle) -> Result<Vec<PathBuf>, String> {
    let thumb_dir = thumbnail_cache_dir(app)?;
    let mut files = Vec::new();

    let entries = std::fs::read_dir(&thumb_dir)
        .map_err(|e| format!("failed to read thumbnail dir {:?}: {e}", thumb_dir))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("failed to read thumbnail dir entry: {e}"))?;
        let path = entry.path();
        if path.is_file() {
            files.push(path);
        }
    }

    Ok(files)
}

fn build_thumbnail_maintenance_result(
    conn: &Connection,
    cache_files: &[PathBuf],
    dry_run: bool,
    removed_file_thumbnail_records: usize,
    removed_container_thumbnail_records: usize,
    removed_orphaned_cache_files: usize,
) -> Result<ThumbnailMaintenanceResult, String> {
    let referenced_paths = collect_referenced_thumbnail_paths(conn)?;
    let missing_file_thumbnail_records = collect_missing_file_thumbnail_record_ids(conn)?;
    let missing_container_thumbnail_records = collect_missing_container_thumbnail_keys(conn)?;
    let orphaned_cache_files = cache_files
        .iter()
        .filter(|path| !referenced_paths.contains(*path))
        .count();

    Ok(ThumbnailMaintenanceResult {
        dry_run,
        referenced_thumbnail_files: referenced_paths.len(),
        missing_file_thumbnail_records: missing_file_thumbnail_records.len(),
        missing_container_thumbnail_records: missing_container_thumbnail_records.len(),
        orphaned_cache_files,
        removed_file_thumbnail_records,
        removed_container_thumbnail_records,
        removed_orphaned_cache_files,
    })
}

fn probe_video_duration_seconds(file_path: &str) -> Option<f64> {
    let output = std::process::Command::new("ffprobe.exe")
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(file_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8(output.stdout).ok()?;
    text.trim().parse::<f64>().ok().filter(|v| *v > 0.0)
}

fn generate_video_thumbnail_set(
    app: &tauri::AppHandle,
    file_path: &str,
    file_hash: &str,
    size: i64,
    slots: i64,
) -> Result<Vec<(i64, String)>, String> {
    let source_path = Path::new(file_path);
    if !source_path.exists() || slots <= 0 {
        return Ok(Vec::new());
    }

    let duration = probe_video_duration_seconds(file_path).unwrap_or(0.0);
    let mut generated = Vec::new();

    for slot in 0..slots {
        let output_path = thumbnail_cache_path_for_slot(app, file_hash, size, slot)?;
        if output_path.exists() {
            generated.push((slot, output_path.to_string_lossy().to_string()));
            continue;
        }

        let ts = if duration > 0.0 {
            ((slot as f64 + 0.5) / slots as f64) * duration
        } else {
            1.0 + slot as f64
        };

        let timestamp = format!("{ts:.3}");
        let output = std::process::Command::new("ffmpeg.exe")
            .arg("-y")
            .arg("-loglevel")
            .arg("error")
            .arg("-ss")
            .arg(timestamp)
            .arg("-i")
            .arg(source_path)
            .arg("-vf")
            .arg("scale=320:-1:flags=lanczos")
            .arg("-frames:v")
            .arg("1")
            .arg(&output_path)
            .output()
            .map_err(|e| format!("failed to run ffmpeg for video thumbnail slot {slot}: {e}"))?;

        if output.status.success() && output_path.exists() {
            generated.push((slot, output_path.to_string_lossy().to_string()));
        }
    }

    Ok(generated)
}

fn find_archive_extractor() -> Option<&'static str> {
    let candidates = ["7z.exe", "7za.exe", "7z"];
    for cmd in candidates {
        let ok = std::process::Command::new(cmd)
            .arg("-h")
            .output()
            .map(|o| o.status.success() || !o.stdout.is_empty() || !o.stderr.is_empty())
            .unwrap_or(false);
        if ok {
            return Some(cmd);
        }
    }
    None
}

fn generate_archive_thumbnail_set(
    app: &tauri::AppHandle,
    archive_path: &str,
    file_hash: &str,
    size: i64,
    slots: i64,
) -> Result<(Vec<(i64, String)>, Option<String>), String> {
    if slots <= 0 {
        return Ok((Vec::new(), Some("no thumbnail slots requested".to_string())));
    }

    let extractor = match find_archive_extractor() {
        Some(v) => v,
        None => {
            return Ok((
                Vec::new(),
                Some("archive extractor not found (7z/7za)".to_string()),
            ));
        }
    };

    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("failed to resolve app data dir: {e}"))?;
    let extract_dir = app_data
        .join("archive_extract")
        .join(format!("{file_hash}_{size}"));

    if extract_dir.exists() {
        let _ = std::fs::remove_dir_all(&extract_dir);
    }
    std::fs::create_dir_all(&extract_dir)
        .map_err(|e| format!("failed to create archive extraction dir {:?}: {e}", extract_dir))?;

    let extract_status = std::process::Command::new(extractor)
        .arg("x")
        .arg("-y")
        .arg(format!("-o{}", extract_dir.to_string_lossy()))
        .arg(archive_path)
        .output();

    match extract_status {
        Ok(output) if output.status.success() => {}
        _ => return Ok((Vec::new(), Some("archive extraction failed".to_string()))),
    }

    let scan = scan_archive_extracted_images(&extract_dir)?;
    let mut image_candidates = scan.image_candidates;
    if image_candidates.is_empty() {
        let sampled = if scan.sampled_extensions.is_empty() {
            "none".to_string()
        } else {
            scan.sampled_extensions.join(", ")
        };
        return Ok((
            Vec::new(),
            Some(format!(
                "no image files found in archive (extracted files: {}, sampled extensions: {})",
                scan.extracted_file_count, sampled
            )),
        ));
    }

    let total_candidates = image_candidates.len();
    image_candidates.sort();

    let mut generated = Vec::new();
    for (slot, image_path) in image_candidates.into_iter().take(slots as usize).enumerate() {
        let slot_index = slot as i64;
        let output_path = thumbnail_cache_path_for_slot(app, file_hash, size, slot_index)?;
        if output_path.exists() {
            generated.push((slot_index, output_path.to_string_lossy().to_string()));
            continue;
        }

        let output = std::process::Command::new("ffmpeg.exe")
            .arg("-y")
            .arg("-loglevel")
            .arg("error")
            .arg("-i")
            .arg(&image_path)
            .arg("-vf")
            .arg("scale=320:-1:flags=lanczos")
            .arg("-frames:v")
            .arg("1")
            .arg(&output_path)
            .output()
            .map_err(|e| format!("failed to run ffmpeg for archive thumbnail slot {slot_index}: {e}"))?;

        if output.status.success() && output_path.exists() {
            generated.push((slot_index, output_path.to_string_lossy().to_string()));
        }
    }

    if generated.is_empty() {
        return Ok((
            generated,
            Some(format!(
                "failed to generate thumbnails from extracted images (candidates: {total_candidates})"
            )),
        ));
    }

    Ok((generated, None))
}

struct ArchiveImageScanResult {
    image_candidates: Vec<PathBuf>,
    extracted_file_count: usize,
    sampled_extensions: Vec<String>,
}

fn normalize_extension_from_path(path: &Path) -> String {
    path.extension()
        .and_then(|v| v.to_str())
        .map(|s| s.trim().trim_start_matches('.').to_ascii_lowercase())
        .unwrap_or_default()
}

fn is_archive_image_extension(ext: &str) -> bool {
    is_image_extension(ext)
        || matches!(
            ext,
            "jfif" | "dib" | "tif" | "tiff" | "heic" | "heif"
        )
}

fn is_archive_video_extension(ext: &str) -> bool {
    is_video_extension(ext)
}

fn is_archive_container_extension(ext: &str) -> bool {
    matches!(ext, "rar" | "7z" | "zip" | "lzh" | "cbz" | "cbr" | "cb7" | "iso" | "pdf")
}

fn scan_archive_extracted_images(extract_dir: &Path) -> Result<ArchiveImageScanResult, String> {
    let mut image_candidates: Vec<PathBuf> = Vec::new();
    let mut extracted_file_count = 0usize;
    let mut sampled_extensions: Vec<String> = Vec::new();

    for entry in WalkDir::new(&extract_dir).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        extracted_file_count += 1;

        let ext = normalize_extension_from_path(entry.path());
        let sample = if ext.is_empty() {
            "(no_ext)".to_string()
        } else {
            ext.clone()
        };
        if sampled_extensions.len() < 8 && !sampled_extensions.iter().any(|v| v == &sample) {
            sampled_extensions.push(sample);
        }

        if is_archive_image_extension(&ext) {
            image_candidates.push(entry.path().to_path_buf());
        }
    }

    Ok(ArchiveImageScanResult {
        image_candidates,
        extracted_file_count,
        sampled_extensions,
    })
}

fn normalize_archive_relative_path(path: &Path) -> String {
    let mut parts: Vec<String> = Vec::new();
    for component in path.components() {
        if let Component::Normal(segment) = component {
            let value = segment.to_string_lossy().trim().to_string();
            if !value.is_empty() {
                parts.push(value);
            }
        }
    }
    parts.join("/")
}

struct NestedArchiveCandidate {
    archive_file_path: PathBuf,
    archive_logical_path: String,
}

fn collect_media_and_nested_archives_in_dir(
    scan_root: &Path,
    logical_prefix: Option<&str>,
) -> (Vec<ArchiveVirtualMediaEntry>, Vec<NestedArchiveCandidate>) {
    let mut files: Vec<(String, PathBuf)> = Vec::new();

    for entry in WalkDir::new(scan_root).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(scan_root) else {
            continue;
        };
        let rel = normalize_archive_relative_path(relative);
        if rel.is_empty() {
            continue;
        }
        files.push((rel, entry.path().to_path_buf()));
    }

    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut media_entries: Vec<ArchiveVirtualMediaEntry> = Vec::new();
    let mut nested_archives: Vec<NestedArchiveCandidate> = Vec::new();

    for (relative_path, absolute_path) in files {
        let logical_path = match logical_prefix {
            Some(prefix) if !prefix.is_empty() => format!("{prefix}/{relative_path}"),
            _ => relative_path,
        };

        let ext = normalize_extension_from_path(&absolute_path);
        if is_archive_image_extension(&ext) {
            media_entries.push(ArchiveVirtualMediaEntry {
                relative_path: logical_path.clone(),
                media_kind: "image",
                source_path: absolute_path.clone(),
            });
        } else if is_archive_video_extension(&ext) {
            media_entries.push(ArchiveVirtualMediaEntry {
                relative_path: logical_path.clone(),
                media_kind: "video",
                source_path: absolute_path.clone(),
            });
        }

        if is_archive_container_extension(&ext) {
            nested_archives.push(NestedArchiveCandidate {
                archive_file_path: absolute_path,
                archive_logical_path: logical_path,
            });
        }
    }

    (media_entries, nested_archives)
}

fn collect_archive_virtual_media_entries(
    extractor: &str,
    extract_dir: &Path,
    max_depth: usize,
) -> Vec<ArchiveVirtualMediaEntry> {
    let (mut entries, initial_nested_archives) =
        collect_media_and_nested_archives_in_dir(extract_dir, None);

    let mut queue: VecDeque<NestedArchiveCandidate> = VecDeque::from(initial_nested_archives);
    let nested_root = extract_dir.join("__nested_archives");
    let _ = std::fs::create_dir_all(&nested_root);
    let mut extraction_counter: usize = 0;

    while let Some(candidate) = queue.pop_front() {
        let depth = candidate
            .archive_logical_path
            .split('/')
            .filter(|segment| {
                let ext = Path::new(segment)
                    .extension()
                    .and_then(|v| v.to_str())
                    .map(|v| v.to_ascii_lowercase())
                    .unwrap_or_default();
                is_archive_container_extension(&ext)
            })
            .count();

        if depth > max_depth {
            continue;
        }

        let nested_extract_dir = nested_root.join(format!("d{depth}_{:06}", extraction_counter));
        extraction_counter = extraction_counter.saturating_add(1);

        if nested_extract_dir.exists() {
            let _ = std::fs::remove_dir_all(&nested_extract_dir);
        }
        if let Err(err) = std::fs::create_dir_all(&nested_extract_dir) {
            eprintln!(
                "archive virtual nested extraction dir create failed [{}]: {}",
                candidate.archive_logical_path, err
            );
            continue;
        }

        let extraction = std::process::Command::new(extractor)
            .arg("x")
            .arg("-y")
            .arg(format!("-o{}", nested_extract_dir.to_string_lossy()))
            .arg(&candidate.archive_file_path)
            .output();

        match extraction {
            Ok(output) if output.status.success() => {
                let (nested_media, nested_archives) = collect_media_and_nested_archives_in_dir(
                    &nested_extract_dir,
                    Some(&candidate.archive_logical_path),
                );
                entries.extend(nested_media);

                if depth < max_depth {
                    for nested in nested_archives {
                        queue.push_back(nested);
                    }
                }
            }
            Ok(_) => {
                eprintln!(
                    "archive virtual nested extraction command failed [{}]",
                    candidate.archive_logical_path
                );
            }
            Err(err) => {
                eprintln!(
                    "archive virtual nested extraction invocation failed [{}]: {}",
                    candidate.archive_logical_path, err
                );
            }
        }
    }

    entries.sort_by(|a, b| {
        a.relative_path
            .cmp(&b.relative_path)
            .then(a.media_kind.cmp(b.media_kind))
            .then(a.source_path.to_string_lossy().cmp(&b.source_path.to_string_lossy()))
    });
    entries.dedup_by(|a, b| a.relative_path == b.relative_path && a.media_kind == b.media_kind);
    entries
}

fn tree_child_mut<'a>(node: &'a mut ArchiveVirtualTreeNode, name: &str) -> &'a mut ArchiveVirtualTreeNode {
    if let Some(index) = node.children.iter().position(|child| child.name == name) {
        return &mut node.children[index];
    }
    node.children.push(ArchiveVirtualTreeNode::new(name.to_string()));
    let index = node.children.len().saturating_sub(1);
    &mut node.children[index]
}

fn build_archive_virtual_tree(entries: &[ArchiveVirtualMediaEntry]) -> ArchiveVirtualTreeNode {
    let mut root = ArchiveVirtualTreeNode::default();

    for entry in entries {
        let parts: Vec<&str> = entry
            .relative_path
            .split('/')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect();
        if parts.is_empty() {
            continue;
        }

        let mut node = &mut root;
        if parts.len() > 1 {
            for part in &parts[..parts.len() - 1] {
                node = tree_child_mut(node, part);
            }
        }

        if entry.media_kind == "image" {
            node.has_image_files = true;
        } else if entry.media_kind == "video" {
            node.has_video_files = true;
        }
    }

    root
}

fn compress_archive_virtual_tree(node: &mut ArchiveVirtualTreeNode, allow_self_compress: bool) {
    for child in &mut node.children {
        compress_archive_virtual_tree(child, true);
    }

    if !allow_self_compress {
        node.children.sort_by(|a, b| a.name.cmp(&b.name));
        return;
    }

    loop {
        let can_compress = !node.has_image_files && !node.has_video_files && node.children.len() == 1;
        if !can_compress {
            break;
        }

        let child = node.children.remove(0);
        if node.name.is_empty() {
            node.name = child.name;
        } else if !child.name.is_empty() {
            node.name = format!("{}/{}", node.name, child.name);
        }
        node.has_image_files = child.has_image_files;
        node.has_video_files = child.has_video_files;
        node.children = child.children;
    }

    node.children.sort_by(|a, b| a.name.cmp(&b.name));
}

fn join_virtual_path(base: &str, child: &str) -> String {
    if base == "." {
        child.to_string()
    } else {
        format!("{base}/{child}")
    }
}

fn collect_archive_virtual_nodes(
    node: &ArchiveVirtualTreeNode,
    current_path: &str,
    parent_path: Option<&str>,
    depth: i64,
    out: &mut Vec<ArchiveVirtualPersistNode>,
) {
    out.push(ArchiveVirtualPersistNode {
        virtual_path: current_path.to_string(),
        parent_virtual_path: parent_path.map(|v| v.to_string()),
        node_kind: "path",
        media_kind: None,
        depth,
    });

    if node.has_image_files {
        out.push(ArchiveVirtualPersistNode {
            virtual_path: join_virtual_path(current_path, "@image"),
            parent_virtual_path: Some(current_path.to_string()),
            node_kind: "media",
            media_kind: Some("image"),
            depth: depth + 1,
        });
    }
    if node.has_video_files {
        out.push(ArchiveVirtualPersistNode {
            virtual_path: join_virtual_path(current_path, "@video"),
            parent_virtual_path: Some(current_path.to_string()),
            node_kind: "media",
            media_kind: Some("video"),
            depth: depth + 1,
        });
    }

    for child in &node.children {
        let child_path = join_virtual_path(current_path, &child.name);
        collect_archive_virtual_nodes(child, &child_path, Some(current_path), depth + 1, out);
    }
}

fn virtual_container_slot_thumbnail_cache_path(
    app: &tauri::AppHandle,
    file_hash: &str,
    size: i64,
    virtual_path: &str,
    slot_index: i64,
) -> Result<PathBuf, String> {
    let thumb_dir = thumbnail_cache_dir(app)?;
    let mut hasher = Sha256::new();
    hasher.update(virtual_path.as_bytes());
    let virtual_key = hex::encode(hasher.finalize());
    Ok(thumb_dir.join(format!(
        "{file_hash}_{size}_v_{virtual_key}_{slot_index}.png"
    )))
}

fn pick_even_index(total: usize, position: usize, slots: usize) -> usize {
    if total == 0 || slots == 0 {
        return 0;
    }
    (position.saturating_mul(total)) / slots
}

fn path_parent_virtual_path(path: &str) -> String {
    if path == "." {
        return ".".to_string();
    }
    let p = Path::new(path);
    let Some(parent) = p.parent() else {
        return ".".to_string();
    };
    let normalized = normalize_archive_relative_path(parent);
    if normalized.is_empty() {
        ".".to_string()
    } else {
        normalized
    }
}

fn resolve_media_parent_path(parent: &str, path_nodes: &[String]) -> String {
    let mut best = ".".to_string();
    let mut best_len = 0usize;

    for candidate in path_nodes {
        let matches = if candidate == "." {
            true
        } else {
            parent == candidate || parent.starts_with(&format!("{candidate}/"))
        };

        if matches && candidate.len() >= best_len {
            best = candidate.clone();
            best_len = candidate.len();
        }
    }

    best
}

fn generate_image_virtual_slot(
    app: &tauri::AppHandle,
    source_path: &Path,
    file_hash: &str,
    size: i64,
    virtual_path: &str,
    slot_index: i64,
) -> Result<Option<String>, String> {
    let output_path = virtual_container_slot_thumbnail_cache_path(
        app,
        file_hash,
        size,
        virtual_path,
        slot_index,
    )?;

    if output_path.exists() {
        return Ok(Some(output_path.to_string_lossy().to_string()));
    }

    let output = std::process::Command::new("ffmpeg.exe")
        .arg("-y")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(source_path)
        .arg("-vf")
        .arg("scale=320:-1:flags=lanczos")
        .arg("-frames:v")
        .arg("1")
        .arg(&output_path)
        .output()
        .map_err(|e| format!("failed to run ffmpeg for virtual image thumbnail: {e}"))?;

    if output.status.success() && output_path.exists() {
        return Ok(Some(output_path.to_string_lossy().to_string()));
    }

    if output_path.exists() {
        let _ = std::fs::remove_file(&output_path);
    }
    Ok(None)
}

fn generate_video_virtual_slot(
    app: &tauri::AppHandle,
    source_path: &Path,
    file_hash: &str,
    size: i64,
    virtual_path: &str,
    slot_index: i64,
    slot_position_in_video: usize,
    slots_for_video: usize,
) -> Result<Option<String>, String> {
    let output_path = virtual_container_slot_thumbnail_cache_path(
        app,
        file_hash,
        size,
        virtual_path,
        slot_index,
    )?;

    if output_path.exists() {
        return Ok(Some(output_path.to_string_lossy().to_string()));
    }

    let source_text = source_path.to_string_lossy().to_string();
    let duration = probe_video_duration_seconds(&source_text).unwrap_or(0.0);
    let ts = if duration > 0.0 && slots_for_video > 0 {
        ((slot_position_in_video as f64 + 0.5) / slots_for_video as f64) * duration
    } else {
        1.0 + slot_position_in_video as f64
    };

    let output = std::process::Command::new("ffmpeg.exe")
        .arg("-y")
        .arg("-loglevel")
        .arg("error")
        .arg("-ss")
        .arg(format!("{ts:.3}"))
        .arg("-i")
        .arg(source_path)
        .arg("-vf")
        .arg("scale=320:-1:flags=lanczos")
        .arg("-frames:v")
        .arg("1")
        .arg(&output_path)
        .output()
        .map_err(|e| format!("failed to run ffmpeg for virtual video thumbnail: {e}"))?;

    if output.status.success() && output_path.exists() {
        return Ok(Some(output_path.to_string_lossy().to_string()));
    }

    if output_path.exists() {
        let _ = std::fs::remove_file(&output_path);
    }
    Ok(None)
}

fn persist_container_thumbnail_slots(
    conn: &Connection,
    container_id: i64,
    slot_paths: &[String],
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM container_thumbnails WHERE container_id = ?",
        [container_id],
    )
    .map_err(|e| format!("failed to clear virtual container thumbnails for {container_id}: {e}"))?;

    for (idx, path) in slot_paths.iter().enumerate() {
        let slot = idx as i64;
        let _ = upsert_container_thumbnail(conn, container_id, slot, path)?;
    }

    Ok(())
}

fn build_non_leaf_aggregate_slots(
    child_slot_sets: &[Vec<String>],
    max_slots: usize,
) -> Vec<String> {
    let active_children: Vec<&Vec<String>> = child_slot_sets
        .iter()
        .filter(|slots| !slots.is_empty())
        .collect();
    let child_count = active_children.len();

    if child_count == 0 || max_slots == 0 {
        return Vec::new();
    }

    if child_count >= max_slots {
        return active_children
            .iter()
            .take(max_slots)
            .map(|slots| slots[0].clone())
            .collect();
    }

    let mut counts = vec![max_slots / child_count; child_count];
    for count in counts.iter_mut().take(max_slots % child_count) {
        *count += 1;
    }

    let mut cursors = vec![0usize; child_count];
    let mut output: Vec<String> = Vec::with_capacity(max_slots);

    while output.len() < max_slots {
        let mut progressed = false;
        for i in 0..child_count {
            if counts[i] == 0 {
                continue;
            }
            let source = active_children[i];
            let value = source[cursors[i] % source.len()].clone();
            output.push(value);
            counts[i] -= 1;
            cursors[i] += 1;
            progressed = true;
            if output.len() >= max_slots {
                break;
            }
        }

        if !progressed {
            break;
        }
    }

    output
}

fn rebuild_archive_virtual_container_thumbnails(
    app: &tauri::AppHandle,
    conn: &Connection,
    archive_file_hash: &str,
    archive_file_size: i64,
    nodes: &[ArchiveVirtualPersistNode],
    entries: &[ArchiveVirtualMediaEntry],
    id_by_virtual_path: &HashMap<String, i64>,
    max_slots: usize,
) -> Result<(), String> {
    if max_slots == 0 {
        return Ok(());
    }

    let path_nodes: Vec<String> = nodes
        .iter()
        .filter(|node| node.node_kind == "path")
        .map(|node| node.virtual_path.clone())
        .collect();
    let media_nodes: HashSet<String> = nodes
        .iter()
        .filter(|node| node.node_kind == "media")
        .map(|node| node.virtual_path.clone())
        .collect();

    let mut media_entries_by_node: HashMap<String, Vec<&ArchiveVirtualMediaEntry>> = HashMap::new();
    for entry in entries {
        let parent = path_parent_virtual_path(&entry.relative_path);
        let resolved_parent = resolve_media_parent_path(&parent, &path_nodes);
        let media_path = join_virtual_path(&resolved_parent, if entry.media_kind == "video" { "@video" } else { "@image" });
        if media_nodes.contains(&media_path) {
            media_entries_by_node.entry(media_path).or_default().push(entry);
        }
    }

    let mut slot_paths_by_virtual_path: HashMap<String, Vec<String>> = HashMap::new();

    for node in nodes.iter().filter(|node| node.node_kind == "media") {
        let Some(container_id) = id_by_virtual_path.get(&node.virtual_path).copied() else {
            continue;
        };

        let mut group_entries: Vec<&ArchiveVirtualMediaEntry> = media_entries_by_node
            .get(&node.virtual_path)
            .cloned()
            .unwrap_or_default();
        group_entries.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

        let mut generated_paths: Vec<String> = Vec::new();
        if node.media_kind == Some("image") {
            if !group_entries.is_empty() {
                for slot in 0..max_slots {
                    let idx = pick_even_index(group_entries.len(), slot, max_slots).min(group_entries.len() - 1);
                    if let Some(path) = generate_image_virtual_slot(
                        app,
                        &group_entries[idx].source_path,
                        archive_file_hash,
                        archive_file_size,
                        &node.virtual_path,
                        slot as i64,
                    )? {
                        generated_paths.push(path);
                    }
                }
            }
        } else if node.media_kind == Some("video") {
            if !group_entries.is_empty() {
                let mut slots_per_video = vec![0usize; group_entries.len()];
                let mut slot_video_index: Vec<usize> = Vec::with_capacity(max_slots);
                for slot in 0..max_slots {
                    let idx = pick_even_index(group_entries.len(), slot, max_slots).min(group_entries.len() - 1);
                    slot_video_index.push(idx);
                    slots_per_video[idx] += 1;
                }

                let mut used_per_video = vec![0usize; group_entries.len()];
                for (slot, video_idx) in slot_video_index.into_iter().enumerate() {
                    let position = used_per_video[video_idx];
                    used_per_video[video_idx] = used_per_video[video_idx].saturating_add(1);
                    if let Some(path) = generate_video_virtual_slot(
                        app,
                        &group_entries[video_idx].source_path,
                        archive_file_hash,
                        archive_file_size,
                        &node.virtual_path,
                        slot as i64,
                        position,
                        slots_per_video[video_idx].max(1),
                    )? {
                        generated_paths.push(path);
                    }
                }
            }
        }

        persist_container_thumbnail_slots(conn, container_id, &generated_paths)?;
        slot_paths_by_virtual_path.insert(node.virtual_path.clone(), generated_paths);
    }

    let mut children_by_parent: HashMap<String, Vec<String>> = HashMap::new();
    for node in nodes {
        if let Some(parent) = &node.parent_virtual_path {
            children_by_parent
                .entry(parent.clone())
                .or_default()
                .push(node.virtual_path.clone());
        }
    }
    for children in children_by_parent.values_mut() {
        children.sort();
    }

    let mut path_nodes_desc: Vec<&ArchiveVirtualPersistNode> = nodes
        .iter()
        .filter(|node| node.node_kind == "path")
        .collect();
    path_nodes_desc.sort_by(|a, b| b.depth.cmp(&a.depth).then(a.virtual_path.cmp(&b.virtual_path)));

    for node in path_nodes_desc {
        let Some(container_id) = id_by_virtual_path.get(&node.virtual_path).copied() else {
            continue;
        };

        let child_paths = children_by_parent
            .get(&node.virtual_path)
            .cloned()
            .unwrap_or_default();
        let child_slot_sets: Vec<Vec<String>> = child_paths
            .iter()
            .map(|child| slot_paths_by_virtual_path.get(child).cloned().unwrap_or_default())
            .collect();

        let aggregated = build_non_leaf_aggregate_slots(&child_slot_sets, max_slots);
        persist_container_thumbnail_slots(conn, container_id, &aggregated)?;
        slot_paths_by_virtual_path.insert(node.virtual_path.clone(), aggregated);
    }

    Ok(())
}

fn persist_archive_virtual_hierarchy(
    conn: &Connection,
    archive_container_id: i64,
    archive_source_path: &str,
    nodes: &[ArchiveVirtualPersistNode],
) -> Result<HashMap<String, i64>, String> {
    let mut existing_stmt = conn
        .prepare(
            "
            SELECT virtual_container_id
            FROM archive_virtual_container_meta
            WHERE archive_container_id = ?
            ",
        )
        .map_err(|e| format!("failed to prepare archive virtual metadata query: {e}"))?;

    let existing_rows = existing_stmt
        .query_map([archive_container_id], |row| row.get::<_, i64>(0))
        .map_err(|e| format!("failed to query archive virtual metadata rows: {e}"))?;

    let mut stale_container_ids: Vec<i64> = Vec::new();
    for row in existing_rows {
        stale_container_ids.push(row.map_err(|e| format!("failed to map archive virtual metadata row: {e}"))?);
    }

    for container_id in stale_container_ids {
        conn.execute("DELETE FROM containers WHERE id = ?", [container_id])
            .map_err(|e| format!("failed to delete stale archive virtual container {container_id}: {e}"))?;
    }

    if nodes.is_empty() {
        return Ok(HashMap::new());
    }

    let mut id_by_virtual_path: HashMap<String, i64> = HashMap::new();

    for node in nodes {
        let source_path = format!("{archive_source_path}::{}", node.virtual_path);
        conn.execute(
            "
            INSERT INTO containers(container_type, display_name, source_path)
            VALUES ('archive_virtual', ?, ?)
            ",
            params![node.virtual_path, source_path],
        )
        .map_err(|e| format!("failed to insert archive virtual container {}: {e}", node.virtual_path))?;

        let virtual_container_id = conn.last_insert_rowid();
        let parent_virtual_container_id = node
            .parent_virtual_path
            .as_ref()
            .and_then(|path| id_by_virtual_path.get(path))
            .copied();

        conn.execute(
            "
            INSERT INTO archive_virtual_container_meta(
                virtual_container_id,
                archive_container_id,
                parent_virtual_container_id,
                virtual_path,
                node_kind,
                media_kind,
                depth,
                created_at,
                updated_at
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, datetime('now'), datetime('now'))
            ",
            params![
                virtual_container_id,
                archive_container_id,
                parent_virtual_container_id,
                node.virtual_path,
                node.node_kind,
                node.media_kind,
                node.depth,
            ],
        )
        .map_err(|e| format!("failed to insert archive virtual metadata {}: {e}", node.virtual_path))?;

        if let Some(parent_id) = parent_virtual_container_id {
            conn.execute(
                "
                INSERT OR IGNORE INTO container_children(parent_container_id, child_container_id)
                VALUES (?, ?)
                ",
                params![parent_id, virtual_container_id],
            )
            .map_err(|e| format!("failed to link archive virtual parent-child: {e}"))?;
        } else {
            conn.execute(
                "
                INSERT OR IGNORE INTO container_children(parent_container_id, child_container_id)
                VALUES (?, ?)
                ",
                params![archive_container_id, virtual_container_id],
            )
            .map_err(|e| format!("failed to link archive root virtual container: {e}"))?;
        }

        id_by_virtual_path.insert(node.virtual_path.clone(), virtual_container_id);
    }

    conn.execute(
        "UPDATE containers SET updated_at = datetime('now') WHERE id = ?",
        [archive_container_id],
    )
    .map_err(|e| format!("failed to update archive container timestamp after virtual rebuild: {e}"))?;

    Ok(id_by_virtual_path)
}

fn rebuild_archive_virtual_hierarchy(
    app: &tauri::AppHandle,
    conn: &Connection,
    archive_container_id: i64,
    archive_source_path: &str,
    file_hash: &str,
    file_size: i64,
) -> Result<(), String> {
    let nested_depth_limit = archive_traversal_depth_for_connection(conn)?;
    let extractor = match find_archive_extractor() {
        Some(v) => v,
        None => return Ok(()),
    };

    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("failed to resolve app data dir: {e}"))?;
    let extract_dir = app_data
        .join("archive_extract_virtual")
        .join(format!("{file_hash}_{file_size}"));

    if extract_dir.exists() {
        let _ = std::fs::remove_dir_all(&extract_dir);
    }
    std::fs::create_dir_all(&extract_dir)
        .map_err(|e| format!("failed to create archive virtual extraction dir {:?}: {e}", extract_dir))?;

    let extract_status = std::process::Command::new(extractor)
        .arg("x")
        .arg("-y")
        .arg(format!("-o{}", extract_dir.to_string_lossy()))
        .arg(archive_source_path)
        .output();

    match extract_status {
        Ok(output) if output.status.success() => {}
        _ => return Ok(()),
    }

    let entries = collect_archive_virtual_media_entries(extractor, &extract_dir, nested_depth_limit);
    if entries.is_empty() {
        persist_archive_virtual_hierarchy(conn, archive_container_id, archive_source_path, &[])?;
        return Ok(());
    }

    let mut tree = build_archive_virtual_tree(&entries);
    for child in &mut tree.children {
        compress_archive_virtual_tree(child, true);
    }
    tree.children.sort_by(|a, b| a.name.cmp(&b.name));

    let mut nodes: Vec<ArchiveVirtualPersistNode> = Vec::new();
    collect_archive_virtual_nodes(&tree, ".", None, 0, &mut nodes);
    let id_by_virtual_path =
        persist_archive_virtual_hierarchy(conn, archive_container_id, archive_source_path, &nodes)?;

    rebuild_archive_virtual_container_thumbnails(
        app,
        conn,
        file_hash,
        file_size,
        &nodes,
        &entries,
        &id_by_virtual_path,
        16,
    )
}

fn generate_thumbnail_for_file(
    app: &tauri::AppHandle,
    file_path: &str,
    file_hash: &str,
    size: i64,
    kind: &str,
) -> Result<Option<String>, String> {
    let source_path = Path::new(file_path);
    if !source_path.exists() {
        return Ok(None);
    }

    let output_path = thumbnail_cache_path(app, file_hash, size)?;
    if output_path.exists() {
        return Ok(Some(output_path.to_string_lossy().to_string()));
    }

    let mut command = std::process::Command::new("ffmpeg.exe");
    command
        .arg("-y")
        .arg("-loglevel")
        .arg("error");

    if kind == "video" {
        command.arg("-ss").arg("00:00:01.000");
    }

    let output = command
        .arg("-i")
        .arg(source_path)
        .arg("-vf")
        .arg("scale=320:-1:flags=lanczos")
        .arg("-frames:v")
        .arg("1")
        .arg(&output_path)
        .output()
        .map_err(|e| format!("failed to run ffmpeg for thumbnail generation: {e}"))?;

    if output.status.success() && output_path.exists() {
        return Ok(Some(output_path.to_string_lossy().to_string()));
    }

    if output_path.exists() {
        let _ = std::fs::remove_file(&output_path);
    }

    Ok(None)
}

fn upsert_thumbnail(conn: &Connection, file_id: i64, thumbnail_path: &str) -> Result<bool, String> {
    let changed = conn
        .execute(
            "
            INSERT INTO thumbnails(file_id, thumbnail_path, created_at, updated_at)
            VALUES (?, ?, datetime('now'), datetime('now'))
            ON CONFLICT(file_id) DO UPDATE SET
              thumbnail_path = excluded.thumbnail_path,
              updated_at = datetime('now')
            ",
            params![file_id, thumbnail_path],
        )
        .map_err(|e| format!("failed to upsert thumbnail: {e}"))?;

    Ok(changed > 0)
}

fn upsert_container_thumbnail(
    conn: &Connection,
    container_id: i64,
    slot_index: i64,
    thumbnail_path: &str,
) -> Result<bool, String> {
    let changed = conn
        .execute(
            "
            INSERT INTO container_thumbnails(container_id, slot_index, thumbnail_path, created_at, updated_at)
            VALUES (?, ?, ?, datetime('now'), datetime('now'))
            ON CONFLICT(container_id, slot_index) DO UPDATE SET
              thumbnail_path = excluded.thumbnail_path,
              updated_at = datetime('now')
            ",
            params![container_id, slot_index, thumbnail_path],
        )
        .map_err(|e| format!("failed to upsert container thumbnail: {e}"))?;

    Ok(changed > 0)
}

fn thumbnail_data_url_from_path(path: &str) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let ext = Path::new(path)
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or_default()
        .to_lowercase();
    let mime = match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        _ => "image/png",
    };
    Some(format!("data:{mime};base64,{}", BASE64_STANDARD.encode(bytes)))
}

fn split_tags_csv(tags_csv: &str) -> Vec<String> {
    tags_csv
        .split(',')
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string())
        .collect()
}

fn duplicate_identity_for_file(
    conn: &Connection,
    file_id: i64,
) -> Result<(String, String, String, i64), String> {
    conn.query_row(
        "SELECT path, filename, hash, size FROM files WHERE id = ?",
        [file_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        },
    )
    .map_err(|e| format!("failed to resolve file {file_id}: {e}"))
}

fn file_has_duplicate_peer(conn: &Connection, hash: &str, size: i64, file_id: i64) -> Result<bool, String> {
    let count = conn
        .query_row(
            "SELECT COUNT(1) FROM files WHERE hash = ? AND size = ? AND id != ?",
            params![hash, size, file_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| format!("failed to query duplicate peers for file {file_id}: {e}"))?;
    Ok(count > 0)
}

fn move_file_path(source_path: &Path, destination_path: &Path) -> Result<(), String> {
    if destination_path.exists() {
        return Err(format!(
            "destination already exists: {}",
            destination_path.to_string_lossy()
        ));
    }

    if let Some(parent) = destination_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "failed to create destination directory {}: {e}",
                parent.to_string_lossy()
            )
        })?;
    }

    if let Err(rename_err) = std::fs::rename(source_path, destination_path) {
        std::fs::copy(source_path, destination_path).map_err(|copy_err| {
            format!(
                "failed to move file (rename: {rename_err}, copy fallback: {copy_err})"
            )
        })?;
        std::fs::remove_file(source_path)
            .map_err(|e| format!("failed to remove original file after copy move: {e}"))?;
    }

    Ok(())
}

fn unique_destination_path(base_dir: &Path, base_name: &str) -> PathBuf {
    let initial = base_dir.join(base_name);
    if !initial.exists() {
        return initial;
    }

    for idx in 1..=1000 {
        let candidate = base_dir.join(format!("{base_name}_{idx}"));
        if !candidate.exists() {
            return candidate;
        }
    }

    base_dir.join(format!("{base_name}_{}", std::process::id()))
}

fn quarantine_directory_for_source(app: &tauri::AppHandle, source: &Path) -> Result<PathBuf, String> {
    if let Some(parent) = source.parent() {
        return Ok(parent.join(".thumbscontainer_quarantine"));
    }

    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("failed to resolve app data dir for fallback quarantine: {e}"))?;
    Ok(app_data.join("duplicate_quarantine"))
}

fn path_is_quarantine_path(path: &Path) -> bool {
    path.components().any(|component| match component {
        Component::Normal(segment) => {
            segment
                .to_str()
                .map(|s| s.eq_ignore_ascii_case(".thumbscontainer_quarantine") || s.eq_ignore_ascii_case("duplicate_quarantine"))
                .unwrap_or(false)
        }
        _ => false,
    })
}

fn cleanup_quarantine_parent_if_empty(path: &Path) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };

    if !path_is_quarantine_path(parent) {
        return Ok(());
    }

    if !parent.exists() {
        return Ok(());
    }

    let mut entries = std::fs::read_dir(parent)
        .map_err(|e| format!("failed to read quarantine directory {}: {e}", parent.to_string_lossy()))?;
    if entries.next().is_none() {
        std::fs::remove_dir(parent)
            .map_err(|e| format!("failed to remove empty quarantine directory {}: {e}", parent.to_string_lossy()))?;
    }

    Ok(())
}

fn mark_existing_active_quarantine_history_resolved(conn: &Connection, file_id: i64) -> Result<(), String> {
    conn.execute(
        "
        UPDATE duplicate_quarantine_history
        SET restored_at = COALESCE(restored_at, datetime('now'))
        WHERE file_id = ? AND restored_at IS NULL AND purged_at IS NULL
        ",
        [file_id],
    )
    .map_err(|e| format!("failed to close existing active quarantine history for file {file_id}: {e}"))?;

    Ok(())
}

fn active_quarantine_entry(
    conn: &Connection,
    file_id: i64,
) -> Result<Option<(i64, String, String, String)>, String> {
    let entry = conn
        .query_row(
            "
            SELECT id, original_path, quarantine_path, quarantined_at
            FROM duplicate_quarantine_history
            WHERE file_id = ? AND restored_at IS NULL AND purged_at IS NULL
            ORDER BY id DESC
            LIMIT 1
            ",
            [file_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .ok();

    Ok(entry)
}

fn remove_file_and_orphaned_container(conn: &Connection, file_id: i64) -> Result<(), String> {
    let linked_container_id = conn
        .query_row(
            "SELECT container_id FROM file_containers WHERE file_id = ?",
            [file_id],
            |row| row.get::<_, i64>(0),
        )
        .ok();

    conn.execute("DELETE FROM files WHERE id = ?", [file_id])
        .map_err(|e| format!("failed to delete file record {file_id}: {e}"))?;

    if let Some(container_id) = linked_container_id {
        let file_link_count = conn
            .query_row(
                "SELECT COUNT(1) FROM file_containers WHERE container_id = ?",
                [container_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|e| format!("failed to count file links for container {container_id}: {e}"))?;
        let parent_link_count = conn
            .query_row(
                "SELECT COUNT(1) FROM container_children WHERE child_container_id = ?",
                [container_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|e| format!("failed to count parent links for container {container_id}: {e}"))?;
        let child_link_count = conn
            .query_row(
                "SELECT COUNT(1) FROM container_children WHERE parent_container_id = ?",
                [container_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|e| format!("failed to count child links for container {container_id}: {e}"))?;

        if file_link_count == 0 && parent_link_count == 0 && child_link_count == 0 {
            let _ = conn.execute("DELETE FROM containers WHERE id = ?", [container_id]);
        }
    }

    Ok(())
}

fn upsert_file_container(
    conn: &Connection,
    file_id: i64,
    filename: &str,
    source_path: &str,
) -> Result<ContainerUpsertOutcome, String> {
    let container_type = detect_container_type(filename);

    let linked_container_id = conn
        .query_row(
            "SELECT container_id FROM file_containers WHERE file_id = ?",
            [file_id],
            |row| row.get::<_, i64>(0),
        )
        .ok();

    if let Some(container_id) = linked_container_id {
        conn.execute(
            "
            UPDATE containers
            SET container_type = ?, display_name = ?, source_path = ?, updated_at = datetime('now')
            WHERE id = ?
            ",
            params![container_type, filename, source_path, container_id],
        )
        .map_err(|e| format!("failed to update container {container_id}: {e}"))?;

        return Ok(ContainerUpsertOutcome {
            created: false,
            container_id,
        });
    }

    conn.execute(
        "
        INSERT INTO containers(container_type, display_name, source_path)
        VALUES (?, ?, ?)
        ",
        params![container_type, filename, source_path],
    )
    .map_err(|e| format!("failed to insert container: {e}"))?;

    let container_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO file_containers(file_id, container_id) VALUES (?, ?)",
        params![file_id, container_id],
    )
    .map_err(|e| format!("failed to link file to container: {e}"))?;

    Ok(ContainerUpsertOutcome {
        created: true,
        container_id,
    })
}

fn upsert_group_container(
    conn: &Connection,
    container_type: &str,
    source_path: &str,
    display_name: &str,
) -> Result<(i64, bool), String> {
    let existing_id = conn
        .query_row(
            "SELECT id FROM containers WHERE container_type = ? AND source_path = ?",
            params![container_type, source_path],
            |row| row.get::<_, i64>(0),
        )
        .ok();

    if let Some(id) = existing_id {
        conn.execute(
            "
            UPDATE containers
            SET display_name = ?, updated_at = datetime('now')
            WHERE id = ?
            ",
            params![display_name, id],
        )
        .map_err(|e| format!("failed to update group container {id}: {e}"))?;
        return Ok((id, false));
    }

    conn.execute(
        "
        INSERT INTO containers(container_type, display_name, source_path)
        VALUES (?, ?, ?)
        ",
        params![container_type, display_name, source_path],
    )
    .map_err(|e| format!("failed to insert group container: {e}"))?;

    Ok((conn.last_insert_rowid(), true))
}

fn insert_combined_container_child_links(
    conn: &Connection,
    parent_container_id: i64,
    child_container_ids: &[i64],
) -> Result<(), String> {
    let mut seen: HashSet<i64> = HashSet::new();

    for child_id in child_container_ids {
        if *child_id <= 0 || *child_id == parent_container_id || !seen.insert(*child_id) {
            continue;
        }

        let child_exists = conn
            .query_row(
                "SELECT 1 FROM containers WHERE id = ?",
                [child_id],
                |_| Ok(()),
            )
            .is_ok();

        if !child_exists {
            return Err(format!("child container {child_id} does not exist"));
        }

        conn.execute(
            "
            INSERT OR IGNORE INTO container_children(parent_container_id, child_container_id)
            VALUES (?, ?)
            ",
            params![parent_container_id, child_id],
        )
        .map_err(|e| {
            format!(
                "failed to link combined container {parent_container_id} to child {child_id}: {e}"
            )
        })?;
    }

    Ok(())
}

fn rebuild_combined_container_thumbnail_slots(
    conn: &Connection,
    container_id: i64,
    slot_limit: u32,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM container_thumbnails WHERE container_id = ?",
        [container_id],
    )
    .map_err(|e| {
        format!("failed to clear previous thumbnails for combined container {container_id}: {e}")
    })?;

    let mut stmt = conn
        .prepare(
            "
            SELECT ct.thumbnail_path
            FROM container_children cc
            INNER JOIN containers c ON c.id = cc.child_container_id
            INNER JOIN container_thumbnails ct ON ct.container_id = cc.child_container_id
            WHERE cc.parent_container_id = ?
            ORDER BY c.updated_at DESC, c.id DESC, ct.slot_index ASC
            LIMIT ?
            ",
        )
        .map_err(|e| {
            format!(
                "failed to prepare thumbnail aggregation query for combined container {container_id}: {e}"
            )
        })?;

    let rows = stmt
        .query_map(params![container_id, slot_limit], |row| row.get::<_, String>(0))
        .map_err(|e| {
            format!(
                "failed to query thumbnail aggregation rows for combined container {container_id}: {e}"
            )
        })?;

    let mut slot_index: i64 = 0;
    for row in rows {
        let thumb_path = row.map_err(|e| {
            format!(
                "failed to map thumbnail aggregation row for combined container {container_id}: {e}"
            )
        })?;
        upsert_container_thumbnail(conn, container_id, slot_index, &thumb_path)?;
        slot_index += 1;
    }

    Ok(())
}

fn is_image_extension(ext: &str) -> bool {
    matches!(ext, "jpg" | "jpeg" | "jpe" | "png" | "gif" | "webp" | "avif" | "bmp")
}

fn is_video_extension(ext: &str) -> bool {
    matches!(
        ext,
        "mp4"
            | "mkv"
            | "avi"
            | "webm"
            | "3gp"
            | "asf"
            | "divx"
            | "flv"
            | "m2t"
            | "m2ts"
            | "m4v"
            | "mov"
            | "mpeg"
            | "mpg"
            | "ogm"
            | "rm"
            | "swf"
            | "ts"
            | "vg2"
            | "wmv"
    )
}

fn rebuild_image_group_containers(conn: &Connection, folder_path: &str) -> Result<GroupRebuildResult, String> {
    let root = Path::new(folder_path).to_path_buf();

    let mut stmt = conn
        .prepare(
            "
            SELECT c.id, f.path
            FROM containers c
            INNER JOIN file_containers fc ON fc.container_id = c.id
            INNER JOIN files f ON f.id = fc.file_id
            WHERE c.container_type = 'image'
              AND f.path LIKE ?
            ",
        )
        .map_err(|e| format!("failed to prepare image container query: {e}"))?;

    let prefix = format!("{}%", folder_path);
    let rows = stmt
        .query_map([prefix], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))
        .map_err(|e| format!("failed to query image containers: {e}"))?;

    let mut image_children_by_dir: HashMap<PathBuf, Vec<i64>> = HashMap::new();
    for row in rows {
        let (child_id, file_path) = row.map_err(|e| format!("failed to map image row: {e}"))?;
        let p = Path::new(&file_path);
        if let Some(parent) = p.parent() {
            image_children_by_dir
                .entry(parent.to_path_buf())
                .or_default()
                .push(child_id);
        }
    }

    if image_children_by_dir.is_empty() {
        return Ok(GroupRebuildResult { created: 0, updated: 0 });
    }

    let mut directory_has_subdirs: HashMap<PathBuf, bool> = HashMap::new();
    let mut dir_all_direct_files_are_images: HashMap<PathBuf, bool> = HashMap::new();

    let mut file_stmt = conn
        .prepare(
            "
            SELECT path
            FROM files
            WHERE path LIKE ?
            ",
        )
        .map_err(|e| format!("failed to prepare direct file query: {e}"))?;

    let file_rows = file_stmt
        .query_map([format!("{}%", folder_path)], |row| row.get::<_, String>(0))
        .map_err(|e| format!("failed to query direct files: {e}"))?;

    let mut direct_file_exts_by_dir: HashMap<PathBuf, Vec<String>> = HashMap::new();
    for row in file_rows {
        let path_text = row.map_err(|e| format!("failed to map file path row: {e}"))?;
        let p = Path::new(&path_text);
        if let Some(parent) = p.parent() {
            let ext = p
                .extension()
                .and_then(|v| v.to_str())
                .unwrap_or_default()
                .to_lowercase();
            direct_file_exts_by_dir
                .entry(parent.to_path_buf())
                .or_default()
                .push(ext);
        }
    }

    for (dir, exts) in &direct_file_exts_by_dir {
        let all_images = !exts.is_empty() && exts.iter().all(|ext| is_image_extension(ext));
        dir_all_direct_files_are_images.insert(dir.clone(), all_images);
    }

    let all_dirs: Vec<PathBuf> = image_children_by_dir.keys().cloned().collect();
    for dir in &all_dirs {
        let mut has_subdir = false;
        for candidate in &all_dirs {
            if candidate != dir && candidate.starts_with(dir) {
                let rel = candidate.strip_prefix(dir).ok();
                if let Some(r) = rel {
                    if r.components().count() >= 1 {
                        has_subdir = true;
                        break;
                    }
                }
            }
        }
        directory_has_subdirs.insert(dir.clone(), has_subdir);
    }

    let mut created = 0usize;
    let mut updated = 0usize;

    let mut managed_group_container_ids: HashSet<i64> = HashSet::new();

    for (dir, child_ids) in image_children_by_dir {
        if child_ids.is_empty() {
            continue;
        }

        if !dir.starts_with(&root) {
            continue;
        }

        let has_subdirs = directory_has_subdirs.get(&dir).copied().unwrap_or(false);
        let all_direct_files_are_images = dir_all_direct_files_are_images.get(&dir).copied().unwrap_or(false);

        let group_type = if !has_subdirs && all_direct_files_are_images {
            "image_folder"
        } else {
            "virtual_group"
        };

        let source_path = dir.to_string_lossy().to_string();
        let display_name = dir
            .file_name()
            .and_then(|v| v.to_str())
            .map(|v| v.to_string())
            .unwrap_or_else(|| source_path.clone());

        let (group_container_id, was_created) = upsert_group_container(conn, group_type, &source_path, &display_name)?;
        managed_group_container_ids.insert(group_container_id);
        if was_created {
            created += 1;
        } else {
            updated += 1;
        }

        conn.execute(
            "DELETE FROM container_children WHERE parent_container_id = ?",
            [group_container_id],
        )
        .map_err(|e| format!("failed to clear child links for group container {group_container_id}: {e}"))?;

        for child_id in child_ids {
            conn.execute(
                "
                INSERT OR IGNORE INTO container_children(parent_container_id, child_container_id)
                VALUES (?, ?)
                ",
                params![group_container_id, child_id],
            )
            .map_err(|e| format!("failed to insert group child link: {e}"))?;
        }
    }

    if !managed_group_container_ids.is_empty() {
        let mut stale_stmt = conn
            .prepare(
                "
                SELECT id
                FROM containers
                WHERE source_path LIKE ?
                  AND container_type IN ('image_folder', 'virtual_group')
                ",
            )
            .map_err(|e| format!("failed to prepare stale group query: {e}"))?;

        let stale_rows = stale_stmt
            .query_map([format!("{}%", folder_path)], |row| row.get::<_, i64>(0))
            .map_err(|e| format!("failed to query stale groups: {e}"))?;

        for row in stale_rows {
            let id = row.map_err(|e| format!("failed to map stale group row: {e}"))?;
            if !managed_group_container_ids.contains(&id) {
                conn.execute("DELETE FROM containers WHERE id = ?", [id])
                    .map_err(|e| format!("failed to delete stale group container {id}: {e}"))?;
            }
        }
    }

    Ok(GroupRebuildResult { created, updated })
}

fn rebuild_group_container_thumbnail_slots(
    conn: &Connection,
    folder_path: &str,
    max_slots: i64,
) -> Result<(), String> {
    if max_slots <= 0 {
        return Ok(());
    }

    let mut parents_stmt = conn
        .prepare(
            "
            SELECT id
            FROM containers
            WHERE source_path LIKE ?
              AND container_type IN ('image_folder', 'virtual_group')
            ",
        )
        .map_err(|e| format!("failed to prepare group parent query: {e}"))?;

    let parent_rows = parents_stmt
        .query_map([format!("{}%", folder_path)], |row| row.get::<_, i64>(0))
        .map_err(|e| format!("failed to query group parents: {e}"))?;

    let mut child_thumb_stmt = conn
        .prepare(
            "
            SELECT ct.thumbnail_path
            FROM container_children cc
            INNER JOIN container_thumbnails ct ON ct.container_id = cc.child_container_id
            WHERE cc.parent_container_id = ?
              AND ct.slot_index = 0
            ORDER BY cc.child_container_id ASC
            ",
        )
        .map_err(|e| format!("failed to prepare child thumbnail query: {e}"))?;

    for parent_row in parent_rows {
        let parent_id = parent_row.map_err(|e| format!("failed to map group parent row: {e}"))?;

        let child_thumb_rows = child_thumb_stmt
            .query_map([parent_id], |row| row.get::<_, String>(0))
            .map_err(|e| format!("failed to query child thumbnails for parent {parent_id}: {e}"))?;

        let mut thumb_paths = Vec::new();
        for row in child_thumb_rows {
            let path = row.map_err(|e| format!("failed to map child thumbnail row: {e}"))?;
            if !path.trim().is_empty() {
                thumb_paths.push(path);
            }
            if thumb_paths.len() as i64 >= max_slots {
                break;
            }
        }

        conn.execute(
            "DELETE FROM container_thumbnails WHERE container_id = ?",
            [parent_id],
        )
        .map_err(|e| format!("failed to clear group thumbnails for {parent_id}: {e}"))?;

        for (idx, path) in thumb_paths.into_iter().enumerate() {
            let slot = idx as i64;
            conn.execute(
                "
                INSERT INTO container_thumbnails(container_id, slot_index, thumbnail_path, created_at, updated_at)
                VALUES (?, ?, ?, datetime('now'), datetime('now'))
                ",
                params![parent_id, slot, path],
            )
            .map_err(|e| format!("failed to insert group thumbnail slot {slot} for {parent_id}: {e}"))?;
        }
    }

    Ok(())
}

fn hash_file<F>(path: &Path, cancel_flag: &Arc<AtomicBool>, mut on_progress: F) -> Result<String, String>
where
    F: FnMut(u64),
{
    let mut file = File::open(path).map_err(|e| format!("failed to open file {:?}: {e}", path))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1_048_576];
    let mut processed: u64 = 0;

    loop {
        if cancel_flag.load(Ordering::Relaxed) {
            return Err(REGISTER_CANCELLED.to_string());
        }
        let n = file
            .read(&mut buffer)
            .map_err(|e| format!("failed to read file {:?}: {e}", path))?;
        if n == 0 {
            break;
        }
        processed = processed.saturating_add(n as u64);
        on_progress(processed);
        hasher.update(&buffer[..n]);
    }

    Ok(hex::encode(hasher.finalize()))
}

fn emit_register_progress(app: &tauri::AppHandle, progress: &RegisterProgress) {
    let _ = app.emit("register-progress", progress.clone());
}

fn find_moved_file_candidate(
    conn: &Connection,
    hash: &str,
    size: i64,
    current_path: &str,
) -> Result<Option<i64>, String> {
    let mut stmt = conn
        .prepare(
            "
            SELECT id, path
            FROM files
            WHERE hash = ? AND size = ? AND path != ?
            ORDER BY updated_at DESC, id DESC
            ",
        )
        .map_err(|e| format!("failed to prepare moved-file candidate query: {e}"))?;

    let rows = stmt
        .query_map(params![hash, size, current_path], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| format!("failed to query moved-file candidates: {e}"))?;

    for row in rows {
        let (candidate_id, candidate_path) =
            row.map_err(|e| format!("failed to map moved-file candidate row: {e}"))?;
        if !Path::new(&candidate_path).exists() {
            return Ok(Some(candidate_id));
        }
    }

    Ok(None)
}

fn register_folder_blocking(
    app: &tauri::AppHandle,
    folder_path: &str,
    cancel_flag: Arc<AtomicBool>,
) -> Result<RegisterResult, String> {
    let path = Path::new(folder_path);
    if !path.exists() || !path.is_dir() {
        return Err("folder_path does not exist or is not a directory".to_string());
    }

    let conn = open_db(app)?;

    let file_entries: Vec<_> = WalkDir::new(path)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .collect();
    let total_files = file_entries.len();

    let mut result = RegisterResult {
        scanned_files: 0,
        inserted_files: 0,
        updated_paths: 0,
        skipped_files: 0,
        created_containers: 0,
        updated_containers: 0,
        created_group_containers: 0,
        updated_group_containers: 0,
        created_thumbnails: 0,
        updated_thumbnails: 0,
        queued_thumbnail_tasks: 0,
        background_job_id: None,
        canceled: false,
    };

    emit_register_progress(
        app,
        &RegisterProgress {
            folder_path: folder_path.to_string(),
            total_files,
            scanned_files: 0,
            inserted_files: 0,
            updated_paths: 0,
            skipped_files: 0,
            current_file_path: None,
            current_file_size_bytes: 0,
            current_file_processed_bytes: 0,
            canceled: false,
            done: false,
        },
    );

    for entry in file_entries {

        if cancel_flag.load(Ordering::Relaxed) {
            result.canceled = true;
            break;
        }

        result.scanned_files += 1;
        let file_path = entry.path();
        let metadata = match entry.metadata() {
            Ok(meta) => meta,
            Err(_) => {
                result.skipped_files += 1;
                continue;
            }
        };

        let size_i64 = match i64::try_from(metadata.len()) {
            Ok(v) => v,
            Err(_) => {
                result.skipped_files += 1;
                continue;
            }
        };

        let size_u64 = metadata.len();
        let current_file_path = file_path.to_string_lossy().to_string();

        emit_register_progress(
            app,
            &RegisterProgress {
                folder_path: folder_path.to_string(),
                total_files,
                scanned_files: result.scanned_files,
                inserted_files: result.inserted_files,
                updated_paths: result.updated_paths,
                skipped_files: result.skipped_files,
                current_file_path: Some(current_file_path.clone()),
                current_file_size_bytes: size_u64,
                current_file_processed_bytes: 0,
                canceled: false,
                done: false,
            },
        );

        let mut last_file_emit_bytes: u64 = 0;
        let hash = match hash_file(file_path, &cancel_flag, |processed_bytes| {
            // Emit every 1 MiB or at file completion to avoid event flooding.
            if processed_bytes.saturating_sub(last_file_emit_bytes) >= 1_048_576 || processed_bytes >= size_u64 {
                last_file_emit_bytes = processed_bytes;
                emit_register_progress(
                    app,
                    &RegisterProgress {
                        folder_path: folder_path.to_string(),
                        total_files,
                        scanned_files: result.scanned_files,
                        inserted_files: result.inserted_files,
                        updated_paths: result.updated_paths,
                        skipped_files: result.skipped_files,
                        current_file_path: Some(current_file_path.clone()),
                        current_file_size_bytes: size_u64,
                        current_file_processed_bytes: processed_bytes,
                        canceled: false,
                        done: false,
                    },
                );
            }
        }) {
            Ok(v) => v,
            Err(err) if err == REGISTER_CANCELLED => {
                result.canceled = true;
                break;
            }
            Err(_) => {
                result.skipped_files += 1;
                continue;
            }
        };

        let path_text = file_path.to_string_lossy().to_string();
        let filename = file_path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path_text.clone());

        let moved_file_id = find_moved_file_candidate(&conn, &hash, size_i64, &path_text)?;

        if let Some(moved_file_id) = moved_file_id {
            conn.execute(
                "
                UPDATE files
                SET path = ?, filename = ?, updated_at = datetime('now')
                WHERE id = ?
                ",
                params![path_text, filename, moved_file_id],
            )
            .map_err(|e| format!("failed to update moved file path for id {moved_file_id}: {e}"))?;

            result.updated_paths += 1;
            let outcome = upsert_file_container(&conn, moved_file_id, &filename, &path_text)?;
            if outcome.created {
                result.created_containers += 1;
            } else {
                result.updated_containers += 1;
            }

            if let Some(kind) = detect_thumbnailable_kind(&filename) {
                if kind == "video" {
                    let slots = generate_video_thumbnail_set(app, &path_text, &hash, size_i64, 16)?;
                    for (slot_index, thumb_path) in &slots {
                        let _ = upsert_container_thumbnail(&conn, outcome.container_id, *slot_index, thumb_path)?;
                    }
                    if let Some((_, primary)) = slots.first() {
                        if upsert_thumbnail(&conn, moved_file_id, primary)? {
                            result.updated_thumbnails += 1;
                        } else {
                            result.created_thumbnails += 1;
                        }
                    }
                } else if let Some(thumbnail_path) =
                    generate_thumbnail_for_file(app, &path_text, &hash, size_i64, kind)?
                {
                    if upsert_thumbnail(&conn, moved_file_id, &thumbnail_path)? {
                        result.updated_thumbnails += 1;
                    } else {
                        result.created_thumbnails += 1;
                    }
                    let _ = upsert_container_thumbnail(&conn, outcome.container_id, 0, &thumbnail_path)?;
                }
            } else if detect_container_type(&filename) == "archive" {
                let (slots, _) = generate_archive_thumbnail_set(app, &path_text, &hash, size_i64, 16)?;
                for (slot_index, thumb_path) in &slots {
                    let _ = upsert_container_thumbnail(&conn, outcome.container_id, *slot_index, thumb_path)?;
                }
                if let Some((_, primary)) = slots.first() {
                    if upsert_thumbnail(&conn, moved_file_id, primary)? {
                        result.updated_thumbnails += 1;
                    } else {
                        result.created_thumbnails += 1;
                    }
                }

                if let Err(err) = rebuild_archive_virtual_hierarchy(
                    app,
                    &conn,
                    outcome.container_id,
                    &path_text,
                    &hash,
                    size_i64,
                ) {
                    eprintln!(
                        "archive virtual hierarchy rebuild failed for {}: {}",
                        path_text, err
                    );
                }
            }

            continue;
        }

        let inserted = conn
            .execute(
                "
                INSERT OR IGNORE INTO files (hash, size, path, filename)
                VALUES (?, ?, ?, ?)
                ",
                params![hash, size_i64, path_text, filename],
            )
            .map_err(|e| format!("failed to insert file: {e}"))?;

        if inserted > 0 {
            result.inserted_files += 1;

            let inserted_file_id = conn
                .query_row("SELECT id FROM files WHERE path = ?", [path_text.as_str()], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(|e| format!("failed to resolve inserted file id: {e}"))?;
            let outcome = upsert_file_container(&conn, inserted_file_id, &filename, &path_text)?;
            if outcome.created {
                result.created_containers += 1;
            } else {
                result.updated_containers += 1;
            }

            if let Some(kind) = detect_thumbnailable_kind(&filename) {
                if kind == "video" {
                    let slots = generate_video_thumbnail_set(app, &path_text, &hash, size_i64, 16)?;
                    for (slot_index, thumb_path) in &slots {
                        let _ = upsert_container_thumbnail(&conn, outcome.container_id, *slot_index, thumb_path)?;
                    }
                    if let Some((_, primary)) = slots.first() {
                        if upsert_thumbnail(&conn, inserted_file_id, primary)? {
                            result.updated_thumbnails += 1;
                        } else {
                            result.created_thumbnails += 1;
                        }
                    }
                } else if let Some(thumbnail_path) =
                    generate_thumbnail_for_file(app, &path_text, &hash, size_i64, kind)?
                {
                    if upsert_thumbnail(&conn, inserted_file_id, &thumbnail_path)? {
                        result.updated_thumbnails += 1;
                    } else {
                        result.created_thumbnails += 1;
                    }
                    let _ = upsert_container_thumbnail(&conn, outcome.container_id, 0, &thumbnail_path)?;
                }
            } else if detect_container_type(&filename) == "archive" {
                let (slots, _) = generate_archive_thumbnail_set(app, &path_text, &hash, size_i64, 16)?;
                for (slot_index, thumb_path) in &slots {
                    let _ = upsert_container_thumbnail(&conn, outcome.container_id, *slot_index, thumb_path)?;
                }
                if let Some((_, primary)) = slots.first() {
                    if upsert_thumbnail(&conn, inserted_file_id, primary)? {
                        result.updated_thumbnails += 1;
                    } else {
                        result.created_thumbnails += 1;
                    }
                }

                if let Err(err) = rebuild_archive_virtual_hierarchy(
                    app,
                    &conn,
                    outcome.container_id,
                    &path_text,
                    &hash,
                    size_i64,
                ) {
                    eprintln!(
                        "archive virtual hierarchy rebuild failed for {}: {}",
                        path_text, err
                    );
                }
            }
        }

        if result.scanned_files % 25 == 0 || result.scanned_files == total_files {
            emit_register_progress(
                app,
                &RegisterProgress {
                    folder_path: folder_path.to_string(),
                    total_files,
                    scanned_files: result.scanned_files,
                    inserted_files: result.inserted_files,
                    updated_paths: result.updated_paths,
                    skipped_files: result.skipped_files,
                    current_file_path: Some(current_file_path.clone()),
                    current_file_size_bytes: size_u64,
                    current_file_processed_bytes: size_u64,
                    canceled: false,
                    done: false,
                },
            );
        }
    }

    emit_register_progress(
        app,
        &RegisterProgress {
            folder_path: folder_path.to_string(),
            total_files,
            scanned_files: result.scanned_files,
            inserted_files: result.inserted_files,
            updated_paths: result.updated_paths,
            skipped_files: result.skipped_files,
            current_file_path: None,
            current_file_size_bytes: 0,
            current_file_processed_bytes: 0,
            canceled: result.canceled,
            done: true,
        },
    );

    if !result.canceled {
        let grouping = rebuild_image_group_containers(&conn, folder_path)?;
        result.created_group_containers = grouping.created;
        result.updated_group_containers = grouping.updated;
        rebuild_group_container_thumbnail_slots(&conn, folder_path, 16)?;
    }

    Ok(result)
}

#[tauri::command]
fn get_archive_traversal_depth(app: tauri::AppHandle) -> Result<u32, String> {
    let conn = open_db(&app)?;
    Ok(archive_traversal_depth_for_connection(&conn)? as u32)
}

#[tauri::command]
fn set_archive_traversal_depth(app: tauri::AppHandle, depth: u32) -> Result<u32, String> {
    let conn = open_db(&app)?;
    Ok(set_archive_traversal_depth_for_connection(&conn, depth as usize)? as u32)
}

#[tauri::command]
fn list_recent_containers(
    app: tauri::AppHandle,
    limit: Option<u32>,
) -> Result<Vec<ContainerRecord>, String> {
    let conn = open_db(&app)?;
    let cap = limit.unwrap_or(25).min(250);

    let mut stmt = conn
        .prepare(
            "
                        SELECT
                            c.id,
                            c.container_type,
                            c.display_name,
                            c.source_path,
                            c.updated_at,
                            COALESCE(child_counts.child_count, 0) AS child_count,
                            CASE
                                WHEN c.container_type = 'combined' THEN COALESCE(ccm.include_children_in_search, 1)
                                ELSE NULL
                            END AS include_children_in_search
                        FROM containers AS c
                        LEFT JOIN (
                            SELECT parent_container_id, COUNT(*) AS child_count
                            FROM container_children
                            GROUP BY parent_container_id
                        ) AS child_counts ON child_counts.parent_container_id = c.id
                        LEFT JOIN combined_container_meta ccm ON ccm.container_id = c.id
                        LEFT JOIN (
                            SELECT container_id, COUNT(*) AS slot_count
                            FROM container_thumbnails
                            GROUP BY container_id
                        ) AS thumb_counts ON thumb_counts.container_id = c.id
                        WHERE c.container_type != 'archive_virtual'
                        ORDER BY
                            CASE
                                WHEN c.container_type = 'archive' AND COALESCE(thumb_counts.slot_count, 0) < 16 THEN 0
                                ELSE 1
                            END ASC,
                            c.updated_at DESC,
                            c.id DESC
            LIMIT ?
            ",
        )
        .map_err(|e| format!("failed to prepare container list query: {e}"))?;

    let rows = stmt
        .query_map([cap], |row| {
            Ok(ContainerRecord {
                id: row.get(0)?,
                container_type: row.get(1)?,
                display_name: row.get(2)?,
                source_path: row.get(3)?,
                updated_at: row.get(4)?,
                child_count: row.get(5)?,
                include_children_in_search: row.get::<_, Option<i64>>(6)?.map(|v| v != 0),
            })
        })
        .map_err(|e| format!("failed to query containers: {e}"))?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("failed to map container row: {e}"))?);
    }

    Ok(out)
}

#[tauri::command]
fn list_container_children(
    app: tauri::AppHandle,
    container_id: i64,
) -> Result<Vec<ContainerChildRecord>, String> {
    let conn = open_db(&app)?;

    let mut stmt = conn
        .prepare(
            "
            SELECT
                c.id,
                c.container_type,
                c.display_name,
                c.source_path,
                c.updated_at,
                COALESCE(child_counts.child_count, 0) AS child_count,
                CASE
                    WHEN c.container_type = 'combined' THEN COALESCE(ccm.include_children_in_search, 1)
                    ELSE NULL
                END AS include_children_in_search
            FROM container_children cc
            INNER JOIN containers c ON c.id = cc.child_container_id
            LEFT JOIN (
                SELECT parent_container_id, COUNT(*) AS child_count
                FROM container_children
                GROUP BY parent_container_id
            ) AS child_counts ON child_counts.parent_container_id = c.id
            LEFT JOIN combined_container_meta ccm ON ccm.container_id = c.id
            WHERE cc.parent_container_id = ?
            ORDER BY
                CASE WHEN c.container_type = 'archive_virtual' THEN 0 ELSE 1 END ASC,
                c.display_name ASC,
                c.updated_at DESC,
                c.id DESC
            ",
        )
        .map_err(|e| format!("failed to prepare child container query: {e}"))?;

    let rows = stmt
        .query_map([container_id], |row| {
            Ok(ContainerChildRecord {
                id: row.get(0)?,
                container_type: row.get(1)?,
                display_name: row.get(2)?,
                source_path: row.get(3)?,
                updated_at: row.get(4)?,
                child_count: row.get(5)?,
                include_children_in_search: row.get::<_, Option<i64>>(6)?.map(|v| v != 0),
            })
        })
        .map_err(|e| format!("failed to query child containers: {e}"))?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("failed to map child container row: {e}"))?);
    }

    Ok(out)
}

#[tauri::command]
fn list_container_thumbnails(
    app: tauri::AppHandle,
    container_id: i64,
    limit: Option<u32>,
) -> Result<Vec<ContainerThumbnailRecord>, String> {
    let conn = open_db(&app)?;
    let cap = limit.unwrap_or(16).min(64);

    let mut stmt = conn
        .prepare(
            "
            SELECT slot_index, thumbnail_path
            FROM container_thumbnails
            WHERE container_id = ?
            ORDER BY slot_index ASC
            LIMIT ?
            ",
        )
        .map_err(|e| format!("failed to prepare container thumbnails query: {e}"))?;

    let rows = stmt
        .query_map(params![container_id, cap], |row| {
            let path: String = row.get(1)?;
            Ok(ContainerThumbnailRecord {
                slot_index: row.get(0)?,
                thumbnail_data_url: thumbnail_data_url_from_path(&path),
                thumbnail_path: path,
            })
        })
        .map_err(|e| format!("failed to query container thumbnails: {e}"))?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("failed to map container thumbnail row: {e}"))?);
    }

    Ok(out)
}

#[tauri::command]
fn backfill_archive_container_thumbnails(
    app: tauri::AppHandle,
    container_id: i64,
) -> Result<ArchiveBackfillResult, String> {
    let conn = open_db(&app)?;

    let row = conn
        .query_row(
            "
            SELECT c.container_type, f.id, f.path, f.hash, f.size
            FROM containers c
            LEFT JOIN file_containers fc ON fc.container_id = c.id
            LEFT JOIN files f ON f.id = fc.file_id
            WHERE c.id = ?
            ",
            [container_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                ))
            },
        )
        .map_err(|e| format!("failed to resolve archive container {container_id}: {e}"))?;

    let (container_type, file_id_opt, file_path_opt, file_hash_opt, file_size_opt) = row;
    if container_type != "archive" {
        return Ok(ArchiveBackfillResult {
            container_id,
            generated_slots: 0,
            updated_slots: 0,
            updated_file_thumbnail: false,
            skipped_reason: Some("container is not archive type".to_string()),
        });
    }

    let (file_id, file_path, file_hash, file_size) = match (file_id_opt, file_path_opt, file_hash_opt, file_size_opt) {
        (Some(id), Some(path), Some(hash), Some(size)) => (id, path, hash, size),
        _ => {
            return Ok(ArchiveBackfillResult {
                container_id,
                generated_slots: 0,
                updated_slots: 0,
                updated_file_thumbnail: false,
                skipped_reason: Some("linked file metadata is missing".to_string()),
            })
        }
    };

    if !Path::new(&file_path).exists() {
        return Ok(ArchiveBackfillResult {
            container_id,
            generated_slots: 0,
            updated_slots: 0,
            updated_file_thumbnail: false,
            skipped_reason: Some("archive source file does not exist".to_string()),
        });
    }

    let (slots, generation_reason) =
        generate_archive_thumbnail_set(&app, &file_path, &file_hash, file_size, 16)?;
    if slots.is_empty() {
        return Ok(ArchiveBackfillResult {
            container_id,
            generated_slots: 0,
            updated_slots: 0,
            updated_file_thumbnail: false,
            skipped_reason: Some(
                generation_reason.unwrap_or_else(|| "no thumbnails generated".to_string()),
            ),
        });
    }

    let mut updated_slots = 0usize;
    for (slot_index, thumb_path) in &slots {
        if upsert_container_thumbnail(&conn, container_id, *slot_index, thumb_path)? {
            updated_slots += 1;
        }
    }

    let mut updated_file_thumbnail = false;
    if let Some((_, first_thumb)) = slots.first() {
        updated_file_thumbnail = upsert_thumbnail(&conn, file_id, first_thumb)?;
    }

    if updated_slots > 0 {
        conn.execute(
            "UPDATE containers SET updated_at = datetime('now') WHERE id = ?",
            [container_id],
        )
        .map_err(|e| format!("failed to update archive container timestamp: {e}"))?;
    }

    Ok(ArchiveBackfillResult {
        container_id,
        generated_slots: slots.len(),
        updated_slots,
        updated_file_thumbnail,
        skipped_reason: None,
    })
}

#[tauri::command]
fn inspect_thumbnail_cache(app: tauri::AppHandle) -> Result<ThumbnailMaintenanceResult, String> {
    let conn = open_db(&app)?;
    let cache_files = list_thumbnail_cache_files(&app)?;
    build_thumbnail_maintenance_result(&conn, &cache_files, true, 0, 0, 0)
}

#[tauri::command]
fn cleanup_thumbnail_cache(app: tauri::AppHandle) -> Result<ThumbnailMaintenanceResult, String> {
    let mut conn = open_db(&app)?;
    let cache_files = list_thumbnail_cache_files(&app)?;
    let missing_file_thumbnail_ids = collect_missing_file_thumbnail_record_ids(&conn)?;
    let missing_container_thumbnail_keys = collect_missing_container_thumbnail_keys(&conn)?;
    let referenced_paths = collect_referenced_thumbnail_paths(&conn)?;

    let orphaned_cache_files: Vec<PathBuf> = cache_files
        .iter()
        .filter(|path| !referenced_paths.contains(*path))
        .cloned()
        .collect();

    let tx = conn
        .transaction()
        .map_err(|e| format!("failed to start thumbnail maintenance transaction: {e}"))?;

    for file_id in &missing_file_thumbnail_ids {
        tx.execute("DELETE FROM thumbnails WHERE file_id = ?", [file_id])
            .map_err(|e| format!("failed to delete missing file thumbnail record {file_id}: {e}"))?;
    }

    for (container_id, slot_index) in &missing_container_thumbnail_keys {
        tx.execute(
            "DELETE FROM container_thumbnails WHERE container_id = ? AND slot_index = ?",
            params![container_id, slot_index],
        )
        .map_err(|e| {
            format!(
                "failed to delete missing container thumbnail record ({container_id}, {slot_index}): {e}"
            )
        })?;
    }

    tx.commit()
        .map_err(|e| format!("failed to commit thumbnail maintenance transaction: {e}"))?;

    let mut removed_orphaned_cache_files = 0usize;
    for path in &orphaned_cache_files {
        std::fs::remove_file(path)
            .map_err(|e| format!("failed to remove orphaned thumbnail {:?}: {e}", path))?;
        removed_orphaned_cache_files += 1;
    }

    let refreshed_cache_files = list_thumbnail_cache_files(&app)?;
    build_thumbnail_maintenance_result(
        &conn,
        &refreshed_cache_files,
        false,
        missing_file_thumbnail_ids.len(),
        missing_container_thumbnail_keys.len(),
        removed_orphaned_cache_files,
    )
}

#[tauri::command]
async fn register_folder(
    app: tauri::AppHandle,
    register_control: tauri::State<'_, RegisterControl>,
    folder_path: String,
) -> Result<RegisterResult, String> {
    let cancel_flag = Arc::new(AtomicBool::new(false));
    {
        let mut guard = register_control
            .cancel_flag
            .lock()
            .map_err(|_| "failed to lock register control".to_string())?;
        *guard = Some(cancel_flag.clone());
    }

    let app_handle = app.clone();
    let join_result = tauri::async_runtime::spawn_blocking(move || {
        register_folder_blocking(&app_handle, &folder_path, cancel_flag)
    })
        .await
        .map_err(|e| format!("registration task failed to join: {e}"));

    {
        let mut guard = register_control
            .cancel_flag
            .lock()
            .map_err(|_| "failed to lock register control".to_string())?;
        *guard = None;
    }

    join_result?
}

#[tauri::command]
fn cancel_register(register_control: tauri::State<'_, RegisterControl>) -> Result<bool, String> {
    let guard = register_control
        .cancel_flag
        .lock()
        .map_err(|_| "failed to lock register control".to_string())?;

    if let Some(flag) = &*guard {
        flag.store(true, Ordering::Relaxed);
        return Ok(true);
    }

    Ok(false)
}

#[tauri::command]
fn list_recent_files(app: tauri::AppHandle, limit: Option<u32>) -> Result<Vec<FileRecord>, String> {
    let conn = open_db(&app)?;
    let cap = limit.unwrap_or(20).min(200);

    let mut stmt = conn
        .prepare(
            "
            SELECT f.id, f.path, f.filename, f.hash, f.size, f.created_at, t.thumbnail_path
            FROM files f
            LEFT JOIN thumbnails t ON t.file_id = f.id
            ORDER BY f.updated_at DESC, f.id DESC
            LIMIT ?
            ",
        )
        .map_err(|e| format!("failed to prepare list query: {e}"))?;

    let rows = stmt
        .query_map([cap], |row| {
            Ok(FileRecord {
                id: row.get(0)?,
                path: row.get(1)?,
                filename: row.get(2)?,
                hash: row.get(3)?,
                size: row.get(4)?,
                created_at: row.get(5)?,
                thumbnail_path: row.get(6)?,
                thumbnail_data_url: row
                    .get::<_, Option<String>>(6)?
                    .and_then(|p| thumbnail_data_url_from_path(&p)),
            })
        })
        .map_err(|e| format!("failed to query files: {e}"))?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("failed to map row: {e}"))?);
    }
    Ok(out)
}

#[tauri::command]
fn search_files(
    app: tauri::AppHandle,
    path_query: Option<String>,
    filename_query: Option<String>,
    tag_query: Option<String>,
    min_rating: Option<i64>,
    limit: Option<u32>,
) -> Result<Vec<FileRecord>, String> {
    let conn = open_db(&app)?;
    let cap = limit.unwrap_or(100).min(500);

    let path_pattern = path_query
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{s}%"));
    let filename_pattern = filename_query
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{s}%"));
        let tag_value = tag_query
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());

    let mut stmt = conn
        .prepare(
            "
                        SELECT f.id, f.path, f.filename, f.hash, f.size, f.created_at, t.thumbnail_path
                        FROM files f
                        LEFT JOIN file_ratings fr ON fr.file_id = f.id
                        LEFT JOIN thumbnails t ON t.file_id = f.id
                        WHERE (?1 IS NULL OR f.path LIKE ?1)
                            AND (?2 IS NULL OR f.filename LIKE ?2)
                            AND (
                                ?3 IS NULL OR EXISTS (
                                    SELECT 1
                                    FROM file_tags ft
                                    INNER JOIN tags t ON t.id = ft.tag_id
                                    WHERE ft.file_id = f.id
                                        AND t.name = ?3
                                )
                            )
                            AND (?4 IS NULL OR fr.rating >= ?4)
                        ORDER BY f.updated_at DESC, f.id DESC
                        LIMIT ?5
            ",
        )
        .map_err(|e| format!("failed to prepare search query: {e}"))?;

    let rows = stmt
                .query_map(params![path_pattern, filename_pattern, tag_value, min_rating, cap], |row| {
            Ok(FileRecord {
                id: row.get(0)?,
                path: row.get(1)?,
                filename: row.get(2)?,
                hash: row.get(3)?,
                size: row.get(4)?,
                created_at: row.get(5)?,
                thumbnail_path: row.get(6)?,
                thumbnail_data_url: row
                    .get::<_, Option<String>>(6)?
                    .and_then(|p| thumbnail_data_url_from_path(&p)),
            })
        })
        .map_err(|e| format!("failed to query search results: {e}"))?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("failed to map search row: {e}"))?);
    }
    Ok(out)
}

#[tauri::command]
fn list_duplicate_groups(
    app: tauri::AppHandle,
    limit_groups: Option<u32>,
    per_group_limit: Option<u32>,
) -> Result<Vec<DuplicateGroupRecord>, String> {
    let conn = open_db(&app)?;
    let group_cap = limit_groups.unwrap_or(50).max(1).min(200);
    let file_cap = per_group_limit.unwrap_or(25).max(2).min(200);

    let mut group_stmt = conn
        .prepare(
            "
            SELECT hash, size, COUNT(1) AS file_count
            FROM files
            GROUP BY hash, size
            HAVING COUNT(1) > 1
            ORDER BY file_count DESC, MAX(updated_at) DESC, hash ASC
            LIMIT ?
            ",
        )
        .map_err(|e| format!("failed to prepare duplicate group query: {e}"))?;

    let group_rows = group_stmt
        .query_map([group_cap], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|e| format!("failed to query duplicate groups: {e}"))?;

    let mut groups = Vec::new();

    for group_row in group_rows {
        let (hash, size, file_count) =
            group_row.map_err(|e| format!("failed to map duplicate group row: {e}"))?;

        let mut file_stmt = conn
            .prepare(
                "
                SELECT
                    f.id,
                    f.path,
                    f.filename,
                    f.hash,
                    f.size,
                    f.created_at,
                    t.thumbnail_path,
                    fr.rating,
                    COALESCE((
                        SELECT group_concat(tag_name, ',')
                        FROM (
                            SELECT tg.name AS tag_name
                            FROM file_tags ft
                            INNER JOIN tags tg ON tg.id = ft.tag_id
                            WHERE ft.file_id = f.id
                            ORDER BY tg.name ASC
                        )
                    ), '') AS tags_csv
                FROM files f
                LEFT JOIN thumbnails t ON t.file_id = f.id
                LEFT JOIN file_ratings fr ON fr.file_id = f.id
                WHERE f.hash = ?1 AND f.size = ?2
                ORDER BY f.updated_at DESC, f.id DESC
                LIMIT ?3
                ",
            )
            .map_err(|e| format!("failed to prepare duplicate file query: {e}"))?;

        let file_rows = file_stmt
            .query_map(params![hash, size, file_cap], |row| {
                let thumbnail_path: Option<String> = row.get(6)?;
                let tags_csv: String = row.get(8)?;
                Ok(DuplicateFileRecord {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    filename: row.get(2)?,
                    hash: row.get(3)?,
                    size: row.get(4)?,
                    created_at: row.get(5)?,
                    thumbnail_path: thumbnail_path.clone(),
                    thumbnail_data_url: thumbnail_path
                        .as_deref()
                        .and_then(thumbnail_data_url_from_path),
                    tags: split_tags_csv(&tags_csv),
                    rating: row.get(7)?,
                })
            })
            .map_err(|e| format!("failed to query duplicate files for hash group: {e}"))?;

        let mut files = Vec::new();
        for file_row in file_rows {
            files.push(file_row.map_err(|e| format!("failed to map duplicate file row: {e}"))?);
        }

        groups.push(DuplicateGroupRecord {
            hash,
            size,
            file_count,
            files,
        });
    }

    Ok(groups)
}

fn update_file_path_and_related_containers(
    app: &tauri::AppHandle,
    conn: &Connection,
    file_id: i64,
    hash: &str,
    size: i64,
    target_path: &str,
) -> Result<(), String> {
    let filename = Path::new(target_path)
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or_else(|| format!("failed to derive filename from destination path: {target_path}"))?
        .to_string();

    conn.execute(
        "UPDATE files SET path = ?, filename = ?, updated_at = datetime('now') WHERE id = ?",
        params![target_path, filename, file_id],
    )
    .map_err(|e| format!("failed to update moved file record {file_id}: {e}"))?;

    let upsert_outcome = upsert_file_container(conn, file_id, &filename, target_path)?;

    let container_type = conn
        .query_row(
            "SELECT container_type FROM containers WHERE id = ?",
            [upsert_outcome.container_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(|e| {
            format!(
                "failed to resolve container type for moved file {file_id} container {}: {e}",
                upsert_outcome.container_id
            )
        })?;

    if container_type == "archive" {
        rebuild_archive_virtual_hierarchy(
            app,
            conn,
            upsert_outcome.container_id,
            target_path,
            hash,
            size,
        )?;
    }

    Ok(())
}

#[tauri::command]
fn move_duplicate_file_to_directory(
    app: tauri::AppHandle,
    file_id: i64,
    target_directory: String,
) -> Result<DuplicateActionResult, String> {
    let conn = open_db(&app)?;
    let (source_path, filename, hash, size) = duplicate_identity_for_file(&conn, file_id)?;

    if !file_has_duplicate_peer(&conn, &hash, size, file_id)? {
        return Err(format!(
            "file {file_id} no longer has duplicate peers; refresh duplicate candidates"
        ));
    }

    let source = PathBuf::from(&source_path);
    if !source.exists() {
        return Err(format!(
            "source file no longer exists on disk: {}",
            source.to_string_lossy()
        ));
    }

    let target_dir = PathBuf::from(target_directory.trim());
    if target_directory.trim().is_empty() {
        return Err("target directory is required".to_string());
    }

    let destination = unique_destination_path(&target_dir, &filename);
    move_file_path(&source, &destination)?;

    let destination_text = destination.to_string_lossy().to_string();
    update_file_path_and_related_containers(&app, &conn, file_id, &hash, size, &destination_text)?;

    Ok(DuplicateActionResult {
        file_id,
        previous_path: source_path,
        current_path: destination_text,
        action: "move".to_string(),
    })
}

#[tauri::command]
fn quarantine_duplicate_file(app: tauri::AppHandle, file_id: i64) -> Result<DuplicateActionResult, String> {
    let conn = open_db(&app)?;
    if active_quarantine_entry(&conn, file_id)?.is_some() {
        return Err(format!("file {file_id} is already quarantined; use Undo or Purge first"));
    }

    let (source_path, filename, hash, size) = duplicate_identity_for_file(&conn, file_id)?;

    if !file_has_duplicate_peer(&conn, &hash, size, file_id)? {
        return Err(format!(
            "file {file_id} no longer has duplicate peers; refresh duplicate candidates"
        ));
    }

    let source = PathBuf::from(&source_path);
    if !source.exists() {
        return Err(format!(
            "source file no longer exists on disk: {}",
            source.to_string_lossy()
        ));
    }

    if path_is_quarantine_path(&source) {
        return Err(format!("file {file_id} is already in a quarantine directory"));
    }

    let quarantine_dir = quarantine_directory_for_source(&app, &source)?;
    std::fs::create_dir_all(&quarantine_dir)
        .map_err(|e| format!("failed to create duplicate quarantine directory: {e}"))?;

    let quarantined_name = format!("{}_{}", file_id, filename);
    let destination = unique_destination_path(&quarantine_dir, &quarantined_name);
    move_file_path(&source, &destination)?;

    let destination_text = destination.to_string_lossy().to_string();
    update_file_path_and_related_containers(&app, &conn, file_id, &hash, size, &destination_text)?;
    mark_existing_active_quarantine_history_resolved(&conn, file_id)?;
    conn.execute(
        "
        INSERT INTO duplicate_quarantine_history(file_id, original_path, quarantine_path, quarantined_at)
        VALUES (?, ?, ?, datetime('now'))
        ",
        params![file_id, source_path, destination_text],
    )
    .map_err(|e| format!("failed to record quarantine history for file {file_id}: {e}"))?;

    Ok(DuplicateActionResult {
        file_id,
        previous_path: source_path,
        current_path: destination_text,
        action: "quarantine".to_string(),
    })
}

#[tauri::command]
fn list_quarantined_duplicates(app: tauri::AppHandle, limit: Option<u32>) -> Result<Vec<QuarantinedDuplicateRecord>, String> {
    let conn = open_db(&app)?;
    let cap = limit.unwrap_or(200).min(1000);

    let mut stmt = conn
        .prepare(
            "
            SELECT
                q.file_id,
                f.filename,
                f.hash,
                f.size,
                q.original_path,
                q.quarantine_path,
                q.quarantined_at,
                t.thumbnail_path
            FROM duplicate_quarantine_history q
            INNER JOIN files f ON f.id = q.file_id
            LEFT JOIN thumbnails t ON t.file_id = q.file_id
            WHERE q.restored_at IS NULL AND q.purged_at IS NULL
            ORDER BY q.id DESC
            LIMIT ?
            ",
        )
        .map_err(|e| format!("failed to prepare quarantined duplicates query: {e}"))?;

    let rows = stmt
        .query_map([cap], |row| {
            let thumb_path: Option<String> = row.get(7)?;
            Ok(QuarantinedDuplicateRecord {
                file_id: row.get(0)?,
                filename: row.get(1)?,
                hash: row.get(2)?,
                size: row.get(3)?,
                original_path: row.get(4)?,
                quarantine_path: row.get(5)?,
                quarantined_at: row.get(6)?,
                thumbnail_data_url: thumb_path.as_deref().and_then(thumbnail_data_url_from_path),
            })
        })
        .map_err(|e| format!("failed to query quarantined duplicates: {e}"))?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("failed to map quarantined duplicate row: {e}"))?);
    }
    Ok(out)
}

#[tauri::command]
fn undo_quarantined_duplicate(app: tauri::AppHandle, file_id: i64) -> Result<DuplicateActionResult, String> {
    let conn = open_db(&app)?;
    let Some((history_id, original_path, _, _)) = active_quarantine_entry(&conn, file_id)? else {
        return Err(format!("file {file_id} has no active quarantine history"));
    };

    let (current_path, filename, hash, size) = duplicate_identity_for_file(&conn, file_id)?;
    let source = PathBuf::from(&current_path);
    if !source.exists() {
        return Err(format!("quarantined file is missing on disk: {}", source.to_string_lossy()));
    }
    let restore_target = PathBuf::from(&original_path);
    if restore_target.exists() {
        return Err(format!("cannot undo quarantine because target already exists: {original_path}"));
    }

    move_file_path(&source, &restore_target)?;
    cleanup_quarantine_parent_if_empty(&source)?;
    update_file_path_and_related_containers(
        &app,
        &conn,
        file_id,
        &hash,
        size,
        &restore_target.to_string_lossy(),
    )?;
    conn.execute(
        "UPDATE duplicate_quarantine_history SET restored_at = datetime('now') WHERE id = ?",
        [history_id],
    )
    .map_err(|e| format!("failed to mark quarantine history restored for file {file_id}: {e}"))?;

    Ok(DuplicateActionResult {
        file_id,
        previous_path: current_path,
        current_path: restore_target.to_string_lossy().to_string(),
        action: format!("undo_quarantine:{filename}"),
    })
}

#[tauri::command]
fn purge_quarantined_duplicate(app: tauri::AppHandle, file_id: i64) -> Result<DuplicateActionResult, String> {
    let conn = open_db(&app)?;
    let Some((history_id, _, quarantine_path, _)) = active_quarantine_entry(&conn, file_id)? else {
        return Err(format!("file {file_id} has no active quarantine history"));
    };

    let (current_path, _, _, _) = duplicate_identity_for_file(&conn, file_id)?;
    if current_path != quarantine_path {
        return Err(format!(
            "file {file_id} is no longer at the active quarantine path; current path: {current_path}"
        ));
    }

    let current = PathBuf::from(&current_path);
    if current.exists() {
        std::fs::remove_file(&current)
            .map_err(|e| format!("failed to purge quarantined file {}: {e}", current.to_string_lossy()))?;
    }
    cleanup_quarantine_parent_if_empty(&current)?;

    remove_file_and_orphaned_container(&conn, file_id)?;
    conn.execute(
        "UPDATE duplicate_quarantine_history SET purged_at = datetime('now') WHERE id = ?",
        [history_id],
    )
    .map_err(|e| format!("failed to mark quarantine history purged for file {file_id}: {e}"))?;

    Ok(DuplicateActionResult {
        file_id,
        previous_path: current_path,
        current_path: "(purged)".to_string(),
        action: "purge".to_string(),
    })
}

#[tauri::command]
fn get_duplicate_overview(app: tauri::AppHandle) -> Result<DuplicateOverview, String> {
    let conn = open_db(&app)?;

    let total_files = conn
        .query_row("SELECT COUNT(1) FROM files", [], |row| row.get::<_, i64>(0))
        .map_err(|e| format!("failed to count files: {e}"))?;

    let duplicate_groups = conn
        .query_row(
            "
            SELECT COUNT(1)
            FROM (
                SELECT 1
                FROM files
                GROUP BY hash, size
                HAVING COUNT(1) > 1
            )
            ",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| format!("failed to count duplicate groups: {e}"))?;

    let duplicate_files = conn
        .query_row(
            "
            SELECT COALESCE(SUM(group_count), 0)
            FROM (
                SELECT COUNT(1) AS group_count
                FROM files
                GROUP BY hash, size
                HAVING COUNT(1) > 1
            )
            ",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| format!("failed to count duplicate files: {e}"))?;

    let quarantined_files = conn
        .query_row(
            "
            SELECT COUNT(1)
            FROM duplicate_quarantine_history
            WHERE restored_at IS NULL AND purged_at IS NULL
            ",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| format!("failed to count quarantined files: {e}"))?;

    Ok(DuplicateOverview {
        total_files,
        duplicate_groups,
        duplicate_files,
        quarantined_files,
    })
}

#[tauri::command]
fn purge_all_quarantined_duplicates(app: tauri::AppHandle) -> Result<BulkPurgeResult, String> {
    let conn = open_db(&app)?;

    let mut stmt = conn
        .prepare(
            "
            SELECT file_id
            FROM duplicate_quarantine_history
            WHERE restored_at IS NULL AND purged_at IS NULL
            ORDER BY id DESC
            ",
        )
        .map_err(|e| format!("failed to prepare active quarantine query: {e}"))?;

    let rows = stmt
        .query_map([], |row| row.get::<_, i64>(0))
        .map_err(|e| format!("failed to query active quarantine rows: {e}"))?;

    let mut file_ids: Vec<i64> = Vec::new();
    for row in rows {
        file_ids.push(row.map_err(|e| format!("failed to map active quarantine row: {e}"))?);
    }

    let requested = file_ids.len();
    let mut purged = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for file_id in file_ids {
        match purge_quarantined_duplicate(app.clone(), file_id) {
            Ok(_) => {
                purged += 1;
            }
            Err(err) => failures.push(format!("file {file_id}: {err}")),
        }
    }

    Ok(BulkPurgeResult {
        requested,
        purged,
        failed: failures.len(),
        failures,
    })
}

#[tauri::command]
fn search_containers(
    app: tauri::AppHandle,
    path_query: Option<String>,
    name_query: Option<String>,
    include_archive_virtual: Option<bool>,
    respect_combined_child_visibility: Option<bool>,
    limit: Option<u32>,
) -> Result<Vec<ContainerRecord>, String> {
    let conn = open_db(&app)?;
    let cap = limit.unwrap_or(100).min(500);
    let include_virtual = include_archive_virtual.unwrap_or(true);
    let respect_child_visibility = respect_combined_child_visibility.unwrap_or(true);

    let path_pattern = path_query
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{s}%"));
    let name_pattern = name_query
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{s}%"));

    let mut stmt = conn
        .prepare(
            "
            SELECT
                c.id,
                c.container_type,
                c.display_name,
                c.source_path,
                c.updated_at,
                                COALESCE(child_counts.child_count, 0) AS child_count,
                                CASE
                                        WHEN c.container_type = 'combined' THEN COALESCE(ccm.include_children_in_search, 1)
                                        ELSE NULL
                                END AS include_children_in_search
            FROM containers AS c
            LEFT JOIN (
                SELECT parent_container_id, COUNT(*) AS child_count
                FROM container_children
                GROUP BY parent_container_id
            ) AS child_counts ON child_counts.parent_container_id = c.id
                        LEFT JOIN combined_container_meta ccm ON ccm.container_id = c.id
            WHERE (?1 IS NULL OR c.source_path LIKE ?1)
              AND (?2 IS NULL OR c.display_name LIKE ?2)
              AND (?3 = 1 OR c.container_type != 'archive_virtual')
              AND (
                  ?4 = 0 OR NOT EXISTS (
                      SELECT 1
                      FROM container_children cc_hidden
                      INNER JOIN containers parent_c ON parent_c.id = cc_hidden.parent_container_id
                      LEFT JOIN combined_container_meta hidden_meta ON hidden_meta.container_id = parent_c.id
                      WHERE cc_hidden.child_container_id = c.id
                        AND parent_c.container_type = 'combined'
                        AND COALESCE(hidden_meta.include_children_in_search, 1) = 0
                  )
              )
            ORDER BY c.updated_at DESC, c.id DESC
            LIMIT ?5
            ",
        )
        .map_err(|e| format!("failed to prepare container search query: {e}"))?;

    let rows = stmt
        .query_map(
            params![
                path_pattern,
                name_pattern,
                include_virtual,
                respect_child_visibility,
                cap
            ],
            |row| {
            Ok(ContainerRecord {
                id: row.get(0)?,
                container_type: row.get(1)?,
                display_name: row.get(2)?,
                source_path: row.get(3)?,
                updated_at: row.get(4)?,
                child_count: row.get(5)?,
                include_children_in_search: row.get::<_, Option<i64>>(6)?.map(|v| v != 0),
            })
        },
        )
        .map_err(|e| format!("failed to query container search results: {e}"))?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("failed to map container search row: {e}"))?);
    }
    Ok(out)
}

#[tauri::command]
fn list_containers_for_combining(
    app: tauri::AppHandle,
    query: Option<String>,
    include_archive_virtual: Option<bool>,
    limit: Option<u32>,
) -> Result<Vec<ContainerRecord>, String> {
    let conn = open_db(&app)?;
    let cap = limit.unwrap_or(300).min(1000);
    let include_virtual = include_archive_virtual.unwrap_or(false);

    let pattern = query
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{s}%"));

    let mut stmt = conn
        .prepare(
            "
            SELECT
                c.id,
                c.container_type,
                c.display_name,
                c.source_path,
                c.updated_at,
                COALESCE(child_counts.child_count, 0) AS child_count,
                CASE
                    WHEN c.container_type = 'combined' THEN COALESCE(ccm.include_children_in_search, 1)
                    ELSE NULL
                END AS include_children_in_search
            FROM containers c
            LEFT JOIN (
                SELECT parent_container_id, COUNT(*) AS child_count
                FROM container_children
                GROUP BY parent_container_id
            ) AS child_counts ON child_counts.parent_container_id = c.id
            LEFT JOIN combined_container_meta ccm ON ccm.container_id = c.id
            WHERE (?1 IS NULL OR c.display_name LIKE ?1 OR c.source_path LIKE ?1)
              AND (?2 = 1 OR c.container_type != 'archive_virtual')
            ORDER BY c.updated_at DESC, c.id DESC
            LIMIT ?3
            ",
        )
        .map_err(|e| format!("failed to prepare combine candidate query: {e}"))?;

    let rows = stmt
        .query_map(params![pattern, include_virtual, cap], |row| {
            Ok(ContainerRecord {
                id: row.get(0)?,
                container_type: row.get(1)?,
                display_name: row.get(2)?,
                source_path: row.get(3)?,
                updated_at: row.get(4)?,
                child_count: row.get(5)?,
                include_children_in_search: row.get::<_, Option<i64>>(6)?.map(|v| v != 0),
            })
        })
        .map_err(|e| format!("failed to query combine candidates: {e}"))?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("failed to map combine candidate row: {e}"))?);
    }

    Ok(out)
}

#[tauri::command]
fn create_combined_container(
    app: tauri::AppHandle,
    display_name: String,
    child_container_ids: Vec<i64>,
    include_children_in_search: Option<bool>,
) -> Result<ContainerRecord, String> {
    let mut conn = open_db(&app)?;
    let trimmed_name = display_name.trim();
    if trimmed_name.is_empty() {
        return Err("combined container name is required".to_string());
    }

    let include_children = include_children_in_search.unwrap_or(true);
    let source_path = format!(
        "combined://{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| format!("failed to generate combined container source path: {e}"))?
            .as_nanos()
    );

    let tx = conn
        .transaction()
        .map_err(|e| format!("failed to start create combined transaction: {e}"))?;

    tx.execute(
        "
        INSERT INTO containers(container_type, display_name, source_path)
        VALUES ('combined', ?, ?)
        ",
        params![trimmed_name, source_path],
    )
    .map_err(|e| format!("failed to insert combined container: {e}"))?;

    let container_id = tx.last_insert_rowid();

    tx.execute(
        "
        INSERT INTO combined_container_meta(container_id, include_children_in_search, created_at, updated_at)
        VALUES (?, ?, datetime('now'), datetime('now'))
        ",
        params![container_id, if include_children { 1 } else { 0 }],
    )
    .map_err(|e| format!("failed to insert combined container metadata: {e}"))?;

    insert_combined_container_child_links(&tx, container_id, &child_container_ids)?;

    tx.commit()
        .map_err(|e| format!("failed to commit combined container creation: {e}"))?;

    rebuild_combined_container_thumbnail_slots(&conn, container_id, 16)?;

    conn.query_row(
        "
        SELECT
            c.id,
            c.container_type,
            c.display_name,
            c.source_path,
            c.updated_at,
            COALESCE(child_counts.child_count, 0) AS child_count,
            CASE
                WHEN c.container_type = 'combined' THEN COALESCE(ccm.include_children_in_search, 1)
                ELSE NULL
            END AS include_children_in_search
        FROM containers c
        LEFT JOIN (
            SELECT parent_container_id, COUNT(*) AS child_count
            FROM container_children
            GROUP BY parent_container_id
        ) AS child_counts ON child_counts.parent_container_id = c.id
        LEFT JOIN combined_container_meta ccm ON ccm.container_id = c.id
        WHERE c.id = ?
        ",
        [container_id],
        |row| {
            Ok(ContainerRecord {
                id: row.get(0)?,
                container_type: row.get(1)?,
                display_name: row.get(2)?,
                source_path: row.get(3)?,
                updated_at: row.get(4)?,
                child_count: row.get(5)?,
                include_children_in_search: row.get::<_, Option<i64>>(6)?.map(|v| v != 0),
            })
        },
    )
    .map_err(|e| format!("failed to load created combined container: {e}"))
}

#[tauri::command]
fn update_combined_container(
    app: tauri::AppHandle,
    container_id: i64,
    display_name: String,
    child_container_ids: Vec<i64>,
    include_children_in_search: Option<bool>,
) -> Result<ContainerRecord, String> {
    let mut conn = open_db(&app)?;
    let trimmed_name = display_name.trim();
    if trimmed_name.is_empty() {
        return Err("combined container name is required".to_string());
    }

    let container_type = conn
        .query_row(
            "SELECT container_type FROM containers WHERE id = ?",
            [container_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(|e| format!("failed to resolve combined container {container_id}: {e}"))?;
    if container_type != "combined" {
        return Err(format!("container {container_id} is not a combined container"));
    }

    let include_children = include_children_in_search.unwrap_or(true);

    let tx = conn
        .transaction()
        .map_err(|e| format!("failed to start update combined transaction: {e}"))?;

    tx.execute(
        "
        UPDATE containers
        SET display_name = ?, updated_at = datetime('now')
        WHERE id = ?
        ",
        params![trimmed_name, container_id],
    )
    .map_err(|e| format!("failed to update combined container {container_id}: {e}"))?;

    tx.execute(
        "
        INSERT INTO combined_container_meta(container_id, include_children_in_search, created_at, updated_at)
        VALUES (?, ?, datetime('now'), datetime('now'))
        ON CONFLICT(container_id) DO UPDATE SET
            include_children_in_search = excluded.include_children_in_search,
            updated_at = datetime('now')
        ",
        params![container_id, if include_children { 1 } else { 0 }],
    )
    .map_err(|e| format!("failed to update combined container metadata for {container_id}: {e}"))?;

    tx.execute(
        "DELETE FROM container_children WHERE parent_container_id = ?",
        [container_id],
    )
    .map_err(|e| format!("failed to clear combined container child links for {container_id}: {e}"))?;

    insert_combined_container_child_links(&tx, container_id, &child_container_ids)?;

    tx.commit()
        .map_err(|e| format!("failed to commit combined container update for {container_id}: {e}"))?;

    rebuild_combined_container_thumbnail_slots(&conn, container_id, 16)?;

    conn.query_row(
        "
        SELECT
            c.id,
            c.container_type,
            c.display_name,
            c.source_path,
            c.updated_at,
            COALESCE(child_counts.child_count, 0) AS child_count,
            CASE
                WHEN c.container_type = 'combined' THEN COALESCE(ccm.include_children_in_search, 1)
                ELSE NULL
            END AS include_children_in_search
        FROM containers c
        LEFT JOIN (
            SELECT parent_container_id, COUNT(*) AS child_count
            FROM container_children
            GROUP BY parent_container_id
        ) AS child_counts ON child_counts.parent_container_id = c.id
        LEFT JOIN combined_container_meta ccm ON ccm.container_id = c.id
        WHERE c.id = ?
        ",
        [container_id],
        |row| {
            Ok(ContainerRecord {
                id: row.get(0)?,
                container_type: row.get(1)?,
                display_name: row.get(2)?,
                source_path: row.get(3)?,
                updated_at: row.get(4)?,
                child_count: row.get(5)?,
                include_children_in_search: row.get::<_, Option<i64>>(6)?.map(|v| v != 0),
            })
        },
    )
    .map_err(|e| format!("failed to load updated combined container {container_id}: {e}"))
}

#[tauri::command]
fn get_combined_container_detail(
    app: tauri::AppHandle,
    container_id: i64,
) -> Result<CombinedContainerDetail, String> {
    let conn = open_db(&app)?;

    let (display_name, container_type, include_children) = conn
        .query_row(
            "
            SELECT
                c.display_name,
                c.container_type,
                COALESCE(ccm.include_children_in_search, 1)
            FROM containers c
            LEFT JOIN combined_container_meta ccm ON ccm.container_id = c.id
            WHERE c.id = ?
            ",
            [container_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .map_err(|e| format!("failed to load combined container detail for {container_id}: {e}"))?;

    if container_type != "combined" {
        return Err(format!("container {container_id} is not a combined container"));
    }

    let mut child_stmt = conn
        .prepare(
            "
            SELECT child_container_id
            FROM container_children
            WHERE parent_container_id = ?
            ORDER BY child_container_id ASC
            ",
        )
        .map_err(|e| format!("failed to prepare combined container child query: {e}"))?;
    let child_rows = child_stmt
        .query_map([container_id], |row| row.get::<_, i64>(0))
        .map_err(|e| format!("failed to query combined container children: {e}"))?;

    let mut child_container_ids = Vec::new();
    for row in child_rows {
        child_container_ids
            .push(row.map_err(|e| format!("failed to map combined container child row: {e}"))?);
    }

    Ok(CombinedContainerDetail {
        container_id,
        display_name,
        include_children_in_search: include_children != 0,
        child_container_ids,
    })
}

#[tauri::command]
fn delete_combined_container(app: tauri::AppHandle, container_id: i64) -> Result<String, String> {
    let conn = open_db(&app)?;

    let container_type = conn
        .query_row(
            "SELECT container_type FROM containers WHERE id = ?",
            [container_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(|e| format!("failed to resolve combined container {container_id}: {e}"))?;

    if container_type != "combined" {
        return Err(format!("container {container_id} is not a combined container"));
    }

    let deleted = conn
        .execute("DELETE FROM containers WHERE id = ?", [container_id])
        .map_err(|e| format!("failed to delete combined container {container_id}: {e}"))?;

    if deleted == 0 {
        return Err(format!("combined container {container_id} was not deleted"));
    }

    Ok(format!("combined container {container_id} deleted"))
}

#[tauri::command]
fn get_file_classification(
    app: tauri::AppHandle,
    file_id: i64,
) -> Result<FileClassification, String> {
    let conn = open_db(&app)?;

    let mut tags_stmt = conn
        .prepare(
            "
            SELECT t.name
            FROM file_tags ft
            INNER JOIN tags t ON t.id = ft.tag_id
            WHERE ft.file_id = ?
            ORDER BY t.name ASC
            ",
        )
        .map_err(|e| format!("failed to prepare tags query: {e}"))?;

    let tags_rows = tags_stmt
        .query_map([file_id], |row| row.get::<_, String>(0))
        .map_err(|e| format!("failed to query tags: {e}"))?;

    let mut tags = Vec::new();
    for row in tags_rows {
        tags.push(row.map_err(|e| format!("failed to map tag row: {e}"))?);
    }

    let rating = conn
        .query_row(
            "SELECT rating FROM file_ratings WHERE file_id = ?",
            [file_id],
            |row| row.get::<_, i64>(0),
        )
        .ok();

    Ok(FileClassification { tags, rating })
}

#[tauri::command]
fn save_file_classification(
    app: tauri::AppHandle,
    file_id: i64,
    tags: Vec<String>,
    rating: Option<i64>,
) -> Result<String, String> {
    if let Some(value) = rating {
        if !(1..=5).contains(&value) {
            return Err("rating must be in range 1..=5".to_string());
        }
    }

    let mut conn = open_db(&app)?;
    conn.execute("PRAGMA foreign_keys = ON", [])
        .map_err(|e| format!("failed to enable foreign keys: {e}"))?;

    let tx = conn
        .transaction()
        .map_err(|e| format!("failed to start transaction: {e}"))?;

    tx.execute("DELETE FROM file_tags WHERE file_id = ?", [file_id])
        .map_err(|e| format!("failed to clear existing tags: {e}"))?;

    let mut normalized_tags = Vec::new();
    for tag in tags {
        let trimmed = tag.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !normalized_tags.iter().any(|existing: &String| existing == trimmed) {
            normalized_tags.push(trimmed.to_string());
        }
    }

    for tag in normalized_tags {
        tx.execute("INSERT OR IGNORE INTO tags(name) VALUES (?)", [tag.as_str()])
            .map_err(|e| format!("failed to insert tag '{tag}': {e}"))?;

        let tag_id = tx
            .query_row("SELECT id FROM tags WHERE name = ?", [tag.as_str()], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(|e| format!("failed to resolve tag id for '{tag}': {e}"))?;

        tx.execute(
            "INSERT OR IGNORE INTO file_tags(file_id, tag_id) VALUES (?, ?)",
            params![file_id, tag_id],
        )
        .map_err(|e| format!("failed to link tag '{tag}' to file: {e}"))?;
    }

    match rating {
        Some(value) => {
            tx.execute(
                "
                INSERT INTO file_ratings(file_id, rating, updated_at)
                VALUES (?, ?, datetime('now'))
                ON CONFLICT(file_id) DO UPDATE SET
                  rating = excluded.rating,
                  updated_at = datetime('now')
                ",
                params![file_id, value],
            )
            .map_err(|e| format!("failed to upsert rating: {e}"))?;
        }
        None => {
            tx.execute("DELETE FROM file_ratings WHERE file_id = ?", [file_id])
                .map_err(|e| format!("failed to clear rating: {e}"))?;
        }
    }

    tx.commit()
        .map_err(|e| format!("failed to commit classification update: {e}"))?;

    Ok("classification saved".to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(RegisterControl::default())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            init_database,
            register_folder,
            cancel_register,
            list_recent_files,
            get_archive_traversal_depth,
            set_archive_traversal_depth,
            list_recent_containers,
            list_container_children,
            list_container_thumbnails,
            backfill_archive_container_thumbnails,
            inspect_thumbnail_cache,
            cleanup_thumbnail_cache,
            search_files,
            list_duplicate_groups,
            get_duplicate_overview,
            list_quarantined_duplicates,
            move_duplicate_file_to_directory,
            quarantine_duplicate_file,
            undo_quarantined_duplicate,
            purge_quarantined_duplicate,
            purge_all_quarantined_duplicates,
            search_containers,
            list_containers_for_combining,
            create_combined_container,
            update_combined_container,
            get_combined_container_detail,
            delete_combined_container,
            get_file_classification,
            save_file_classification
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::{
        archive_traversal_depth_for_connection, build_non_leaf_aggregate_slots,
        collect_media_and_nested_archives_in_dir, file_has_duplicate_peer,
        detect_container_type,
        is_archive_container_extension,
        path_is_quarantine_path,
        normalize_extension_from_path,
        split_tags_csv,
        scan_archive_extracted_images, set_archive_traversal_depth_for_connection,
    };
    use rusqlite::Connection;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn create_file(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("failed to create parent directory for test file");
        }
        fs::write(path, b"x").expect("failed to create test file");
    }

    #[test]
    fn scan_archive_images_detects_nested_variants() {
        let base = std::env::temp_dir().join(format!(
            "thumbscontainer_test_nested_{}",
            std::process::id()
        ));
        if base.exists() {
            let _ = fs::remove_dir_all(&base);
        }
        fs::create_dir_all(&base).expect("failed to create test temp dir");

        let img_a = base.join("folder/subfolder1/image_a.JPG");
        let img_b = base.join("folder/subfolder2/image_b.jPeG");
        let img_c = base.join("folder/subfolder3/image_c.jfif");
        let txt = base.join("folder/readme.txt");
        let no_ext = base.join("folder/subfolder4/cover");

        create_file(&img_a);
        create_file(&img_b);
        create_file(&img_c);
        create_file(&txt);
        create_file(&no_ext);

        let scan = scan_archive_extracted_images(&base).expect("scan should succeed");

        assert_eq!(scan.extracted_file_count, 5);
        assert_eq!(scan.image_candidates.len(), 3);

        let mut found: Vec<PathBuf> = scan.image_candidates;
        found.sort();
        assert!(found.contains(&img_a));
        assert!(found.contains(&img_b));
        assert!(found.contains(&img_c));

        fs::remove_dir_all(&base).expect("failed to clean up test temp dir");
    }

    #[test]
    fn normalize_extension_trims_and_lowercases() {
        let path = Path::new("C:/tmp/FILE.JpEg ");
        assert_eq!(normalize_extension_from_path(path), "jpeg");
    }

    #[test]
    fn collect_media_and_nested_archives_orders_and_classifies_entries() {
        let base = std::env::temp_dir().join(format!(
            "thumbscontainer_test_virtual_nested_{}",
            std::process::id()
        ));
        if base.exists() {
            let _ = fs::remove_dir_all(&base);
        }
        fs::create_dir_all(&base).expect("failed to create test temp dir");

        let img = base.join("z_dir/frame01.JPG");
        let video = base.join("a_dir/clip01.MKV");
        let nested = base.join("m_dir/inner_pack.zip");
        let other = base.join("readme.txt");

        create_file(&img);
        create_file(&video);
        create_file(&nested);
        create_file(&other);

        let (media, nested_archives) = collect_media_and_nested_archives_in_dir(&base, Some("outer/archive.zip"));

        assert_eq!(media.len(), 2);
        assert_eq!(nested_archives.len(), 1);

        assert_eq!(media[0].relative_path, "outer/archive.zip/a_dir/clip01.MKV");
        assert_eq!(media[0].media_kind, "video");
        assert_eq!(media[1].relative_path, "outer/archive.zip/z_dir/frame01.JPG");
        assert_eq!(media[1].media_kind, "image");

        assert_eq!(
            nested_archives[0].archive_logical_path,
            "outer/archive.zip/m_dir/inner_pack.zip"
        );

        fs::remove_dir_all(&base).expect("failed to clean up test temp dir");
    }

    #[test]
    fn non_leaf_aggregate_distributes_slots_evenly_when_children_are_fewer_than_slots() {
        let child_slots = vec![
            vec!["a0".to_string(), "a1".to_string()],
            vec!["b0".to_string()],
            vec!["c0".to_string(), "c1".to_string(), "c2".to_string()],
        ];

        let slots = build_non_leaf_aggregate_slots(&child_slots, 8);
        assert_eq!(slots.len(), 8);

        // Deterministic round-robin distribution: 3,3,2 allocation across children.
        let a_count = slots.iter().filter(|v| v.starts_with('a')).count();
        let b_count = slots.iter().filter(|v| v.starts_with('b')).count();
        let c_count = slots.iter().filter(|v| v.starts_with('c')).count();
        assert_eq!(a_count, 3);
        assert_eq!(b_count, 3);
        assert_eq!(c_count, 2);
    }

    #[test]
    fn archive_traversal_depth_is_persisted_and_clamped() {
        let conn = Connection::open_in_memory().expect("in-memory db should open");
        conn.execute_batch(
            "CREATE TABLE app_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .expect("app_settings table should be creatable");

        assert_eq!(archive_traversal_depth_for_connection(&conn).unwrap(), 2);

        set_archive_traversal_depth_for_connection(&conn, 5).unwrap();
        assert_eq!(archive_traversal_depth_for_connection(&conn).unwrap(), 5);

        set_archive_traversal_depth_for_connection(&conn, 20).unwrap();
        assert_eq!(archive_traversal_depth_for_connection(&conn).unwrap(), 10);

        set_archive_traversal_depth_for_connection(&conn, 0).unwrap();
        assert_eq!(archive_traversal_depth_for_connection(&conn).unwrap(), 0);
    }

    #[test]
    fn split_tags_csv_ignores_empty_entries() {
        let tags = split_tags_csv("alpha, beta ,, ,gamma");
        assert_eq!(tags, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn duplicate_peer_detection_requires_another_file() {
        let conn = Connection::open_in_memory().expect("in-memory db should open");
        conn.execute_batch(
            "
            CREATE TABLE files (
              id INTEGER PRIMARY KEY,
              hash TEXT NOT NULL,
              size INTEGER NOT NULL,
              path TEXT NOT NULL UNIQUE,
              filename TEXT NOT NULL,
              created_at TEXT NOT NULL DEFAULT (datetime('now')),
              updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            ",
        )
        .expect("files table should be creatable");

        conn.execute(
            "INSERT INTO files(id, hash, size, path, filename) VALUES (1, 'h', 10, 'a', 'a')",
            [],
        )
        .expect("insert should succeed");
        assert!(!file_has_duplicate_peer(&conn, "h", 10, 1).unwrap());

        conn.execute(
            "INSERT INTO files(id, hash, size, path, filename) VALUES (2, 'h', 10, 'b', 'b')",
            [],
        )
        .expect("second insert should succeed");
        assert!(file_has_duplicate_peer(&conn, "h", 10, 1).unwrap());
    }

    #[test]
    fn detect_container_type_covers_added_extensions() {
        assert_eq!(detect_container_type("video.m2ts"), "video");
        assert_eq!(detect_container_type("video.wmv"), "video");
        assert_eq!(detect_container_type("image.bmp"), "image");
        assert_eq!(detect_container_type("image.jpe"), "image");
        assert_eq!(detect_container_type("doc.pdf"), "archive");
        assert_eq!(detect_container_type("disk.iso"), "archive");
    }

    #[test]
    fn archive_container_extension_covers_iso_and_pdf() {
        assert!(is_archive_container_extension("iso"));
        assert!(is_archive_container_extension("pdf"));
    }

    #[test]
    fn quarantine_path_detection_handles_known_folder_names() {
        assert!(path_is_quarantine_path(Path::new("C:/x/.thumbscontainer_quarantine/file.mp4")));
        assert!(path_is_quarantine_path(Path::new("C:/x/duplicate_quarantine/file.mp4")));
        assert!(!path_is_quarantine_path(Path::new("C:/x/media/file.mp4")));
    }
}
