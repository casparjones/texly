mod archive;
mod auth;
mod compile;
mod error;
mod files;
mod log_parser;
mod models;
mod users;

use std::{path::PathBuf, sync::Arc};

use axum::{
    extract::State,
    http::StatusCode,
    middleware,
    response::{Html, IntoResponse, Redirect},
    routing::{delete, get, post, put},
    Json, Router,
};
use dashmap::DashMap;
use tower_http::{cors::CorsLayer, services::ServeDir};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use users::UserStore;

#[derive(Clone)]
pub struct AppState {
    pub users: Arc<UserStore>,
    pub data_dir: PathBuf,
    pub jwt_secret: String,
    pub compile_locks: Arc<DashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Init tracing
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            "texly=info,tower_http=info".parse().unwrap()
        }))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Load .env if present (silently ignored if missing)
    let _ = dotenvy::dotenv();

    // Load env vars
    let data_dir = PathBuf::from(std::env::var("TEXLY_DATA_DIR").unwrap_or("/data".into()));
    let jwt_secret = std::env::var("TEXLY_JWT_SECRET")
        .expect("TEXLY_JWT_SECRET environment variable is required");
    let port: u16 = std::env::var("TEXLY_PORT")
        .unwrap_or("8080".into())
        .parse()
        .expect("TEXLY_PORT must be a number");
    let static_dir = std::env::var("TEXLY_STATIC_DIR").unwrap_or("./static".into());

    // Create data directories
    let users_dir = data_dir.join("users");
    let home_dir = data_dir.join("home");
    let share_dir = data_dir.join("share");

    std::fs::create_dir_all(&users_dir)?;
    std::fs::create_dir_all(&home_dir)?;
    std::fs::create_dir_all(&share_dir)?;

    // Canonicalize so all derived paths (compile, pdf serve) are absolute
    let data_dir = data_dir.canonicalize()
        .unwrap_or_else(|_| data_dir.clone());
    let users_dir = data_dir.join("users");

    let user_store = Arc::new(UserStore::new(users_dir)?);

    if user_store.count()? == 0 {
        tracing::info!(
            "No users found. Create the first user via POST /api/users — it will automatically be Superadmin."
        );
    }

    let state = AppState {
        users: user_store,
        data_dir,
        jwt_secret,
        compile_locks: Arc::new(DashMap::new()),
    };

    // Build router
    let protected = Router::new()
        .route("/api/users", get(users::list_users))
        .route("/api/users/:username", get(users::get_user))
        .route("/api/users/:username", put(users::update_user))
        .route("/api/users/:username", delete(users::delete_user))
        .route("/api/fs", get(files::list_roots))
        .route("/api/fs/*path", get(files::read_entry))
        .route("/api/fs/*path", put(files::write_file))
        .route("/api/fs/*path", delete(files::delete_entry))
        .route("/api/fs/*path", post(files::create_dir_or_upload))
        .route("/api/move/*path", post(files::move_entry))
        .route("/api/copy/*path", post(files::copy_entry))
        .route("/api/zip/*path", get(archive::export_zip))
        .route("/api/zip/*path", post(archive::import_zip))
        .route("/api/compile/*path", post(compile::compile_project))
        .route("/api/pdf/*path", get(compile::serve_pdf))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me", get(auth::me))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ));

    let app = Router::new()
        .route("/health", get(health))
        .route("/api/setup", get(setup_status))
        .route("/auth/login", post(auth::login))
        // First user creation without auth
        .route("/api/users", post(users::create_user))
        // Static pages
        .route("/", get(|| async { Redirect::temporary("/app") }))
        .route("/app", get(serve_index))
        .route("/login", get(serve_login))
        .merge(protected)
        .nest_service("/static", ServeDir::new(&static_dir))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = format!("0.0.0.0:{port}");
    tracing::info!("Texly listening on http://{addr}");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true, "version": "0.1.0" }))
}

async fn setup_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let needs_setup = state.users.count().unwrap_or(0) == 0;
    Json(serde_json::json!({ "needs_setup": needs_setup }))
}

async fn serve_index() -> impl axum::response::IntoResponse {
    let static_dir = std::env::var("TEXLY_STATIC_DIR").unwrap_or("./static".into());
    match tokio::fs::read_to_string(format!("{static_dir}/index.html")).await {
        Ok(html) => Html(html).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "index.html not found").into_response(),
    }
}

async fn serve_login() -> impl axum::response::IntoResponse {
    let static_dir = std::env::var("TEXLY_STATIC_DIR").unwrap_or("./static".into());
    match tokio::fs::read_to_string(format!("{static_dir}/login.html")).await {
        Ok(html) => Html(html).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "login.html not found").into_response(),
    }
}
