use std::path::{Path, PathBuf};

use axum::{
    body::Body,
    extract::{FromRequest, Multipart, Query, State},
    http::header,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use tokio::fs;

use crate::{auth::AuthUser, error::AppError, models::FileEntry, AppState};

// ─── Path helpers ─────────────────────────────────────────────────────────────

fn safe_join(base: &Path, rel: &str) -> Result<PathBuf, AppError> {
    // Strip leading slashes
    let rel = rel.trim_start_matches('/');

    // Build candidate path
    let candidate = base.join(rel);

    // Normalize by resolving components without requiring it to exist first
    let normalized = normalize_path(&candidate);

    // Ensure base itself is normalized
    let base_normalized = normalize_path(base);

    if !normalized.starts_with(&base_normalized) {
        return Err(AppError::BadRequest("path traversal detected".into()));
    }

    Ok(normalized)
}

/// Normalize a path without requiring it to exist (no canonicalize)
fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for comp in path.components() {
        match comp {
            std::path::Component::ParentDir => {
                components.pop();
            }
            std::path::Component::CurDir => {}
            other => components.push(other),
        }
    }
    components.iter().collect()
}

pub fn check_access_pub(auth: &AuthUser, data_dir: &Path, path: &str) -> Result<PathBuf, AppError> {
    check_access(auth, data_dir, path)
}

