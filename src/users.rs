use std::path::PathBuf;

use anyhow::Context;
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rand::rngs::OsRng;
use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Utc;

use crate::{
    auth::{AuthUser, MaybeAuth},
    error::AppError,
    models::{CreateUserRequest, Role, UpdateUserRequest, UserInfo, UserRecord},
    AppState,
};

pub fn validate_username(name: &str) -> bool {
    !name.is_empty()
        && name.len() >= 2
        && name.len() <= 32
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub struct UserStore {
    dir: PathBuf,
}

impl UserStore {
    pub fn new(dir: PathBuf) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&dir).with_context(|| format!("create users dir {:?}", dir))?;
        Ok(Self { dir })
    }

    pub fn count(&self) -> anyhow::Result<usize> {
        let mut count = 0;
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            if entry.path().extension().and_then(|e| e.to_str()) == Some("toml") {
                count += 1;
            }
        }
        Ok(count)
    }

    pub fn exists(&self, username: &str) -> bool {
        self.dir.join(format!("{username}.toml")).exists()
    }

    pub fn list(&self) -> anyhow::Result<Vec<UserInfo>> {
        let mut users = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("toml") {
                let content = std::fs::read_to_string(&path)?;
                let record: UserRecord = toml::from_str(&content)
                    .with_context(|| format!("parse user file {:?}", path))?;
                users.push(UserInfo {
                    username: record.username,
                    role: record.role,
                });
            }
        }
        users.sort_by(|a, b| a.username.cmp(&b.username));
        Ok(users)
    }

    pub fn get(&self, username: &str) -> anyhow::Result<Option<UserRecord>> {
        let path = self.dir.join(format!("{username}.toml"));
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path)?;
        let record: UserRecord = toml::from_str(&content)?;
        Ok(Some(record))
    }

    pub fn create(&self, username: &str, password: &str, role: Role) -> anyhow::Result<()> {
        if self.exists(username) {
            anyhow::bail!("user already exists");
        }
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        let password_hash = argon2
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| anyhow::anyhow!("hash error: {e}"))?
            .to_string();
        let record = UserRecord {
            username: username.to_string(),
            password_hash,
            role,
            created_at: Utc::now().to_rfc3339(),
        };
        let content = toml::to_string(&record)?;
        std::fs::write(self.dir.join(format!("{username}.toml")), content)?;
        Ok(())
    }

    pub fn update(
        &self,
        username: &str,
        password: Option<&str>,
        role: Option<Role>,
    ) -> anyhow::Result<()> {
        let mut record = self
            .get(username)?
            .ok_or_else(|| anyhow::anyhow!("user not found"))?;
        if let Some(pw) = password {
            let salt = SaltString::generate(&mut OsRng);
            let argon2 = Argon2::default();
            let password_hash = argon2
                .hash_password(pw.as_bytes(), &salt)
                .map_err(|e| anyhow::anyhow!("hash error: {e}"))?
                .to_string();
            record.password_hash = password_hash;
        }
        if let Some(r) = role {
            record.role = r;
        }
        let content = toml::to_string(&record)?;
        std::fs::write(self.dir.join(format!("{username}.toml")), content)?;
        Ok(())
    }

    pub fn delete(&self, username: &str) -> anyhow::Result<bool> {
        let path = self.dir.join(format!("{username}.toml"));
        if !path.exists() {
            return Ok(false);
        }
        std::fs::remove_file(path)?;
        Ok(true)
    }

    pub fn verify(&self, username: &str, password: &str) -> anyhow::Result<Option<UserRecord>> {
        let record = match self.get(username)? {
            Some(r) => r,
            None => return Ok(None),
        };
        let parsed_hash = PasswordHash::new(&record.password_hash)
            .map_err(|e| anyhow::anyhow!("invalid hash: {e}"))?;
        let ok = Argon2::default()
            .verify_password(password.as_bytes(), &parsed_hash)
            .is_ok();
        if ok {
            Ok(Some(record))
        } else {
            Ok(None)
        }
    }
}

// ─── HTTP Handlers ───────────────────────────────────────────────────────────

