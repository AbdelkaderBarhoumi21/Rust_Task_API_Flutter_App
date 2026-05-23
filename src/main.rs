mod devices;
mod fcm;
mod handlers;
mod models;

use axum::{
    routing::{delete, get, post},
    Router,
};
use dotenv::dotenv;
use sqlx::postgres::PgPoolOptions;
use std::env;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tracing::info;
use tracing_subscriber::EnvFilter;

use fcm::FcmClient;

#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::PgPool,
    pub fcm: Arc<FcmClient>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let database_url = env::var("DATABASE_URL").map_err(|_| "DATABASE_URL must be set")?;
    let project_id =
        env::var("FIREBASE_PROJECT_ID").map_err(|_| "FIREBASE_PROJECT_ID must be set")?;
    let sa_path = env::var("FIREBASE_SERVICE_ACCOUNT_PATH")
        .map_err(|_| "FIREBASE_SERVICE_ACCOUNT_PATH must be set")?;

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    // Ensure the database schema is ready on startup.
    sqlx::migrate!("./migrations").run(&pool).await?;

    let fcm = FcmClient::new(project_id, &sa_path).await?;
    let state = AppState { pool, fcm };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/tasks", get(handlers::get_tasks).post(handlers::create_task))
        .route(
            "/tasks/:id",
            get(handlers::get_task)
                .put(handlers::update_task)
                .delete(handlers::delete_task),
        )
        .route("/devices", post(devices::register_device))
        .route("/devices/:id", delete(devices::delete_device))
        .with_state(state)
        .layer(cors);

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], 3000));
    info!("listening on {}", addr);
    info!("base url: http://localhost:3000");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