fn check_access(auth: &AuthUser, data_dir: &Path, path: &str) -> Result<PathBuf, AppError> {
    let path = path.trim_start_matches('/');
    let parts: Vec<&str> = path.splitn(3, '/').collect();

    match parts.as_slice() {
        ["home", username, ..] => {
            if !auth.role.is_admin_or_above() && *username != auth.username.as_str() {
                return Err(AppError::Forbidden);
            }
        }
        ["share", ..] => {} // all users can access share
        _ => return Err(AppError::BadRequest("invalid path prefix".into())),
    }

    safe_join(data_dir, path)
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

pub async fn list_roots(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<FileEntry>>, AppError> {
    let mut roots = Vec::new();

    if auth.role.is_admin_or_above() {
        // List all home directories
        let home_base = state.data_dir.join("home");
        if home_base.exists() {
            let mut rd = fs::read_dir(&home_base)
                .await
                .map_err(|e| AppError::Internal(anyhow::anyhow!("read home dir: {e}")))?;
            let mut home_entries = Vec::new();
            while let Some(entry) = rd
                .next_entry()
                .await
                .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            {
                if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                    let name = entry.file_name().to_string_lossy().to_string();
                    home_entries.push(FileEntry {
                        name: name.clone(),
                        path: format!("home/{name}"),
                        kind: "dir".into(),
                        children: None,
                    });
                }
            }
            home_entries.sort_by(|a, b| a.name.cmp(&b.name));
            roots.extend(home_entries);
        }
    } else {
        // Only own home
        roots.push(FileEntry {
            name: auth.username.clone(),
            path: format!("home/{}", auth.username),
            kind: "dir".into(),
            children: None,
        });
    }

    // Share folder
    roots.push(FileEntry {
        name: "share".into(),
        path: "share".into(),
        kind: "dir".into(),
        children: None,
    });

    Ok(Json(roots))
}

pub async fn read_entry(
    State(state): State<AppState>,
    auth: AuthUser,
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Result<Response, AppError> {
    let abs_path = check_access(&auth, &state.data_dir, &path)?;

    if !abs_path.exists() {
        return Err(AppError::NotFound);
    }

    let meta = fs::metadata(&abs_path)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    if meta.is_dir() {
        let entries = build_file_tree(&abs_path, &path)?;
        Ok(Json(entries).into_response())
    } else {
        // Serve file with correct content type
        let mime = mime_guess::from_path(&abs_path).first_or_octet_stream();
        let content = fs::read(&abs_path)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        Ok(([(header::CONTENT_TYPE, mime.as_ref().to_string())], content).into_response())
    }
}

fn build_file_tree(abs_dir: &Path, rel_prefix: &str) -> Result<Vec<FileEntry>, AppError> {
    let mut entries = Vec::new();

    let read_dir =
        std::fs::read_dir(abs_dir).map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    for entry in read_dir {
        let entry = entry.map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        let name = entry.file_name().to_string_lossy().to_string();
        // Skip .build output directory and hidden files/dirs
        if name == ".build" || name.starts_with('.') {
            continue;
        }
        let meta = entry
            .metadata()
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        let rel_path = format!("{}/{}", rel_prefix.trim_end_matches('/'), name);

        if meta.is_dir() {
            let children = build_file_tree(&entry.path(), &rel_path)?;
            entries.push(FileEntry {
                name,
                path: rel_path,
                kind: "dir".into(),
                children: Some(children),
            });
        } else {
            entries.push(FileEntry {
                name,
                path: rel_path,
                kind: "file".into(),
                children: None,
            });
        }
    }

    entries.sort_by(|a, b| {
        // dirs first, then files, then alphabetical
        match (a.kind.as_str(), b.kind.as_str()) {
            ("dir", "file") => std::cmp::Ordering::Less,
            ("file", "dir") => std::cmp::Ordering::Greater,
            _ => a.name.cmp(&b.name),
        }
    });

    Ok(entries)
}

pub async fn write_file(
    State(state): State<AppState>,
    auth: AuthUser,
    axum::extract::Path(path): axum::extract::Path<String>,
    body: Body,
) -> Result<Json<serde_json::Value>, AppError> {
    let abs_path = check_access(&auth, &state.data_dir, &path)?;

    // Create parent directories
    if let Some(parent) = abs_path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    }

    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    fs::write(&abs_path, &bytes)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn delete_entry(
    State(state): State<AppState>,
    auth: AuthUser,
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let abs_path = check_access(&auth, &state.data_dir, &path)?;

    if !abs_path.exists() {
        return Err(AppError::NotFound);
    }

    let meta = fs::metadata(&abs_path)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    if meta.is_dir() {
        fs::remove_dir_all(&abs_path)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    } else {
        fs::remove_file(&abs_path)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct MoveRequest {
    pub destination: String,
    pub new_name: Option<String>,
}

pub async fn move_entry(
    State(state): State<AppState>,
    auth: AuthUser,
    axum::extract::Path(path): axum::extract::Path<String>,
    Json(body): Json<MoveRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let src = check_access(&auth, &state.data_dir, &path)?;

    if !src.exists() {
        return Err(AppError::NotFound);
    }

    let dst_dir = check_access(&auth, &state.data_dir, &body.destination)?;

    if dst_dir.exists() && !dst_dir.is_dir() {
        return Err(AppError::BadRequest(
            "destination must be a directory".into(),
        ));
    }

    fs::create_dir_all(&dst_dir)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let name = if let Some(n) = &body.new_name {
        // Sanitize new name
        let p = Path::new(n)
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| AppError::BadRequest("invalid new_name".into()))?
            .to_string();
        p
    } else {
        src.file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| AppError::BadRequest("invalid source path".into()))?
            .to_string()
    };

    let dst = dst_dir.join(&name);

    if dst == src {
        return Ok(Json(serde_json::json!({ "ok": true })));
    }

    fs::rename(&src, &dst)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn copy_entry(
    State(state): State<AppState>,
    auth: AuthUser,
    axum::extract::Path(path): axum::extract::Path<String>,
    Json(body): Json<MoveRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let src = check_access(&auth, &state.data_dir, &path)?;

    if !src.exists() {
        return Err(AppError::NotFound);
    }

    let dst_dir = check_access(&auth, &state.data_dir, &body.destination)?;

    fs::create_dir_all(&dst_dir)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let name = src
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| AppError::BadRequest("invalid source path".into()))?
        .to_string();

    let dst = dst_dir.join(&name);

    tokio::task::spawn_blocking(move || copy_recursive(&src, &dst))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

fn copy_recursive(src: &Path, dst: &Path) -> anyhow::Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with('.') {
                continue;
            }
            copy_recursive(&entry.path(), &dst.join(&name))?;
        }
    } else {
        std::fs::copy(src, dst)?;
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct FsQuery {
    pub action: Option<String>,
}

pub async fn create_dir_or_upload(
    State(state): State<AppState>,
    auth: AuthUser,
    axum::extract::Path(path): axum::extract::Path<String>,
    Query(query): Query<FsQuery>,
    request: axum::extract::Request,
) -> Result<Response, AppError> {
    if query.action.as_deref() == Some("mkdir") {
        let abs_path = check_access(&auth, &state.data_dir, &path)?;
        fs::create_dir_all(&abs_path)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        return Ok(Json(serde_json::json!({ "ok": true })).into_response());
    }

    // Check if multipart
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    if content_type.contains("multipart/form-data") {
        let abs_dir = check_access(&auth, &state.data_dir, &path)?;

        // Extract multipart
        let mut multipart = Multipart::from_request(request, &state)
            .await
            .map_err(|_| AppError::BadRequest("invalid multipart".into()))?;

        while let Some(field) = multipart
            .next_field()
            .await
            .map_err(|e| AppError::BadRequest(format!("multipart error: {e}")))?
        {
            let filename = field
                .file_name()
                .map(|s| s.to_string())
                .or_else(|| field.name().map(|s| s.to_string()))
                .ok_or_else(|| AppError::BadRequest("no filename in upload".into()))?;

            // Sanitize filename
            let filename = Path::new(&filename)
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| AppError::BadRequest("invalid filename".into()))?
                .to_string();

            let target = abs_dir.join(&filename);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .await
                    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
            }

            let data = field
                .bytes()
                .await
                .map_err(|e| AppError::BadRequest(format!("read field: {e}")))?;

            fs::write(&target, &data)
                .await
                .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        }

        Ok(Json(serde_json::json!({ "ok": true })).into_response())
    } else {
        Err(AppError::BadRequest(
            "use ?action=mkdir or multipart/form-data upload".into(),
        ))
    }
}
