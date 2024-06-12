use anyhow::Result;
use axum::{
    extract::{Path, State},
    http::{header::LOCATION, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    serve, Json, Router,
};
use nanoid::nanoid;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use std::sync::Arc;
use thiserror::Error;
use tokio::net::TcpListener;
use tracing::{info, level_filters::LevelFilter, warn};
use tracing_subscriber::{
    fmt::Layer, layer::SubscriberExt as _, util::SubscriberInitExt as _, Layer as _,
};

#[derive(Debug)]
struct HttpServeState {
    db: PgPool,
}

#[derive(Debug, Deserialize)]
struct RequestBody {
    url: String,
}

#[derive(Debug, Serialize)]
struct ResponseBody {
    url: String,
}

#[derive(Debug, FromRow)]
struct ShortenedUrl {
    #[sqlx(default)]
    id: String,
    #[sqlx(default)]
    url: String,
}

#[derive(Debug, Error)]
#[error("{0}")]
struct CreateShortUrlFailed(anyhow::Error);

#[derive(Debug, Error)]
#[error("{0}")]
struct GetUrlFailed(anyhow::Error);

#[derive(Debug, Error)]
enum ShortenerError {
    #[error("Not found, id: {0}")]
    NotFound(String),
    #[error("Create shorten url failed: {0}")]
    CreateShortUrlFailed(#[from] CreateShortUrlFailed),
    #[error("Get url failed: {0}")]
    GetUrlFailed(#[from] GetUrlFailed),
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    code: u16,
    message: String,
}

const LISTEN_ADDR: &str = "0.0.0.0:4321";

#[tokio::main]
async fn main() -> Result<()> {
    let layer = Layer::new().with_filter(LevelFilter::INFO);
    tracing_subscriber::registry().with(layer).init();

    let addr = LISTEN_ADDR;
    let listener = TcpListener::bind(addr).await?;
    info!("Listening on: {}", addr);

    let db_url = "postgresql://localhost/shortener";
    let state = HttpServeState::try_new(db_url).await?;
    info!("Database connected: {}", db_url);

    let router = Router::new()
        .route("/", post(create_url))
        .route("/:id", get(redirect))
        .with_state(Arc::new(state));

    serve(listener, router.into_make_service()).await?;

    Ok(())
}

async fn create_url(
    State(state): State<Arc<HttpServeState>>,
    Json(body): Json<RequestBody>,
) -> Result<impl IntoResponse, ShortenerError> {
    let id = state
        .create_shortened_url(&body.url)
        .await
        .map_err(CreateShortUrlFailed)?;

    Ok((StatusCode::CREATED, Json(ResponseBody::new(id))))
}

async fn redirect(
    State(state): State<Arc<HttpServeState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ShortenerError> {
    let url = state.get_url(&id).await.map_err(GetUrlFailed)?;

    let url = url.ok_or(ShortenerError::NotFound(id))?;

    let mut header = HeaderMap::new();
    header.append(LOCATION, url.parse().unwrap());

    Ok((StatusCode::FOUND, header))
}

impl HttpServeState {
    async fn try_new(url: &str) -> Result<Self> {
        let db = PgPool::connect(url).await?;
        // create table if not exists
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS urls (
                id CHAR(6) PRIMARY KEY,
                url TEXT NOT NULL UNIQUE
            )
            "#,
        )
        .execute(&db)
        .await?;

        Ok(Self { db })
    }

    async fn find_new_id(&self) -> Result<String> {
        let mut id = nanoid!(6);
        while sqlx::query("SELECT id FROM urls WHERE id = $1")
            .bind(&id)
            .fetch_optional(&self.db)
            .await?
            .is_some()
        {
            id = nanoid!(6);
        }
        info!("New id found: {}", id);

        Ok(id)
    }

    async fn insert_url(&self, id: &str, url: &str) -> Result<String> {
        let ret: ShortenedUrl = sqlx::query_as(
            "INSERT INTO urls (id, url) VALUES ($1, $2) ON CONFLICT(url) DO UPDATE SET url=EXCLUDED.url RETURNING id",
        ).bind(id).bind(url).fetch_one(&self.db).await?;

        Ok(ret.id)
    }

    async fn create_shortened_url(&self, url: &str) -> Result<String> {
        let id = nanoid!(6);
        let ret: Result<String> = self.insert_url(&id, url).await;

        match ret {
            Ok(ret) => Ok(ret),
            Err(e) => {
                warn!("Create shortened url failed: {}", e);
                let id = self.find_new_id().await?;
                self.insert_url(&id, url).await
            }
        }
    }

    async fn get_url(&self, id: &str) -> Result<Option<String>> {
        let ret: Option<ShortenedUrl> = sqlx::query_as("SELECT * FROM urls WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.db)
            .await?;

        Ok(ret.map(|url| url.url))
    }
}

impl ResponseBody {
    fn new(id: String) -> Self {
        Self {
            url: format!("http://{}/{}", LISTEN_ADDR, id),
        }
    }
}

impl ErrorResponse {
    fn new(code: u16, message: String) -> Self {
        Self { code, message }
    }

    fn create_short_url_failed() -> Self {
        Self::new(1, "Create short url failed".to_string())
    }

    fn get_url_failed() -> Self {
        Self::new(2, "Get url failed".to_string())
    }
}

impl IntoResponse for ShortenerError {
    fn into_response(self) -> axum::http::Response<axum::body::Body> {
        warn!("{}", self);
        match self {
            ShortenerError::NotFound(_) => StatusCode::NOT_FOUND.into_response(),
            ShortenerError::CreateShortUrlFailed(_) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(ErrorResponse::create_short_url_failed()),
            )
                .into_response(),
            Self::GetUrlFailed(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse::get_url_failed()),
            )
                .into_response(),
        }
    }
}
