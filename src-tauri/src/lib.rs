use rusqlite::{params, Connection};
use serde::Serialize;
use sha2::{Digest, Sha256};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager};
use walkdir::WalkDir;

const REGISTER_CANCELLED: &str = "register_cancelled";
const THUMBNAIL_JOB_PROGRESS_EVENT: &str = "thumbnail-job-progress";
const DEFAULT_BG_FAIL_MARKER: &str = "__fail_bg__";

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

#[derive(Serialize, Clone)]
struct ThumbnailJobProgress {
    job_id: u64,
    folder_path: String,
    total_tasks: usize,
    completed_tasks: usize,
    succeeded_tasks: usize,
    failed_tasks: usize,
    current_item: Option<String>,
    last_error: Option<String>,
    done: bool,
}

#[derive(Clone)]
enum ThumbnailTaskKind {
    Image,
    Video,
    Archive,
}

#[derive(Clone)]
struct ThumbnailFileTask {
    file_id: i64,
    container_id: i64,
    file_path: String,
    file_hash: String,
    file_size: i64,
    kind: ThumbnailTaskKind,
}

#[derive(Clone)]
enum BackgroundThumbnailTask {
    Generate(ThumbnailFileTask),
    RebuildGroups {
        folder_path: String,
        max_slots: i64,
    },
}

struct ThumbnailJob {
    id: u64,
    folder_path: String,
    tasks: Vec<BackgroundThumbnailTask>,
}

#[derive(Default)]
struct ThumbnailJobQueueInner {
    jobs: Mutex<VecDeque<ThumbnailJob>>,
    worker_running: AtomicBool,
    next_job_id: AtomicU64,
}

#[derive(Clone, Default)]
struct ThumbnailJobQueue {
    inner: Arc<ThumbnailJobQueueInner>,
}

struct RegisterBlockingOutcome {
    result: RegisterResult,
    thumbnail_tasks: Vec<BackgroundThumbnailTask>,
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
struct ContainerRecord {
    id: i64,
    container_type: String,
    display_name: String,
    source_path: String,
    updated_at: String,
    child_count: i64,
}

#[derive(Serialize)]
struct ContainerChildRecord {
    id: i64,
    container_type: String,
    display_name: String,
    source_path: String,
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

    Ok("database ready".to_string())
}

fn detect_container_type(filename: &str) -> String {
    let lower = filename.to_lowercase();
    let extension = lower.rsplit('.').next().unwrap_or_default();

    match extension {
        "mp4" | "mkv" | "avi" => "video".to_string(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" => "image".to_string(),
        "rar" | "7z" | "zip" | "lzh" | "cbz" | "cbr" | "cb7" => "archive".to_string(),
        _ => "other".to_string(),
    }
}

fn detect_thumbnailable_kind(filename: &str) -> Option<&'static str> {
    let lower = filename.to_lowercase();
    let extension = lower.rsplit('.').next().unwrap_or_default();

    match extension {
        "mp4" | "mkv" | "avi" => Some("video"),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" => Some("image"),
        _ => None,
    }
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
            "jpe" | "jfif" | "bmp" | "dib" | "tif" | "tiff" | "heic" | "heif"
        )
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

fn is_image_extension(ext: &str) -> bool {
    matches!(ext, "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif")
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
    let mut buffer = [0u8; 8192];
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

fn emit_thumbnail_job_progress(app: &tauri::AppHandle, progress: &ThumbnailJobProgress) {
    let _ = app.emit(THUMBNAIL_JOB_PROGRESS_EVENT, progress.clone());
}

impl ThumbnailJobQueue {
    fn enqueue_job(
        &self,
        folder_path: String,
        tasks: Vec<BackgroundThumbnailTask>,
    ) -> Result<Option<u64>, String> {
        if tasks.is_empty() {
            return Ok(None);
        }

        let id = self
            .inner
            .next_job_id
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);

        let mut jobs = self
            .inner
            .jobs
            .lock()
            .map_err(|_| "failed to lock thumbnail job queue".to_string())?;
        jobs.push_back(ThumbnailJob {
            id,
            folder_path,
            tasks,
        });
        Ok(Some(id))
    }

    fn pop_job(&self) -> Option<ThumbnailJob> {
        let mut jobs = self.inner.jobs.lock().ok()?;
        jobs.pop_front()
    }

    fn ensure_worker(&self, app: tauri::AppHandle) {
        if self
            .inner
            .worker_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }

        let queue = self.clone();
        tauri::async_runtime::spawn(async move {
            let app_handle = app.clone();
            let queue_for_worker = queue.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || {
                while let Some(job) = queue_for_worker.pop_job() {
                    process_thumbnail_job(&app_handle, job);
                }
            })
            .await;

            queue.inner.worker_running.store(false, Ordering::SeqCst);

            if let Some(job) = queue.pop_job() {
                if let Ok(mut jobs) = queue.inner.jobs.lock() {
                    jobs.push_front(job);
                }
                queue.ensure_worker(app);
            }
        });
    }
}

