#![allow(non_snake_case)]
use std::{collections::BTreeMap, future::Future, ops::Deref};

pub(crate) mod prelude {
    #![allow(unused_imports)]
    pub(crate) use super::{flatten_error, wrap_error_async};
    pub(crate) use anyhow::Context as _;
    pub(crate) use dioxus_logger::tracing::{debug, error, info, warn};
}

use chrono::Utc;
use dioxus::prelude::*;
use pages::vault::VaultKey;
use prelude::*;

mod pages;
mod user;

#[cfg(feature = "server")]
mod server;

pub(crate) fn flatten_error<T, E>(r: Result<Result<T, E>, E>) -> Result<T, E> {
    r?
}

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

pub type VaultContext = Resource<BTreeMap<VaultKey, chrono::DateTime<Utc>>>;

#[cfg(any(feature = "web", feature = "server"))]
fn app() -> Element {
    let base_url = use_server_future(|| async move { service_base_url().await.unwrap() })?;
    let user = use_resource(|| async move { pages::get_user_info().await.unwrap_or_default() });
    let vault = use_resource({
        let user = user.clone();
        move || async move {
            if user.as_ref().map(|u| u.is_signed_in()).unwrap_or_default() {
                pages::source::list_sources().await.unwrap_or_default()
            } else {
                Default::default()
            }
        }
    });

    use_context_provider(|| user);
    use_context_provider(|| vault);
    use_context_provider(|| base_url);

    rsx! {
        link {
            rel: "stylesheet",
            href: "https://cdn.jsdelivr.net/npm/bulma@1.0.2/css/bulma.min.css",
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
