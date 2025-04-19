use anyhow::Context;
use axum::{routing::*, Extension};
use dioxus::prelude::*;
use server_fn::error::NoCustomError;
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

pub mod reservation;

pub(crate) mod prelude {
    pub(crate) mod common {
        #![allow(unused_imports)]
        pub(crate) use crate::server::sqlx_error_to_dioxus_error;
        pub(crate) use axum::Extension;
        pub(crate) use dioxus::prelude::server_fn::error::NoCustomError;
        pub(crate) use sqlx::{query, SqlitePool};
    }
    pub(crate) mod user {
        #![allow(unused_imports)]
        pub(crate) use crate::server::{
            user::{UserId, UserKey, UserSession},
            Session,
        };
    }
    pub(crate) mod crypto {
        #![allow(unused_imports)]
        pub(crate) use aead::{
            generic_array::sequence::GenericSequence,
            rand_core::{OsRng, RngCore},
            Aead, NewAead,
        };
        pub(crate) use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
    }

    pub(crate) mod reservation {
        #![allow(unused_imports)]
        pub(crate) use crate::server::{reservation::*, USER_AGENT};
        pub(crate) use crate::{define_user_data, regex, selector, url};
        pub(crate) use reqwest::{
            cookie::{CookieStore as _, Jar},
            Client,
        };
        pub(crate) use scraper::Html;
    }
}

pub mod user;

#[macro_export]
macro_rules! define_user_data {
    (
        $(#[domain = $domain:literal])?
        #[base_url = $url:literal]
        struct $name:ident(
            $(
                $session_name:literal
            ),+
        )
    ) => {
        pub struct $name;

        impl $name {
            #[allow(dead_code)]
            pub fn from_chrome_tab(tab: &std::sync::Arc<headless_chrome::Tab>) -> anyhow::Result<reqwest::cookie::Jar> {
                let cookies = tab.get_cookies()?;
                let endpoint_base = crate::url!($url);

                let jar = reqwest::cookie::Jar::default();
                for cookie in cookies {
                    tracing::info!("cookie {}={}", cookie.name, cookie.value);
                    let Some((name, value)) = (match cookie.name.as_str() {
                        $(
                            $session_name => Some(($session_name, cookie.value)),
                        )+
                        _ => None,
                    }) else {
                        continue;
                    };
                    tracing::info!("cookie add");
                    jar.add_cookie_str(&format!("{}={}", name, value), endpoint_base);
                }

                Ok(jar)
            }

            #[allow(dead_code)]
            pub fn from_iter(values: impl Iterator<Item = impl AsRef<str>>) -> anyhow::Result<reqwest::cookie::Jar> {
                let endpoint_base = crate::url!($url);

                let jar = reqwest::cookie::Jar::default();
                for (key, value) in [$($session_name,)+].iter().zip(values) {
                    let value = value.as_ref();
                    let cookie = define_user_data!(@render_cookie key, value $((domain = $domain))?);
                    jar.add_cookie_str(&cookie, endpoint_base);
                }

                Ok(jar)
            }
        }
    };
    (@render_cookie $key:ident, $value:ident (domain = $domain:literal)) => {
        format!("{}={}; Domain={}", $key, $value, $domain)
    };
    (@render_cookie $key:ident, $value:ident ) => {
        format!("{}={}", $key, $value)
    };
}

pub const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/15.5 Safari/605.1.15";

#[macro_export]
macro_rules! selector {
    ($selector:literal) => {{
        static SELECTOR: once_cell::sync::OnceCell<scraper::Selector> =
            once_cell::sync::OnceCell::new();
        SELECTOR.get_or_init(|| scraper::Selector::parse($selector).unwrap())
    }};
}

#[macro_export]
macro_rules! url {
    ($url:literal) => {{
        static URL: once_cell::sync::OnceCell<reqwest::Url> = once_cell::sync::OnceCell::new();
        URL.get_or_init(|| <reqwest::Url as std::str::FromStr>::from_str($url).unwrap())
    }};
}

#[macro_export]
macro_rules! regex {
    ($regex:literal) => {{
        static REGEX: once_cell::sync::OnceCell<regex::Regex> = once_cell::sync::OnceCell::new();
        REGEX.get_or_init(|| regex::Regex::new($regex).unwrap())
    }};
}

pub fn sqlx_error_to_dioxus_error(error: sqlx::Error) -> ServerFnError {
    tracing::error!("Failed while executing SQL - {error:?}");

    ServerFnError::<NoCustomError>::ServerError("Internal Server Error".to_string())
}

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
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
            
            let api_secret = google_calendar3::yup_oauth2::read_application_secret("google.json")
                .await
                .context("Failed to read google api secret")?;
            let service_account = Arc::new(
                google_calendar3::yup_oauth2::read_service_account_key("service_account.json")
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