fn enqueue_thumbnail_task(
    tasks: &mut Vec<BackgroundThumbnailTask>,
    file_id: i64,
    container_id: i64,
    path_text: &str,
    hash: &str,
    size_i64: i64,
    filename: &str,
) {
    let kind = if let Some(v) = detect_thumbnailable_kind(filename) {
        if v == "video" {
            Some(ThumbnailTaskKind::Video)
        } else {
            Some(ThumbnailTaskKind::Image)
        }
    } else if detect_container_type(filename) == "archive" {
        Some(ThumbnailTaskKind::Archive)
    } else {
        None
    };

    if let Some(kind) = kind {
        tasks.push(BackgroundThumbnailTask::Generate(ThumbnailFileTask {
            file_id,
            container_id,
            file_path: path_text.to_string(),
            file_hash: hash.to_string(),
            file_size: size_i64,
            kind,
        }));
    }
}

fn background_fail_marker_for_debug() -> Option<String> {
    if !cfg!(debug_assertions) {
        return None;
    }

    let marker = std::env::var("THUMBS_BG_FAIL_MARKER")
        .unwrap_or_else(|_| DEFAULT_BG_FAIL_MARKER.to_string())
        .trim()
        .to_string();

    if marker.is_empty() {
        None
    } else {
        Some(marker)
    }
}

fn injected_background_task_error(file_path: &str) -> Option<String> {
    let marker = background_fail_marker_for_debug()?;
    let path_lower = file_path.to_ascii_lowercase();
    let marker_lower = marker.to_ascii_lowercase();
    if path_lower.contains(&marker_lower) {
        Some(format!(
            "injected background thumbnail failure for testing (marker: {marker})"
        ))
    } else {
        None
    }
}

fn process_thumbnail_file_task(
    app: &tauri::AppHandle,
    conn: &Connection,
    task: &ThumbnailFileTask,
) -> Result<(), String> {
    if let Some(err) = injected_background_task_error(&task.file_path) {
        return Err(err);
    }

    if !Path::new(&task.file_path).exists() {
        return Err("source file does not exist".to_string());
    }

    match task.kind {
        ThumbnailTaskKind::Video => {
            let slots = generate_video_thumbnail_set(
                app,
                &task.file_path,
                &task.file_hash,
                task.file_size,
                16,
            )?;
            if slots.is_empty() {
                return Err("no video thumbnails generated".to_string());
            }
            for (slot_index, thumb_path) in &slots {
                let _ = upsert_container_thumbnail(conn, task.container_id, *slot_index, thumb_path)?;
            }
            if let Some((_, primary)) = slots.first() {
                let _ = upsert_thumbnail(conn, task.file_id, primary)?;
            }
        }
        ThumbnailTaskKind::Image => {
            if let Some(thumbnail_path) =
                generate_thumbnail_for_file(app, &task.file_path, &task.file_hash, task.file_size, "image")?
            {
                let _ = upsert_thumbnail(conn, task.file_id, &thumbnail_path)?;
                let _ = upsert_container_thumbnail(conn, task.container_id, 0, &thumbnail_path)?;
            } else {
                return Err("no image thumbnail generated".to_string());
            }
        }
        ThumbnailTaskKind::Archive => {
            let (slots, reason) = generate_archive_thumbnail_set(
                app,
                &task.file_path,
                &task.file_hash,
                task.file_size,
                16,
            )?;
            if slots.is_empty() {
                return Err(reason.unwrap_or_else(|| "no archive thumbnails generated".to_string()));
            }
            for (slot_index, thumb_path) in &slots {
                let _ = upsert_container_thumbnail(conn, task.container_id, *slot_index, thumb_path)?;
            }
            if let Some((_, primary)) = slots.first() {
                let _ = upsert_thumbnail(conn, task.file_id, primary)?;
            }
        }
    }

    conn.execute(
        "UPDATE containers SET updated_at = datetime('now') WHERE id = ?",
        [task.container_id],
    )
    .map_err(|e| format!("failed to update container timestamp for {}: {e}", task.container_id))?;

    Ok(())
}

