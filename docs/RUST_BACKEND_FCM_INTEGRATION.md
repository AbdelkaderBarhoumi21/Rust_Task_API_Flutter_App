# Rust Backend — FCM Integration Guide

> Add **Firebase Cloud Messaging (FCM HTTP v1 API)** to the existing `task_api` (Axum + SQLx + Postgres) so the backend can push notifications to Flutter clients on task lifecycle events.

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Project Structure (after integration)](#2-project-structure-after-integration)
3. [Step 1 — Add Dependencies](#step-1--add-dependencies)
4. [Step 2 — Firebase Setup & Service Account](#step-2--firebase-setup--service-account)
5. [Step 3 — Environment Configuration](#step-3--environment-configuration)
6. [Step 4 — New Migration: `devices` Table](#step-4--new-migration-devices-table)
7. [Step 5 — Domain Models for Devices & Notifications](#step-5--domain-models)
8. [Step 6 — FCM Client Module (`src/fcm.rs`)](#step-6--fcm-client-module)
9. [Step 7 — Device Registration Handlers (`src/devices.rs`)](#step-7--device-registration-handlers)
10. [Step 8 — Wire FCM into `AppState` and Router](#step-8--wire-fcm-into-appstate-and-router)
11. [Step 9 — Trigger Pushes from Task Handlers](#step-9--trigger-pushes-from-task-handlers)
12. [Step 10 — Notification Payload Conventions](#step-10--notification-payload-conventions)
13. [Step 11 — Testing the Backend](#step-11--testing-the-backend)
14. [Step 12 — Production Notes](#step-12--production-notes)

---

## 1. Architecture Overview

```
┌──────────────────┐       HTTPS        ┌──────────────────────┐
│  Flutter Client  │ ─── POST /devices ──▶│   task_api (Rust)    │
│  (FCM SDK)       │                    │  Axum + SQLx + Tokio │
└────────┬─────────┘                    └──────┬───────────────┘
         │                                     │
         │ stores FCM token                    │ stores token in `devices`
         │                                     │
         │                                     │ on task event:
         │                                     ▼
         │                          ┌────────────────────────┐
         │                          │   FcmClient (src/fcm)  │
         │                          │   • OAuth2 (service    │
         │                          │     account)           │
         │                          │   • POST /v1/projects/ │
         │                          │     {pid}/messages:send│
         │                          └──────┬─────────────────┘
         │                                 │
         │                                 ▼
         │                       ┌─────────────────────┐
         └──────── push ─────────│  Google FCM Servers │
                                 └─────────────────────┘
```

**Key design choices:**

- We use the **FCM HTTP v1 API** (the legacy server-key API is deprecated).
- OAuth2 access tokens are minted from a **Google service account JSON** and **cached until expiry**.
- Push sends are **fire-and-forget** (`tokio::spawn`) so notification delivery never fails the HTTP request.
- The `devices` table is **single-tenant** for now (no auth) — easy to extend later with a `user_id` column.

---

## 2. Project Structure (after integration)

```
task_api/
├── Cargo.toml
├── .env
├── service-account.json             # ← NEW (gitignored!)
├── migrations/
│   ├── 001_create_tasks_table.sql
│   └── 002_create_devices_table.sql # ← NEW
└── src/
    ├── main.rs                      # updated: AppState + routes
    ├── models.rs                    # updated: Device, RegisterDeviceRequest
    ├── handlers.rs                  # updated: trigger pushes
    ├── devices.rs                   # ← NEW: device CRUD
    └── fcm.rs                       # ← NEW: FCM client
```

---

## Step 1 — Add Dependencies

Add the following to `Cargo.toml` under `[dependencies]`:

```toml
# HTTP client to call FCM
reqwest = { version = "0.12", features = ["json", "rustls-tls"] }

# Google service-account auth (mints OAuth2 access tokens)
yup-oauth2 = "11"

# Async-friendly synchronization for token cache
tokio = { version = "1.36", features = ["macros", "rt-multi-thread", "sync"] }

# Error handling (optional but recommended)
thiserror = "1.0"
anyhow = "1.0"
```

**Why each one:**

- **`reqwest`** — async HTTPS client for talking to `fcm.googleapis.com`. We use `rustls-tls` to avoid the system OpenSSL dependency (cleaner for Docker images later).
- **`yup-oauth2`** — handles the JWT signing + token exchange dance for Google service accounts. You give it the JSON file, it gives you `Bearer` tokens.
- **`tokio` `sync` feature** — adds `tokio::sync::RwLock` which we use to cache the access token across requests.
- **`thiserror` / `anyhow`** — ergonomic error types for the FCM module without polluting `AppError`.

---

## Step 2 — Firebase Setup & Service Account

1. Go to **[Firebase Console](https://console.firebase.google.com/)** → create / select your project.
2. **Project settings** → **Service accounts** tab → **Generate new private key**.
3. Save the downloaded JSON as `service-account.json` at the project root.
4. **Add it to `.gitignore` immediately** — this file grants full FCM send rights:

```
# .gitignore
service-account.json
.env
target/
```

5. Note your **Firebase project ID** (visible in Project settings → General). You'll need it for the API URL.

---

## Step 3 — Environment Configuration

Update `.env`:

```env
DATABASE_URL=postgresql://postgres:123456@localhost:5432/task_db

# FCM
FIREBASE_PROJECT_ID=your-firebase-project-id
FIREBASE_SERVICE_ACCOUNT_PATH=./service-account.json
```

**Why two variables?**
- `FIREBASE_PROJECT_ID` is embedded directly in the FCM endpoint URL (`/v1/projects/{project_id}/messages:send`).
- `FIREBASE_SERVICE_ACCOUNT_PATH` lets you keep the credentials file outside the repo and point at it (in production you'd point to a mounted secret).

---

## Step 4 — New Migration: `devices` Table

Create `migrations/002_create_devices_table.sql`:

```sql
CREATE TYPE device_platform AS ENUM ('android', 'ios', 'web');

CREATE TABLE devices (
    id           UUID PRIMARY KEY,
    token        TEXT NOT NULL UNIQUE,
    platform     device_platform NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_devices_token ON devices (token);
```

**Explanation of each column:**

- **`id`** — internal UUID we control (lets us expose a delete endpoint without leaking the FCM token in URLs).
- **`token`** — the FCM registration token returned by the Flutter SDK. `UNIQUE` so re-registering the same device updates instead of duplicates.
- **`platform`** — useful later for platform-specific payloads (Android needs `android.notification.icon`, iOS needs `apns.payload.aps`).
- **`last_seen_at`** — touched whenever the client re-registers; lets us prune stale tokens periodically.

> The migration auto-runs on startup via `sqlx::migrate!("./migrations").run(&pool)` — no extra step needed.

---

## Step 5 — Domain Models

Append to `src/models.rs`:

```rust
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Copy, sqlx::Type, Serialize, Deserialize)]
#[sqlx(type_name = "device_platform", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum DevicePlatform {
    Android,
    Ios,
    Web,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: Uuid,
    pub token: String,
    pub platform: DevicePlatform,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterDeviceRequest {
    pub token: String,
    pub platform: DevicePlatform,
}
```

**Why `sqlx::Type` on the enum?** It maps the Rust enum to the Postgres `device_platform` enum at compile time, so SQLx can validate queries.

---

## Step 6 — FCM Client Module

Create `src/fcm.rs`:

```rust
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::RwLock;
use yup_oauth2::{ServiceAccountAuthenticator, ServiceAccountKey};

const FCM_SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";

#[derive(Debug, thiserror::Error)]
pub enum FcmError {
    #[error("oauth error: {0}")]
    Oauth(String),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("fcm api error {status}: {body}")]
    Api { status: u16, body: String },
}

pub struct FcmClient {
    project_id: String,
    http: reqwest::Client,
    authenticator: yup_oauth2::authenticator::Authenticator<
        yup_oauth2::hyper_rustls::HttpsConnector<yup_oauth2::hyper::client::HttpConnector>,
    >,
    cached_token: RwLock<Option<(String, std::time::Instant)>>,
}

impl FcmClient {
    pub async fn new(project_id: String, service_account_path: &str) -> anyhow::Result<Arc<Self>> {
        let key: ServiceAccountKey = yup_oauth2::read_service_account_key(service_account_path)
            .await
            .map_err(|e| anyhow::anyhow!("failed to read service account: {e}"))?;

        let authenticator = ServiceAccountAuthenticator::builder(key)
            .build()
            .await
            .map_err(|e| anyhow::anyhow!("failed to build authenticator: {e}"))?;

        Ok(Arc::new(Self {
            project_id,
            http: reqwest::Client::new(),
            authenticator,
            cached_token: RwLock::new(None),
        }))
    }

    /// Returns a valid OAuth2 access token, refreshing if expired.
    async fn access_token(&self) -> Result<String, FcmError> {
        // Fast path: read-lock and check cache.
        {
            let guard = self.cached_token.read().await;
            if let Some((token, expires_at)) = guard.as_ref() {
                if std::time::Instant::now() < *expires_at {
                    return Ok(token.clone());
                }
            }
        }
        // Slow path: refresh.
        let token = self
            .authenticator
            .token(&[FCM_SCOPE])
            .await
            .map_err(|e| FcmError::Oauth(e.to_string()))?;
        let token_str = token
            .token()
            .ok_or_else(|| FcmError::Oauth("empty token".into()))?
            .to_string();

        // yup-oauth2 doesn't always expose expiry; use a safe 50-min cache.
        let expires_at = std::time::Instant::now() + std::time::Duration::from_secs(50 * 60);
        *self.cached_token.write().await = Some((token_str.clone(), expires_at));
        Ok(token_str)
    }

    /// Send a notification to a single FCM device token.
    pub async fn send_to_token(
        &self,
        device_token: &str,
        title: &str,
        body: &str,
        data: Option<Value>,
    ) -> Result<(), FcmError> {
        let token = self.access_token().await?;
        let url = format!(
            "https://fcm.googleapis.com/v1/projects/{}/messages:send",
            self.project_id
        );

        let payload = json!({
            "message": {
                "token": device_token,
                "notification": { "title": title, "body": body },
                "data": data.unwrap_or(json!({})),
                "android": {
                    "priority": "HIGH",
                    "notification": {
                        "channel_id": "task_updates",
                        "icon": "ic_notification"
                    }
                },
                "apns": {
                    "payload": { "aps": { "sound": "default" } }
                }
            }
        });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(FcmError::Api { status, body });
        }
        Ok(())
    }
}
```

**What's happening here, step by step:**

1. **`new`** loads the service account JSON from disk and builds a `yup-oauth2` authenticator. We return `Arc<Self>` because `AppState` is cloned per request and we want one shared client.
2. **`access_token`** uses a `RwLock`-backed cache: most calls hit the fast read-lock path; only when the token nears expiry do we acquire the write lock and refresh. We use a conservative **50-minute** cache (Google tokens are valid for 60 minutes).
3. **`send_to_token`** posts the FCM v1 message payload. Note the `android.notification.icon = "ic_notification"` — this references a drawable in the Flutter app's Android resources (we'll create it on the Flutter side). The `channel_id = "task_updates"` must match the channel Flutter creates locally.
4. Errors are bucketed: OAuth failures, network failures, and FCM API failures (4xx/5xx) are all distinguishable, so the caller can log them meaningfully.

---

## Step 7 — Device Registration Handlers

Create `src/devices.rs`:

```rust
use axum::{extract::{Path, State}, http::StatusCode, Json};
use uuid::Uuid;

use crate::handlers::AppError;
use crate::main::AppState; // adjust path: see note below
use crate::models::{Device, RegisterDeviceRequest};

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
    .await
    .map_err(AppError::Db)?;

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
        .await
        .map_err(AppError::Db)?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}
```

**Why `ON CONFLICT (token) DO UPDATE`?** Flutter's FCM SDK can rotate tokens (on app reinstall, after some time, etc.) and re-register them. Without an upsert, we'd either get duplicate-key errors or stale rows.

> **Note on the `use crate::main::AppState;` line:** Rust doesn't let you import from `main.rs` cleanly. The proper fix is to move `AppState` into a small `src/state.rs` module (or `src/lib.rs`). For brevity here, define `AppState` in a shared module and import from it everywhere. See Step 8.

---

## Step 8 — Wire FCM into `AppState` and Router

Refactor `src/main.rs`:

```rust
use axum::{routing::{delete, get, post, put}, Router};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::EnvFilter;

mod devices;
mod fcm;
mod handlers;
mod models;

use fcm::FcmClient;

#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::PgPool,
    pub fcm: Arc<FcmClient>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let database_url = std::env::var("DATABASE_URL")?;
    let project_id = std::env::var("FIREBASE_PROJECT_ID")?;
    let sa_path = std::env::var("FIREBASE_SERVICE_ACCOUNT_PATH")?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    sqlx::migrate!("./migrations").run(&pool).await?;

    let fcm = FcmClient::new(project_id, &sa_path).await?;
    let state = AppState { pool, fcm };

    let cors = CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any);

    let app = Router::new()
        // tasks
        .route("/tasks", get(handlers::get_tasks).post(handlers::create_task))
        .route(
            "/tasks/:id",
            get(handlers::get_task).put(handlers::update_task).delete(handlers::delete_task),
        )
        // devices
        .route("/devices", post(devices::register_device))
        .route("/devices/:id", delete(devices::delete_device))
        .layer(cors)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    tracing::info!("listening on http://0.0.0.0:3000");
    axum::serve(listener, app).await?;
    Ok(())
}
```

**What changed:**

- `AppState` now carries `Arc<FcmClient>` so handlers can reach FCM.
- The `FcmClient` is built **once at startup** (so the service-account JSON is parsed once and the HTTPS client connection pool is reused).
- Two new routes: `POST /devices` and `DELETE /devices/:id`.

---

## Step 9 — Trigger Pushes from Task Handlers

Update relevant handlers in `src/handlers.rs`. Below is a helper plus an example wiring into `create_task`:

```rust
use serde_json::json;

/// Fan out a push to every registered device. Fire-and-forget.
async fn broadcast(state: &AppState, title: String, body: String, data: serde_json::Value) {
    let pool = state.pool.clone();
    let fcm = state.fcm.clone();
    tokio::spawn(async move {
        let tokens: Vec<(String,)> = match sqlx::query_as("SELECT token FROM devices")
            .fetch_all(&pool)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("broadcast: failed to load device tokens: {e}");
                return;
            }
        };

        for (token,) in tokens {
            if let Err(e) = fcm.send_to_token(&token, &title, &body, Some(data.clone())).await {
                tracing::error!("fcm send failed for token={}: {e}", &token[..8.min(token.len())]);
            }
        }
    });
}

// Inside create_task, AFTER the INSERT succeeds:
broadcast(
    &state,
    "New task created".into(),
    format!("\"{}\" — priority: {:?}", task.title, task.priority),
    json!({
        "type": "task_created",
        "taskId": task.id.to_string(),
        "route": "/taskDetails" // ← Flutter uses this to navigate
    }),
).await;
```

**Where to call `broadcast`:**

| Event                                          | Title                  | Body                                   |
|------------------------------------------------|------------------------|----------------------------------------|
| `POST /tasks` (always)                         | "New task created"     | task title + priority                  |
| `PUT /tasks/:id` when status → `Completed`     | "Task completed"       | task title                             |
| `PUT /tasks/:id` when priority changes to High | "Priority bumped"      | task title                             |
| `DELETE /tasks/:id` (optional)                 | "Task deleted"         | task title                             |

**Why `tokio::spawn`?**

- The HTTP response returns immediately — the caller never waits for FCM.
- A failing FCM call **never** turns a successful task creation into a 500.
- Errors are logged via `tracing::error!`, ready for a monitoring system.

---

## Step 10 — Notification Payload Conventions

To keep the Flutter side simple and predictable, agree on a **fixed `data` schema**:

```json
{
  "type": "task_created" | "task_completed" | "task_deleted",
  "taskId": "<uuid>",
  "route": "/taskDetails"
}
```

- **`type`** — lets Flutter switch on the event (analytics, custom UI).
- **`taskId`** — the entity to fetch / open.
- **`route`** — the GoRouter path to navigate to on tap. Putting routing inside the payload means **the backend controls navigation** — you can change destinations server-side without shipping a Flutter update.

---

## Step 11 — Testing the Backend

### 11.1 Manual smoke test with `curl`

```bash
# 1. Register a fake device
curl -X POST http://localhost:3000/devices \
  -H "Content-Type: application/json" \
  -d '{"token":"FAKE_TOKEN_FROM_FLUTTER","platform":"android"}'

# 2. Create a task — should trigger a push (check logs)
curl -X POST http://localhost:3000/tasks \
  -H "Content-Type: application/json" \
  -d '{"title":"Test","description":"Hello","priority":"high"}'
```

You'll see something like this in logs (assuming the token is invalid):

```
ERROR fcm send failed for token=FAKE_TOK: fcm api error 404: { "error": ... }
```

That's expected — the **task creation still returned 201**.

### 11.2 Integration test sketch

In `tests/devices.rs` (create the `tests/` folder if missing):

```rust
#[tokio::test]
async fn register_device_is_idempotent() {
    // spin up app + test pool, post the same token twice,
    // assert second response is 201 and only one row exists.
}
```

For full integration tests you'll want `sqlx::test` macro or a test container — out of scope here.

---

## Step 12 — Production Notes

- **Never commit `service-account.json`.** Mount it as a Kubernetes secret / Docker secret in production.
- **Rotate stale tokens.** Add a periodic job (or a `last_seen_at < now() - interval '60 days'` cleanup) — FCM rejects long-dead tokens, and clutter inflates the broadcast loop.
- **Per-token send is O(n).** For >1k devices, switch to **FCM topics** (subscribe all clients to e.g. `all-tasks` and send to `/topics/all-tasks`).
- **Observability.** Wrap `send_to_token` with a metric (`fcm_send_total{result="ok|err"}`) so you can alert on delivery failure spikes.
- **TLS for the public API.** Put Axum behind nginx / Caddy with HTTPS — FCM tokens are sensitive.

---

**Done.** The backend now stores FCM tokens, mints OAuth2 credentials, and pushes notifications on task events — without ever blocking an HTTP response.
