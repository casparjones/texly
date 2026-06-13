use std::{
    io::{Cursor, Write},
    path::Path,
};

use axum::{
    body::Body,
    extract::{FromRequest, Multipart, State},
    http::header,
    response::{IntoResponse, Response},
    Json,
};
use walkdir::WalkDir;
use zip::{write::SimpleFileOptions, ZipArchive, ZipWriter};

use crate::{auth::AuthUser, error::AppError, files::check_access_pub, AppState};

pub async fn export_zip(
    State(state): State<AppState>,
    auth: AuthUser,
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Result<Response, AppError> {
    let abs_path = check_access_pub(&auth, &state.data_dir, &path)?;

    let dir = if abs_path.is_dir() {
        abs_path.clone()
    } else {
        abs_path
            .parent()
            .ok_or_else(|| AppError::BadRequest("invalid path".into()))?
            .to_path_buf()
    };

    if !dir.exists() {
        return Err(AppError::NotFound);
    }

    let zip_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("export")
        .to_string();

    let bytes = tokio::task::spawn_blocking(move || build_zip(&dir))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok((
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{zip_name}.zip\""),
            ),
            (header::CACHE_CONTROL, "no-cache".to_string()),
        ],
        Body::from(bytes),
    )
        .into_response())
}

fn build_zip(dir: &Path) -> anyhow::Result<Vec<u8>> {
    let mut buf = Vec::new();
    let cursor = Cursor::new(&mut buf);
    let mut zw = ZipWriter::new(cursor);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for entry in WalkDir::new(dir)
        .min_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let rel = entry.path().strip_prefix(dir)?;
        let rel_str = rel.to_string_lossy();

        // Skip hidden files and .build
        if rel
            .components()
            .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
        {
            continue;
        }

        if entry.file_type().is_dir() {
            zw.add_directory(rel_str.as_ref(), opts)?;
        } else {
            zw.start_file(rel_str.as_ref(), opts)?;
            let data = std::fs::read(entry.path())?;
            zw.write_all(&data)?;
        }
    }

    zw.finish()?;
    Ok(buf)
}

pub async fn import_zip(
    State(state): State<AppState>,
    auth: AuthUser,
    axum::extract::Path(path): axum::extract::Path<String>,
    request: axum::extract::Request,
) -> Result<Json<serde_json::Value>, AppError> {
    let abs_dir = check_access_pub(&auth, &state.data_dir, &path)?;

    let mut multipart = Multipart::from_request(request, &state)
        .await
        .map_err(|_| AppError::BadRequest("invalid multipart".into()))?;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart: {e}")))?
    {
        let filename = field.file_name().map(|s| s.to_string()).unwrap_or_default();

        if !filename.to_lowercase().ends_with(".zip") {
            return Err(AppError::BadRequest("only .zip files supported".into()));
        }

        let data = field
            .bytes()
            .await
            .map_err(|e| AppError::BadRequest(format!("read: {e}")))?;

        let dest = abs_dir.clone();
        tokio::task::spawn_blocking(move || extract_zip(&data, &dest))
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

fn extract_zip(data: &[u8], dest: &Path) -> anyhow::Result<()> {
    let cursor = Cursor::new(data);
    let mut archive = ZipArchive::new(cursor)?;

    // Detect common prefix to strip (e.g. if zip contains a single top-level folder)
    let prefix = {
        let first = archive.by_index(0)?.name().to_string();
        let candidate = first.split('/').next().unwrap_or("").to_string();
        let all_share = (0..archive.len()).all(|i| {
            archive
                .by_index(i)
                .ok()
                .map(|f| f.name().starts_with(&format!("{candidate}/")))
                .unwrap_or(false)
        });
        if all_share && !candidate.is_empty() {
            format!("{candidate}/")
        } else {
            String::new()
        }
    };

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let raw_name = file.name().to_string();

        // Strip common prefix
        let rel_name = raw_name
            .strip_prefix(&prefix)
            .unwrap_or(&raw_name)
            .trim_start_matches('/');

        if rel_name.is_empty() {
            continue;
        }

        // Security: reject path traversal
        if rel_name.contains("..") {
            continue;
        }

        let out_path = dest.join(rel_name);

        if file.is_dir() {
            std::fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = std::fs::File::create(&out_path)?;
            std::io::copy(&mut file, &mut out)?;
        }
    }

    Ok(())
}