fn process_thumbnail_job(app: &tauri::AppHandle, job: ThumbnailJob) {
    let total_tasks = job.tasks.len();
    let mut completed_tasks = 0usize;
    let mut succeeded_tasks = 0usize;
    let mut failed_tasks = 0usize;
    let mut last_error_message: Option<String> = None;

    let conn = match open_db(app) {
        Ok(conn) => conn,
        Err(err) => {
            emit_thumbnail_job_progress(
                app,
                &ThumbnailJobProgress {
                    job_id: job.id,
                    folder_path: job.folder_path,
                    total_tasks,
                    completed_tasks: total_tasks,
                    succeeded_tasks: 0,
                    failed_tasks: total_tasks,
                    current_item: None,
                    last_error: Some(err),
                    done: true,
                },
            );
            return;
        }
    };

    emit_thumbnail_job_progress(
        app,
        &ThumbnailJobProgress {
            job_id: job.id,
            folder_path: job.folder_path.clone(),
            total_tasks,
            completed_tasks,
            succeeded_tasks,
            failed_tasks,
            current_item: None,
            last_error: None,
            done: false,
        },
    );

    for task in job.tasks {
        let current_item = match &task {
            BackgroundThumbnailTask::Generate(file_task) => Some(file_task.file_path.clone()),
            BackgroundThumbnailTask::RebuildGroups { folder_path, .. } => {
                Some(format!("rebuild_group_thumbnails:{folder_path}"))
            }
        };

        let task_result = match task {
            BackgroundThumbnailTask::Generate(file_task) => {
                process_thumbnail_file_task(app, &conn, &file_task)
            }
            BackgroundThumbnailTask::RebuildGroups {
                folder_path,
                max_slots,
            } => rebuild_group_container_thumbnail_slots(&conn, &folder_path, max_slots),
        };

        completed_tasks += 1;
        if let Err(err) = task_result {
            failed_tasks += 1;
            last_error_message = Some(err);
        } else {
            succeeded_tasks += 1;
        }

        emit_thumbnail_job_progress(
            app,
            &ThumbnailJobProgress {
                job_id: job.id,
                folder_path: job.folder_path.clone(),
                total_tasks,
                completed_tasks,
                succeeded_tasks,
                failed_tasks,
                current_item,
                last_error: last_error_message.clone(),
                done: completed_tasks >= total_tasks,
            },
        );
    }
}

