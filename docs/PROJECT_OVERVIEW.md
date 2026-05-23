# Task API — Project Overview (FCM Integration Handoff)

This document is a compact handoff for an AI agent that will design Firebase Cloud Messaging (FCM) integration for this project. It captures the stack, structure, data model, current endpoints, and the touch points where push-notification logic would plug in.

---

## 1. Project Summary

- **Name:** `task_api`
- **Language / Edition:** Rust (edition `2024`)
- **Type:** RESTful HTTP API for managing tasks (CRUD)
- **Web framework:** [Axum](https://docs.rs/axum) `0.7`
- **Async runtime:** [Tokio](https://docs.rs/tokio) `1.36` (multi-thread)
- **Database:** PostgreSQL via [SQLx](https://docs.rs/sqlx) `0.7` (compile-time-checked queries, migrations)
- **Server bind:** `0.0.0.0:3000` (base URL: `http://localhost:3000`)
- **CORS:** open (`Any` origin / methods / headers)
- **Logging:** `tracing` + `tracing-subscriber` with `EnvFilter` (default `info`)
- **Config:** `.env` via `dotenv`

No authentication, no user accounts, no device-token storage **yet** — all of this needs to be added for FCM.

---

## 2. File & Folder Structure

```
task_api/
├── Cargo.toml                       # Dependencies & crate metadata
├── Cargo.lock
├── .env                             # DATABASE_URL (local dev)
├── API_ENDPOINTS.md                 # Existing REST API documentation
├── migrations/
│   └── 001_create_tasks_table.sql   # Initial schema (enums + tasks table)
└── src/
    ├── main.rs                      # Entry point: env, pool, migrations, router, CORS, serve
    ├── models.rs                    # Domain types: Task, enums, request DTOs
    └── handlers.rs                  # Axum handlers + AppError
```

There is **no** `lib.rs`, no module subfolders, no tests directory, no Dockerfile, and no CI configuration in this repo.

---

## 3. Dependencies (`Cargo.toml`)

```toml
[package]
name = "task_api"
version = "0.1.0"
edition = "2024"

[dependencies]
axum = "0.7"
tokio = { version = "1.36", features = ["macros", "rt-multi-thread"] }
sqlx = { version = "0.7", features = ["runtime-tokio", "postgres", "chrono", "uuid", "macros", "migrate"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1.7", features = ["serde", "v4"] }
tower-http = { version = "0.5", features = ["cors"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
dotenv = "0.15"
```

**Not yet present (likely needed for FCM):**
- HTTP client for calling FCM HTTP v1 API (e.g. `reqwest`)
- Google service-account auth / OAuth2 token mint (e.g. `yup-oauth2`, `gcp_auth`, or a community FCM crate)
- JSON Web Token / JWT lib if signing service-account credentials manually (`jsonwebtoken`)

---

## 4. Database Schema

**File:** [migrations/001_create_tasks_table.sql](migrations/001_create_tasks_table.sql)

```sql
CREATE TYPE task_priority AS ENUM ('low', 'medium', 'high');
CREATE TYPE task_status   AS ENUM ('pending', 'in_progress', 'completed');

CREATE TABLE tasks (
    id           UUID PRIMARY KEY,
    title        TEXT NOT NULL,
    description  TEXT NOT NULL,
    priority     task_priority NOT NULL,
    status       task_status   NOT NULL,
    created_at   TIMESTAMPTZ   NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ
);

CREATE INDEX idx_tasks_status   ON tasks (status);
CREATE INDEX idx_tasks_priority ON tasks (priority);
```

Migrations are auto-run on startup via `sqlx::migrate!("./migrations").run(&pool)` in [src/main.rs:31](src/main.rs#L31).

**For FCM**, expect to add a new migration (e.g. `002_create_devices_table.sql`) for storing device/FCM registration tokens — there is currently nowhere to send pushes *to*.

---

## 5. Domain Models

**File:** [src/models.rs](src/models.rs)

- `TaskPriority` — enum `Low | Medium | High` (Postgres enum `task_priority`, lowercase). Serialized as camelCase JSON.
- `TaskStatus` — enum `Pending | InProgress | Completed` (Postgres enum `task_status`, snake_case). Serialized as camelCase JSON.
- `Task` — main row struct: `id, title, description, priority, status, created_at, completed_at` (`completed_at` is `Option<DateTime<Utc>>`).
- `CreateTaskRequest` — `title, description, priority`, optional `status`.
- `UpdateTaskRequest` — every field optional (partial update).

> Note: `serde(rename_all = "camelCase")` is set on the structs, so JSON keys are `createdAt` / `completedAt`, even though `API_ENDPOINTS.md` shows `created_at` / `completed_at`. The code is the source of truth.

---

## 6. Application Wiring

**File:** [src/main.rs](src/main.rs)

```rust
#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::PgPool,
}
```

Startup sequence:
1. Load `.env` (`dotenv().ok()`).
2. Init `tracing-subscriber` with `EnvFilter` (default `info`).
3. Read `DATABASE_URL`, build a `PgPool` (`max_connections = 5`).
4. Run SQL migrations from `./migrations`.
5. Build `CorsLayer` (allow any origin/methods/headers).
6. Build router and serve on `0.0.0.0:3000`.

Routes wired in [src/main.rs:40-49](src/main.rs#L40-L49):

| Method | Path          | Handler                  |
|--------|---------------|--------------------------|
| GET    | `/tasks`      | `handlers::get_tasks`    |
| POST   | `/tasks`      | `handlers::create_task`  |
| GET    | `/tasks/:id`  | `handlers::get_task`     |
| PUT    | `/tasks/:id`  | `handlers::update_task`  |
| DELETE | `/tasks/:id`  | `handlers::delete_task`  |

---

## 7. Handlers / Error Model

**File:** [src/handlers.rs](src/handlers.rs)

- `AppError` enum: `NotFound | Db(sqlx::Error)` with a manual `IntoResponse` impl.
  - `NotFound` → `404` + `{"message": "Task not found"}`
  - `Db(_)` → logged via `tracing::error!` and returned as `500` + `{"message": "Internal server error"}`
- All handlers take `State<AppState>` for the connection pool and return `Result<_, AppError>`.
- `create_task`: generates `Uuid::new_v4()`; if `status == Completed`, sets `completed_at = Utc::now()`.
- `update_task`: fetches existing row first (returns `NotFound` if missing), then merges optional fields and **maintains the `completed_at` invariant**:
  - moving *into* `Completed` ⇒ stamp `completed_at = now()` (only if not already completed).
  - moving *out of* `Completed` (when `status` is explicitly set to something else) ⇒ clear `completed_at`.
  - status not provided ⇒ keep existing `completed_at`.
- `delete_task`: returns `204` on success, `404` if no row affected.

---

## 8. REST Contract (current)

See [API_ENDPOINTS.md](API_ENDPOINTS.md) for the full doc. Quick reference:

- `GET    /tasks`        → `200` `Task[]` (newest first)
- `GET    /tasks/:id`    → `200` `Task` | `404`
- `POST   /tasks`        → `201` `Task`
- `PUT    /tasks/:id`    → `200` `Task` | `404`
- `DELETE /tasks/:id`    → `204` | `404`

Errors: `404` not found, `500` internal.

---

## 9. Configuration

**File:** `.env`

```
DATABASE_URL=postgresql://postgres:123456@localhost:5432/task_db
```

No FCM-related env vars exist yet. The integration will most likely need (depending on chosen approach):
- `FIREBASE_PROJECT_ID`
- `FIREBASE_SERVICE_ACCOUNT_JSON` (path or inline) — for FCM HTTP v1 API + OAuth2
- Optionally `FCM_SERVER_KEY` if using the legacy HTTP API (deprecated; not recommended)

---

## 10. How to Run (current)

```powershell
# 1. Ensure Postgres is running and `task_db` exists
# 2. .env points at it
cargo run
# server listens on http://localhost:3000
```

Migrations run automatically on boot; no manual `sqlx-cli` step is required.

---

## 11. FCM Integration — Touch Points / Suggested Additions

These are the natural places an FCM integration would slot in. The agent should treat them as guidance, not constraints.

### New module(s) under `src/`
- `src/fcm.rs` (or `src/notifications/mod.rs`) — wraps the FCM HTTP v1 client: build access token from a service account, send `Message` payloads to a token / topic.
- `src/devices.rs` — handlers for device-token registration (`POST /devices`, `DELETE /devices/:id`), backed by a new `devices` table.

### New migration
- `migrations/002_create_devices_table.sql` — store FCM registration tokens (e.g. `id UUID`, `token TEXT UNIQUE`, `platform TEXT`, optional `user_id`, `created_at`, `last_seen_at`). Without auth there is no per-user scoping yet — consider whether this is needed.

### `AppState` changes ([src/main.rs:12-15](src/main.rs#L12-L15))
- Add an `fcm: Arc<FcmClient>` (or similar) so handlers can fire pushes without rebuilding auth on every request.

### Where to trigger pushes (in [src/handlers.rs](src/handlers.rs))
- `create_task` — push on new task ("New task created: …").
- `update_task` — push when `status` transitions to `Completed`, or on priority change.
- `delete_task` — optional.

These should be **fire-and-forget** (e.g. `tokio::spawn`) so notification failures don't fail the HTTP request, but errors still logged via `tracing::error!`.

### Recommended FCM approach
- **FCM HTTP v1 API** (`https://fcm.googleapis.com/v1/projects/{project_id}/messages:send`) using a Google service account.
- Mint OAuth2 access tokens with `yup-oauth2` or `gcp_auth`; cache until expiry.
- Send via `reqwest` with `Authorization: Bearer <token>`.
- Avoid the legacy server-key API.

### Things to confirm with the user
- Is there going to be authentication / multi-user, or is this single-tenant? (Affects whether `devices` is global or per-user.)
- Topic-based pushes vs. token-based vs. both?
- Which task lifecycle events should trigger notifications?
- Should pushes be synchronous (block the request) or async/background?

---

## 12. Things the Agent Should Know but Aren't Code

- The repo has `target/` (build artifacts) — ignore for design purposes.
- There is **no test suite** in the project right now.
- There is **no Dockerfile, CI, or deploy config**.
- Git: single `main` branch, one commit so far (`Implement Axum task API with Postgres, migrations, and logging`).
- The Postgres enums (`task_priority`, `task_status`) are created by migration `001`. If FCM adds enums (e.g. `device_platform`), use a new migration — never edit `001`.

---

## 13. Quick File Index (for the agent)

| File                                       | Purpose                                       |
|--------------------------------------------|-----------------------------------------------|
| [Cargo.toml](Cargo.toml)                   | Crate manifest / dependencies                  |
| [.env](.env)                               | Local DB connection string                     |
| [migrations/001_create_tasks_table.sql](migrations/001_create_tasks_table.sql) | Initial schema |
| [src/main.rs](src/main.rs)                 | Entry point, router, pool, migrations          |
| [src/models.rs](src/models.rs)             | Domain types & DTOs                            |
| [src/handlers.rs](src/handlers.rs)         | HTTP handlers, AppError                        |
| [API_ENDPOINTS.md](API_ENDPOINTS.md)       | Existing REST API docs                         |
| [PROJECT_OVERVIEW.md](PROJECT_OVERVIEW.md) | This file                                      |
