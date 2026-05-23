use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::Utc;
use serde::Serialize;
use serde_json::{json, Value};
use tracing::error;
use uuid::Uuid;

use crate::{
    models::{CreateTaskRequest, Task, TaskPriority, TaskStatus, UpdateTaskRequest},
    AppState,
};

/// Fan out a push to every registered device. Fire-and-forget.
fn broadcast(state: &AppState, title: String, body: String, data: Value) {
    let pool = state.pool.clone();
    let fcm = state.fcm.clone();
    tokio::spawn(async move {
        let tokens: Vec<(String,)> = match sqlx::query_as("SELECT token FROM devices")
            .fetch_all(&pool)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                error!("broadcast: failed to load device tokens: {e}");
                return;
            }
        };

        for (token,) in tokens {
            if let Err(e) = fcm
                .send_to_token(&token, &title, &body, Some(data.clone()))
                .await
            {
                let preview = &token[..8.min(token.len())];
                error!("fcm send failed for token={}: {e}", preview);
            }
        }
    });
}

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

    broadcast(
        &state,
        "New task created".into(),
        format!("\"{}\" — priority: {:?}", task.title, task.priority),
        json!({
            "type": "task_created",
            "taskId": task.id.to_string(),
            "route": "/task-details"
        }),
    );

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

    let task = match task {
        Some(task) => task,
        None => return Err(AppError::NotFound),
    };

    let just_completed =
        task.status == TaskStatus::Completed && existing_status != TaskStatus::Completed;
    let priority_bumped_to_high =
        task.priority == TaskPriority::High && existing_priority != TaskPriority::High;

    if just_completed {
        broadcast(
            &state,
            "Task completed".into(),
            task.title.clone(),
            json!({
                "type": "task_completed",
                "taskId": task.id.to_string(),
                "route": "/task-details"
            }),
        );
    } else if priority_bumped_to_high {
        broadcast(
            &state,
            "Priority bumped".into(),
            task.title.clone(),
            json!({
                "type": "task_priority_high",
                "taskId": task.id.to_string(),
                "route": "/task-details"
            }),
        );
    }

    Ok(Json(task))
}

pub async fn delete_task(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let deleted: Option<(String,)> =
        sqlx::query_as("DELETE FROM tasks WHERE id = $1 RETURNING title")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;

    let Some((title,)) = deleted else {
        return Err(AppError::NotFound);
    };

    broadcast(
        &state,
        "Task deleted".into(),
        title,
        json!({
            "type": "task_deleted",
            "taskId": id.to_string(),
            "route": "/task-details"
        }),
    );

    Ok(StatusCode::NO_CONTENT)
}