fn register_folder_blocking(
    app: &tauri::AppHandle,
    folder_path: &str,
    cancel_flag: Arc<AtomicBool>,
) -> Result<RegisterBlockingOutcome, String> {
    let path = Path::new(folder_path);
    if !path.exists() || !path.is_dir() {
        return Err("folder_path does not exist or is not a directory".to_string());
    }

    let conn = open_db(app)?;

    let total_files = WalkDir::new(path)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .count();

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

    let mut thumbnail_tasks: Vec<BackgroundThumbnailTask> = Vec::new();

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

    for entry in WalkDir::new(path).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }

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

        let updated = conn
            .execute(
                "
                UPDATE files
                SET path = ?, filename = ?, updated_at = datetime('now')
                WHERE hash = ? AND size = ? AND path != ?
                ",
                params![path_text, filename, hash, size_i64, path_text],
            )
            .map_err(|e| format!("failed to update moved file path: {e}"))?;

        if updated > 0 {
            result.updated_paths += updated as usize;

            let moved_file_id = conn
                .query_row(
                    "SELECT id FROM files WHERE hash = ? AND size = ? AND path = ?",
                    params![hash, size_i64, path_text],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|e| format!("failed to resolve moved file id: {e}"))?;
            let outcome = upsert_file_container(&conn, moved_file_id, &filename, &path_text)?;
            if outcome.created {
                result.created_containers += 1;
            } else {
                result.updated_containers += 1;
            }

            enqueue_thumbnail_task(
                &mut thumbnail_tasks,
                moved_file_id,
                outcome.container_id,
                &path_text,
                &hash,
                size_i64,
                &filename,
            );

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

            enqueue_thumbnail_task(
                &mut thumbnail_tasks,
                inserted_file_id,
                outcome.container_id,
                &path_text,
                &hash,
                size_i64,
                &filename,
            );
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
        thumbnail_tasks.push(BackgroundThumbnailTask::RebuildGroups {
            folder_path: folder_path.to_string(),
            max_slots: 16,
        });
    }

    result.queued_thumbnail_tasks = thumbnail_tasks.len();

    Ok(RegisterBlockingOutcome {
        result,
        thumbnail_tasks,
    })
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
                            COALESCE(child_counts.child_count, 0) AS child_count
                        FROM containers AS c
                        LEFT JOIN (
                            SELECT parent_container_id, COUNT(*) AS child_count
                            FROM container_children
                            GROUP BY parent_container_id
                        ) AS child_counts ON child_counts.parent_container_id = c.id
                        LEFT JOIN (
                            SELECT container_id, COUNT(*) AS slot_count
                            FROM container_thumbnails
                            GROUP BY container_id
                        ) AS thumb_counts ON thumb_counts.container_id = c.id
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
            SELECT c.id, c.container_type, c.display_name, c.source_path
            FROM container_children cc
            INNER JOIN containers c ON c.id = cc.child_container_id
            WHERE cc.parent_container_id = ?
            ORDER BY c.updated_at DESC, c.id DESC
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
    thumbnail_jobs: tauri::State<'_, ThumbnailJobQueue>,
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
    let register_folder_path = folder_path.clone();
    let join_result = tauri::async_runtime::spawn_blocking(move || {
        register_folder_blocking(&app_handle, &register_folder_path, cancel_flag)
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

    let mut outcome = join_result??;

    if !outcome.result.canceled {
        let job_id = thumbnail_jobs.enqueue_job(folder_path, outcome.thumbnail_tasks)?;
        outcome.result.background_job_id = job_id;
        if job_id.is_some() {
            thumbnail_jobs.ensure_worker(app);
        }
    }

    Ok(outcome.result)
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
fn search_containers(
    app: tauri::AppHandle,
    path_query: Option<String>,
    name_query: Option<String>,
    limit: Option<u32>,
) -> Result<Vec<ContainerRecord>, String> {
    let conn = open_db(&app)?;
    let cap = limit.unwrap_or(100).min(500);

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
                COALESCE(child_counts.child_count, 0) AS child_count
            FROM containers AS c
            LEFT JOIN (
                SELECT parent_container_id, COUNT(*) AS child_count
                FROM container_children
                GROUP BY parent_container_id
            ) AS child_counts ON child_counts.parent_container_id = c.id
            WHERE (?1 IS NULL OR c.source_path LIKE ?1)
              AND (?2 IS NULL OR c.display_name LIKE ?2)
            ORDER BY c.updated_at DESC, c.id DESC
            LIMIT ?3
            ",
        )
        .map_err(|e| format!("failed to prepare container search query: {e}"))?;

    let rows = stmt
        .query_map(params![path_pattern, name_pattern, cap], |row| {
            Ok(ContainerRecord {
                id: row.get(0)?,
                container_type: row.get(1)?,
                display_name: row.get(2)?,
                source_path: row.get(3)?,
                updated_at: row.get(4)?,
                child_count: row.get(5)?,
            })
        })
        .map_err(|e| format!("failed to query container search results: {e}"))?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("failed to map container search row: {e}"))?);
    }
    Ok(out)
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
        .manage(ThumbnailJobQueue::default())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            init_database,
            register_folder,
            cancel_register,
            list_recent_files,
            list_recent_containers,
            list_container_children,
            list_container_thumbnails,
            backfill_archive_container_thumbnails,
            inspect_thumbnail_cache,
            cleanup_thumbnail_cache,
            search_files,
            search_containers,
            get_file_classification,
            save_file_classification
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::{normalize_extension_from_path, scan_archive_extracted_images};
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
}
