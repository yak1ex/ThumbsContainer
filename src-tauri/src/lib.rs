use rusqlite::{params, Connection};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use tauri::Manager;
use walkdir::WalkDir;

#[derive(Serialize)]
struct RegisterResult {
    scanned_files: usize,
    inserted_files: usize,
    updated_paths: usize,
    skipped_files: usize,
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
        ",
    )
    .map_err(|e| format!("failed to create schema: {e}"))?;

    Ok("database ready".to_string())
}

fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| format!("failed to open file {:?}: {e}", path))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];

    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|e| format!("failed to read file {:?}: {e}", path))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    Ok(hex::encode(hasher.finalize()))
}

#[tauri::command]
fn register_folder(app: tauri::AppHandle, folder_path: String) -> Result<RegisterResult, String> {
    let path = Path::new(&folder_path);
    if !path.exists() || !path.is_dir() {
        return Err("folder_path does not exist or is not a directory".to_string());
    }

    let conn = open_db(&app)?;

    let mut result = RegisterResult {
        scanned_files: 0,
        inserted_files: 0,
        updated_paths: 0,
        skipped_files: 0,
    };

    for entry in WalkDir::new(path).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
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

        let hash = match hash_file(file_path) {
            Ok(v) => v,
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
    }

    Ok(result)
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            init_database,
            register_folder,
            list_recent_files
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
