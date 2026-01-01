use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::Utc;
use serde::Serialize;
use tracing::error;
use uuid::Uuid;

use crate::{
    models::{CreateTaskRequest, Task, TaskStatus, UpdateTaskRequest},
    AppState,
};

#[derive(Debug, Serialize)]
struct ErrorResponse {
    message: String,
}

#[derive(Debug)]
pub enum AppError {
    NotFound,
    Db(sqlx::Error),
}

impl From<sqlx::Error> for AppError {
    fn from(err: sqlx::Error) -> Self {
        Self::Db(err)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        match self {
            AppError::NotFound => (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    message: "Task not found".to_string(),
                }),
            )
                .into_response(),
            AppError::Db(err) => {
                error!(error = ?err, "database error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        message: "Internal server error".to_string(),
                    }),
                )
                    .into_response()
            }
        }
    }
}

pub async fn get_tasks(State(state): State<AppState>) -> Result<Json<Vec<Task>>, AppError> {
    let tasks = sqlx::query_as::<_, Task>(
        "SELECT id, title, description, priority, status, created_at, completed_at \
         FROM tasks ORDER BY created_at DESC",
    )
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(tasks))
}

pub async fn get_task(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Task>, AppError> {
    let task = sqlx::query_as::<_, Task>(
        "SELECT id, title, description, priority, status, created_at, completed_at \
         FROM tasks WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;

    match task {
        Some(task) => Ok(Json(task)),
        None => Err(AppError::NotFound),
    }
}

pub async fn create_task(
    State(state): State<AppState>,
    Json(payload): Json<CreateTaskRequest>,
) -> Result<(StatusCode, Json<Task>), AppError> {
    let status = payload.status.unwrap_or(TaskStatus::Pending);
    let completed_at = if matches!(status, TaskStatus::Completed) {
        Some(Utc::now())
    } else {
        None
    };

    let task = sqlx::query_as::<_, Task>(
        "INSERT INTO tasks (id, title, description, priority, status, completed_at) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING id, title, description, priority, status, created_at, completed_at",
    )
    .bind(Uuid::new_v4())
    .bind(payload.title)
    .bind(payload.description)
    .bind(payload.priority)
    .bind(status)
    .bind(completed_at)
    .fetch_one(&state.pool)
    .await?;

    Ok((StatusCode::CREATED, Json(task)))
}

pub async fn update_task(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateTaskRequest>,
) -> Result<Json<Task>, AppError> {
    let existing = sqlx::query_as::<_, Task>(
        "SELECT id, title, description, priority, status, created_at, completed_at \
         FROM tasks WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;

    let existing = match existing {
        Some(task) => task,
        None => return Err(AppError::NotFound),
    };

    let Task {
        title: existing_title,
        description: existing_description,
        priority: existing_priority,
        status: existing_status,
        completed_at: existing_completed_at,
        ..
    } = existing;

    let status_was_set = payload.status.is_some();
    let status = payload.status.unwrap_or(existing_status);
    // Keep completed_at aligned with status transitions.
    let completed_at = if matches!(status, TaskStatus::Completed) {
        if existing_status != TaskStatus::Completed || existing_completed_at.is_none() {
            Some(Utc::now())
        } else {
            existing_completed_at
        }
    } else if status_was_set {
        None
    } else {
        existing_completed_at
    };

    let title = payload.title.unwrap_or(existing_title);
    let description = payload.description.unwrap_or(existing_description);
    let priority = payload.priority.unwrap_or(existing_priority);

    let task = sqlx::query_as::<_, Task>(
        "UPDATE tasks \
         SET title = $1, description = $2, priority = $3, status = $4, completed_at = $5 \
         WHERE id = $6 \
         RETURNING id, title, description, priority, status, created_at, completed_at",
    )
    .bind(title)
    .bind(description)
    .bind(priority)
    .bind(status)
    .bind(completed_at)
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;

    match task {
        Some(task) => Ok(Json(task)),
        None => Err(AppError::NotFound),
    }
}

pub async fn delete_task(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let result = sqlx::query("DELETE FROM tasks WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}
