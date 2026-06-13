use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use axum::{
    extract::State,
    http::header,
    response::{IntoResponse, Response},
    Json,
};
use tokio::{process::Command, time::Duration};

use crate::{
    auth::AuthUser,
    error::AppError,
    files::check_access_pub,
    log_parser::parse_log,
    models::{CompileResult, DiagnosticItem},
    AppState,
};

pub async fn compile_project(
    State(state): State<AppState>,
    auth: AuthUser,
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Result<Json<CompileResult>, AppError> {
    let abs_path = check_access_pub(&auth, &state.data_dir, &path)?;

    // If a specific .tex file was given, use it directly; otherwise search the directory
    let (project_dir, tex_file) = if abs_path.is_file() {
        let dir = abs_path
            .parent()
            .ok_or_else(|| AppError::BadRequest("invalid path".into()))?
            .to_path_buf();
        (dir, abs_path.clone())
    } else {
        if !abs_path.exists() {
            return Err(AppError::NotFound);
        }
        let tex = find_main_tex(&abs_path)?;
        (abs_path.clone(), tex)
    };

    if !project_dir.exists() {
        return Err(AppError::NotFound);
    }

    let project_key = project_dir.to_string_lossy().to_string();

    // Compile lock
    let lock_arc = {
        state
            .compile_locks
            .entry(project_key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };

    let _guard = match lock_arc.try_lock() {
        Ok(g) => g,
        Err(_) => {
            return Err(AppError::Conflict("compile already running".into()));
        }
    };

    // Create .build dir
    let build_dir = project_dir.join(".build");
    tokio::fs::create_dir_all(&build_dir)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // Opt-in: resolve/download any fonts the document requests before compiling.
    // Best-effort and time-bounded; never fails the compile on its own.
    if state.font_autodownload {
        if let Ok(src) = tokio::fs::read_to_string(&tex_file).await {
            crate::fonts::ensure_fonts_for_source(&state, &src).await;
        }
    }

    let start = Instant::now();

    // Run tectonic — use filename only so it resolves relative to current_dir
    let tex_filename = tex_file
        .file_name()
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("invalid tex path")))?;

    let result = tokio::time::timeout(
        Duration::from_secs(120),
        Command::new("tectonic")
            .arg("--outdir")
            .arg(".build")
            .arg(tex_filename)
            .current_dir(&project_dir)
            .output(),
    )
    .await;

    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Err(_) => Ok(Json(CompileResult {
            ok: false,
            duration_ms,
            errors: vec![DiagnosticItem {
                file: None,
                line: None,
                message: "compile timeout (120s)".into(),
            }],
            warnings: vec![],
        })),
        Ok(Err(e)) => Err(AppError::Internal(anyhow::anyhow!("spawn tectonic: {e}"))),
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{stdout}\n{stderr}");

            let (errors, warnings) = parse_log(&combined);
            let ok = output.status.success() && errors.is_empty();

            Ok(Json(CompileResult {
                ok,
                duration_ms,
                errors,
                warnings,
            }))
        }
    }
}

fn find_main_tex(dir: &Path) -> Result<PathBuf, AppError> {
    // Prefer main.tex
    let main = dir.join("main.tex");
    if main.exists() {
        return Ok(main);
    }

    // Find all .tex files
    let mut tex_files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("tex"))
        .collect();

    if tex_files.len() == 1 {
        return Ok(tex_files.remove(0));
    }

    if tex_files.is_empty() {
        return Err(AppError::BadRequest(
            "no .tex file found in project directory".into(),
        ));
    }

    Err(AppError::BadRequest(
        "multiple .tex files found — create a main.tex".into(),
    ))
}

pub async fn serve_pdf(
    State(state): State<AppState>,
    auth: AuthUser,
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Result<Response, AppError> {
    let abs_path = check_access_pub(&auth, &state.data_dir, &path)?;

    // Resolve project dir
    let project_dir = if abs_path.is_file() {
        abs_path
            .parent()
            .ok_or_else(|| AppError::BadRequest("invalid path".into()))?
            .to_path_buf()
    } else {
        abs_path.clone()
    };

    if !project_dir.exists() {
        return Err(AppError::NotFound);
    }

    let build_dir = project_dir.join(".build");

    // If a specific .tex file was given, prefer its matching .pdf
    let pdf_path = if abs_path.is_file() {
        let stem = abs_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("main");
        let specific = build_dir.join(format!("{stem}.pdf"));
        if specific.exists() {
            specific
        } else {
            return Err(AppError::NotFound);
        }
    } else {
        // Directory given — try main.pdf, then any pdf
        let main_pdf = build_dir.join("main.pdf");
        if main_pdf.exists() {
            main_pdf
        } else {
            std::fs::read_dir(&build_dir)
                .map_err(|_| AppError::NotFound)?
                .filter_map(|e| e.ok())
                .find(|e| e.path().extension().and_then(|x| x.to_str()) == Some("pdf"))
                .map(|e| e.path())
                .ok_or(AppError::NotFound)?
        }
    };

    let data = tokio::fs::read(&pdf_path)
        .await
        .map_err(|_| AppError::NotFound)?;

    Ok((
        [
            (header::CONTENT_TYPE, "application/pdf"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        data,
    )
        .into_response())
}
