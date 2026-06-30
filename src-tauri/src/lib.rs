use rusqlite::{params, Connection};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager};
use walkdir::WalkDir;

const REGISTER_CANCELLED: &str = "register_cancelled";

#[derive(Serialize)]
struct RegisterResult {
    scanned_files: usize,
    inserted_files: usize,
    updated_paths: usize,
    skipped_files: usize,
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
}

#[derive(Serialize)]
struct FileClassification {
    tags: Vec<String>,
    rating: Option<i64>,
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
    Connection::open(db_path).map_err(|e| format!("failed to open db: {e}"))
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
        ",
    )
    .map_err(|e| format!("failed to create schema: {e}"))?;

    Ok("database ready".to_string())
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

    Ok(result)
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
            SELECT id, path, filename, hash, size, created_at
            FROM files
            ORDER BY updated_at DESC, id DESC
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
                        SELECT f.id, f.path, f.filename, f.hash, f.size, f.created_at
                        FROM files f
                        LEFT JOIN file_ratings fr ON fr.file_id = f.id
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
            search_files,
            get_file_classification,
            save_file_classification
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
