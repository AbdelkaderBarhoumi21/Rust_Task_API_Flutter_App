use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::{
    handlers::AppError,
    models::{Device, RegisterDeviceRequest},
    AppState,
};

/// POST /devices — register or refresh an FCM token.
/// Idempotent: if the token already exists, bumps `last_seen_at`.
pub async fn register_device(
    State(state): State<AppState>,
    Json(req): Json<RegisterDeviceRequest>,
) -> Result<(StatusCode, Json<Device>), AppError> {
    let id = Uuid::new_v4();
    let device = sqlx::query_as::<_, Device>(
        r#"
        INSERT INTO devices (id, token, platform)
        VALUES ($1, $2, $3)
        ON CONFLICT (token)
        DO UPDATE SET last_seen_at = now()
        RETURNING id, token, platform, created_at, last_seen_at
        "#,
    )
    .bind(id)
    .bind(&req.token)
    .bind(req.platform)
    .fetch_one(&state.pool)
    .await?;

    Ok((StatusCode::CREATED, Json(device)))
}

/// DELETE /devices/:id — unregister.
pub async fn delete_device(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let result = sqlx::query("DELETE FROM devices WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}
