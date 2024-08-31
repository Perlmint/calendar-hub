#![allow(non_snake_case)]

use std::{future::Future, ops::Deref};

use dioxus::prelude::*;
use dioxus_logger::tracing::{error, info};
pub(crate) mod tracing {
    pub use dioxus_logger::tracing::*;
}

mod pages;
mod user;

#[cfg(feature = "server")]
mod server;
#[cfg(feature = "server")]
pub use server::*;

pub(crate) async fn wrap_error_async<T>(f: impl Future<Output = anyhow::Result<T>>) -> Option<T> {
    match f.await {
        Err(e) => {
            error!("{e}");
            None
        }
        Ok(v) => Some(v),
    }
}

fn main() {
    // Init logger
    dioxus_logger::init(tracing::Level::DEBUG).expect("failed to init logger");
    tracing::info!("starting app");

    #[cfg(feature = "web")]
    // Hydrate the application on the client
    dioxus_web::launch::launch_cfg(app, dioxus_web::Config::new().hydrate(true));

    #[cfg(feature = "server")]
    server::run().unwrap();
}

#[cfg(any(feature = "web", feature = "server"))]
fn app() -> Element {
    let base_url = use_server_future(|| async move {
        service_base_url().await.unwrap()
    })?;
    let user = use_resource(|| async move { pages::UserInfo().await.unwrap_or_default() });

    use_context_provider(|| {
        info!("user: {:?}", user.value());
        user.value()
    });
    use_context_provider(|| base_url);

    rsx! {
        link {
            rel: "stylesheet",
            href: "https://cdn.jsdelivr.net/npm/@picocss/pico@2/css/pico.min.css",
        }
        Router::<pages::Route> {}
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[repr(transparent)]
pub struct BaseUrl(pub String);

impl AsRef<String> for BaseUrl {
    fn as_ref(&self) -> &String {
        &self.0
    }
}

impl Deref for BaseUrl {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::fmt::Display for BaseUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[server]
pub async fn service_base_url() -> Result<BaseUrl, ServerFnError> {
    let axum::Extension(config): axum::Extension<std::sync::Arc<server::Config>> =
        extract().await?;

    Ok(BaseUrl(config.url_prefix.clone()))
}
