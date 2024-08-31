use anyhow::Context;
use axum::{routing::*, Extension};
use dioxus::prelude::*;
use std::ops::Deref;
use std::sync::Arc;
use tower_sessions::SessionManagerLayer;
use tower_sessions_sqlx_store::SqliteStore;
use tracing::info;

use axum::{
    http,
    response::{IntoResponse, Response},
};

use crate::app;

pub mod google;
pub mod user;

#[repr(transparent)]
pub struct Session(pub tower_sessions::Session);

impl Deref for Session {
    type Target = tower_sessions::Session;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub struct Config {
    pub url_prefix: String,
}

#[derive(Debug)]
pub struct SessionLayerNotFound;

impl std::fmt::Display for SessionLayerNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AuthSessionLayer was not found")
    }
}

impl std::error::Error for SessionLayerNotFound {}

impl IntoResponse for SessionLayerNotFound {
    fn into_response(self) -> Response {
        (
            http::status::StatusCode::INTERNAL_SERVER_ERROR,
            "SessionLayer was not found",
        )
            .into_response()
    }
}

#[async_trait::async_trait]
impl<S: std::marker::Sync + std::marker::Send> axum::extract::FromRequestParts<S> for Session {
    type Rejection = SessionLayerNotFound;

    async fn from_request_parts(
        parts: &mut http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        tower_sessions::Session::from_request_parts(parts, state)
            .await
            .map(Session)
            .map_err(|_| SessionLayerNotFound)
    }
}

pub fn run() -> anyhow::Result<()> {
    let url_prefix =
        std::env::var("URL_PREFIX").unwrap_or_else(|_| "http://localhost:3000".to_string());

    let config = Arc::new(Config { url_prefix });

    tokio::runtime::Runtime::new()
        .context("Failed to init tokio runtime")?
        .block_on(async move {
            let api_secret = google_calendar3::oauth2::read_application_secret("google.json")
                .await
                .context("Failed to read google api secret")?;
            let service_account = Arc::new(
                google_calendar3::oauth2::read_service_account_key("service_account.json")
                    .await
                    .context("Failed to read google service account config")?,
            );

            let db_pool = sqlx::SqlitePool::connect("./db.db").await?;
            sqlx::migrate!().run(&db_pool).await?;

            let session_store = SqliteStore::new(db_pool.clone());
            session_store.migrate().await?;

            info!("DB migration completed");

            // build our application with some routes
            let app = Router::new()
                .nest("/user", user::web_router(api_secret))
                .nest("/google", google::web_router())
                // Server side render the application, serve static assets, and register server functions
                .serve_dioxus_application(ServeConfig::builder().build(), || VirtualDom::new(app))
                .await
                .layer(
                    SessionManagerLayer::new(session_store)
                        .with_secure(config.url_prefix.starts_with("https")),
                )
                .layer(Extension(config))
                .layer(Extension(db_pool))
                .layer(Extension(service_account));

            // run it
            let addr = std::net::SocketAddr::from(([0, 0, 0, 0], 3000));
            let listener = tokio::net::TcpListener::bind(&addr).await?;

            axum::serve(listener, app.into_make_service()).await?;

            anyhow::Ok(())
        })?;

    Ok(())
}