pub async fn list_users(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<UserInfo>>, AppError> {
    if !auth.role.is_admin_or_above() {
        return Err(AppError::Forbidden);
    }
    let users = state
        .users
        .list()
        .map_err(|e| AppError::Internal(e))?;
    Ok(Json(users))
}

pub async fn create_user(
    State(state): State<AppState>,
    MaybeAuth(auth_opt): MaybeAuth,
    Json(req): Json<CreateUserRequest>,
) -> Result<Json<UserInfo>, AppError> {
    let user_count = state.users.count().unwrap_or(0);

    // First user can be created without auth (bootstrap)
    if user_count > 0 {
        let auth = auth_opt.ok_or(AppError::Unauthorized)?;
        if !auth.role.is_admin_or_above() {
            return Err(AppError::Forbidden);
        }
        // Admin cannot create superadmins
        if req.role.is_superadmin() && !auth.role.is_superadmin() {
            return Err(AppError::Forbidden);
        }
    }

    if !validate_username(&req.username) {
        return Err(AppError::BadRequest(
            "username must be 2-32 chars, alphanumeric/underscore/dash only".into(),
        ));
    }
    if req.password.len() < 6 {
        return Err(AppError::BadRequest("password too short (min 6 chars)".into()));
    }

    if state.users.exists(&req.username) {
        return Err(AppError::Conflict(format!(
            "user '{}' already exists",
            req.username
        )));
    }

    // First user is always superadmin
    let effective_role = if user_count == 0 {
        Role::Superadmin
    } else {
        req.role
    };

    // Create home directory
    let home_dir = state.data_dir.join("home").join(&req.username);
    std::fs::create_dir_all(&home_dir)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("create home dir: {e}")))?;

    state
        .users
        .create(&req.username, &req.password, effective_role.clone())
        .map_err(|e| AppError::Internal(e))?;

    tracing::info!("created user: {}", req.username);
    Ok(Json(UserInfo {
        username: req.username,
        role: effective_role,
    }))
}

pub async fn get_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(username): Path<String>,
) -> Result<Json<UserInfo>, AppError> {
    if !auth.role.is_admin_or_above() && auth.username != username {
        return Err(AppError::Forbidden);
    }
    let record = state
        .users
        .get(&username)
        .map_err(|e| AppError::Internal(e))?
        .ok_or(AppError::NotFound)?;
    Ok(Json(UserInfo {
        username: record.username,
        role: record.role,
    }))
}

pub async fn update_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(username): Path<String>,
    Json(req): Json<UpdateUserRequest>,
) -> Result<Json<UserInfo>, AppError> {
    let target = state
        .users
        .get(&username)
        .map_err(|e| AppError::Internal(e))?
        .ok_or(AppError::NotFound)?;

    // Permission checks
    if auth.role.is_superadmin() {
        // superadmin can do anything
    } else if auth.role.is_admin_or_above() {
        // admin can edit non-admin users, or change own password
        if target.role.is_admin_or_above() && auth.username != username {
            return Err(AppError::Forbidden);
        }
        // admin cannot promote to admin or superadmin
        if let Some(ref new_role) = req.role {
            if new_role.is_admin_or_above() {
                return Err(AppError::Forbidden);
            }
        }
    } else {
        // regular user: can only change own password
        if auth.username != username {
            return Err(AppError::Forbidden);
        }
        if req.role.is_some() {
            return Err(AppError::Forbidden);
        }
    }

    if let Some(ref pw) = req.password {
        if pw.len() < 6 {
            return Err(AppError::BadRequest("password too short (min 6 chars)".into()));
        }
    }

    state
        .users
        .update(
            &username,
            req.password.as_deref(),
            req.role.clone(),
        )
        .map_err(|e| AppError::Internal(e))?;

    let updated = state
        .users
        .get(&username)
        .map_err(|e| AppError::Internal(e))?
        .ok_or(AppError::NotFound)?;

    Ok(Json(UserInfo {
        username: updated.username,
        role: updated.role,
    }))
}

pub async fn delete_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(username): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !auth.role.is_superadmin() {
        return Err(AppError::Forbidden);
    }
    if auth.username == username {
        return Err(AppError::BadRequest("cannot delete yourself".into()));
    }
    let found = state
        .users
        .delete(&username)
        .map_err(|e| AppError::Internal(e))?;
    if !found {
        return Err(AppError::NotFound);
    }
    tracing::info!("deleted user: {}", username);
    Ok(Json(serde_json::json!({ "ok": true })))
}
