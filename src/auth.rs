use axum::{
    async_trait,
    extract::{FromRequestParts, Request, State},
    http::request::Parts,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use axum_extra::extract::CookieJar;
use chrono::Utc;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde_json::json;

use axum::extract::FromRef;

use crate::{
    error::AppError,
    models::{Claims, LoginRequest, Role, UserInfo},
    AppState,
};

pub const COOKIE_NAME: &str = "texly_session";
const TOKEN_EXPIRY_SECS: i64 = 7 * 24 * 3600;

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub username: String,
    pub role: Role,
}

pub fn create_token(username: &str, role: &Role, secret: &str) -> anyhow::Result<String> {
    let exp = (Utc::now().timestamp() + TOKEN_EXPIRY_SECS) as usize;
    let claims = Claims {
        sub: username.to_string(),
        role: role.clone(),
        exp,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;
    Ok(token)
}

pub fn verify_token(token: &str, secret: &str) -> anyhow::Result<Claims> {
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )?;
    Ok(data.claims)
}

// ─── Middleware ───────────────────────────────────────────────────────────────

pub async fn require_auth(
    State(state): State<AppState>,
    jar: CookieJar,
    mut req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let token = jar
        .get(COOKIE_NAME)
        .map(|c| c.value().to_string())
        .ok_or(AppError::Unauthorized)?;

    let claims = verify_token(&token, &state.jwt_secret).map_err(|_| AppError::Unauthorized)?;

    req.extensions_mut().insert(AuthUser {
        username: claims.sub,
        role: claims.role,
    });

    Ok(next.run(req).await)
}

// ─── Extractor ────────────────────────────────────────────────────────────────

#[async_trait]
impl<S: Send + Sync> FromRequestParts<S> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<AuthUser>()
            .cloned()
            .ok_or(AppError::Unauthorized)
    }
}

// Optional wrapper — used in handlers that don't require auth.
// Works both behind and outside the require_auth middleware.
pub struct MaybeAuth(pub Option<AuthUser>);

#[async_trait]
impl<S: Send + Sync> FromRequestParts<S> for MaybeAuth
where
    AppState: FromRef<S>,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        // Try extension first (set by require_auth middleware)
        if let Some(user) = parts.extensions.get::<AuthUser>().cloned() {
            return Ok(MaybeAuth(Some(user)));
        }
        // Fall back: read JWT directly from cookie (route not behind middleware)
        let jar = CookieJar::from_request_parts(parts, state).await.unwrap_or_default();
        let app_state = AppState::from_ref(state);
        let user = jar.get(COOKIE_NAME).and_then(|c| {
            let claims = verify_token(c.value(), &app_state.jwt_secret).ok()?;
            Some(AuthUser { username: claims.sub, role: claims.role })
        });
        Ok(MaybeAuth(user))
    }
}

// ─── Handlers ────────────────────────────────────────────────────────────────

pub async fn login(
    State(state): State<AppState>,
    _jar: CookieJar,
    Json(req): Json<LoginRequest>,
) -> Result<impl IntoResponse, AppError> {
    let record = state
        .users
        .verify(&req.username, &req.password)
        .map_err(|e| AppError::Internal(e))?
        .ok_or(AppError::Unauthorized)?;

    let token = create_token(&record.username, &record.role, &state.jwt_secret)
        .map_err(|e| AppError::Internal(e))?;

    let cookie = format!(
        "{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={TOKEN_EXPIRY_SECS}"
    );

    Ok((
        [(axum::http::header::SET_COOKIE, cookie)],
        Json(json!({
            "ok": true,
            "username": record.username,
            "role": record.role,
        })),
    ))
}

pub async fn logout(_jar: CookieJar) -> impl IntoResponse {
    let cookie = format!(
        "{COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT"
    );
    (
        [(axum::http::header::SET_COOKIE, cookie)],
        Json(json!({ "ok": true })),
    )
}

pub async fn me(auth: AuthUser) -> Json<UserInfo> {
    Json(UserInfo {
        username: auth.username,
        role: auth.role,
    })
}
