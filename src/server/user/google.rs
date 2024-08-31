use super::UserSession;
use crate::{
    server::user::UserKey,
    tracing::{debug, error, info},
    wrap_error_async, Config,
};
use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    pin::Pin,
    sync::Arc,
};

use anyhow::{anyhow, Context};
use axum::{
    extract::Query,
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Extension,
};
use keyring::Keyring;
use sqlx::SqlitePool;
use tokio::sync::{oneshot, Mutex, RwLock};
use tower_sessions::Session;
use uuid::Uuid;

use google_calendar3::oauth2::{
    self, authenticator_delegate::InstalledFlowDelegate, ApplicationSecret,
};

use super::UserId;

mod keyring;

const CALENDAR_SCOPE: &[&str] = &["openid"];

#[repr(transparent)]
#[derive(Debug, Clone)]
struct LoginCallbackCode(String);

#[repr(transparent)]
#[derive(Debug, Clone)]
struct RedirectUrl(String);

type LoginContextMap = HashMap<
    Uuid,
    (
        oneshot::Sender<LoginCallbackCode>,
        oneshot::Receiver<Option<UserId>>,
    ),
>;

struct LoginDelegate {
    channels: Mutex<
        Option<(
            oneshot::Sender<RedirectUrl>,
            oneshot::Receiver<LoginCallbackCode>,
        )>,
    >,
    redirect_uri: String,
    context_id: Uuid,
}

impl InstalledFlowDelegate for LoginDelegate {
    fn redirect_uri(&self) -> Option<&str> {
        Some(&self.redirect_uri)
    }

    fn present_user_url<'a>(
        &'a self,
        url: &'a str,
        _need_code: bool,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            let (redirect_url_sender, code_receiver) = self
                .channels
                .lock()
                .await
                .take()
                .ok_or_else(|| "already used".to_string())?;
            if let Err(e) =
                redirect_url_sender.send(RedirectUrl(format!("{url}&state={}", self.context_id)))
            {
                Err(format!("Failed to send redirect URL - {:?}", e))
            } else {
                let code = code_receiver
                    .await
                    .map_err(|e| format!("Failed to receive auth code - {:?}", e))?;

                debug!("code received");

                Ok(code.0)
            }
        })
    }
}

async fn begin_login(
    session: Session,
    Extension(db): Extension<SqlitePool>,
    Extension(contexts): Extension<Arc<Mutex<LoginContextMap>>>,
    Extension(storage): Extension<Arc<GoogleAuthStorage>>,
    Extension(config): Extension<Arc<Config>>,
) -> Response {
    if let Ok(Some(session)) = session.get::<UserSession>(UserSession::SESSION_KEY).await {
        debug!("Already logged in redirect to main - {session:?}");
        return Redirect::to(&config.url_prefix).into_response();
    }

    let (url_sender, url_receiver) = oneshot::channel();
    let (code_sender, code_receiver) = oneshot::channel();
    let (user_id_sender, user_id_receiver) = oneshot::channel();

    let id = Uuid::new_v4();
    contexts
        .lock()
        .await
        .insert(id, (code_sender, user_id_receiver));

    tokio::spawn(async move {
        let user_id = wrap_error_async(async move {
            let auth = oauth2::InstalledFlowAuthenticator::builder(
                storage.api_secret.clone(),
                oauth2::InstalledFlowReturnMethod::Interactive,
            )
            .flow_delegate(Box::new(LoginDelegate {
                channels: Mutex::new(Some((url_sender, code_receiver))),
                redirect_uri: format!("{}/user/google/callback", config.url_prefix),
                context_id: id,
            }))
            .build()
            .await
            .context("Failed to installed flow")?;

            let subject = {
                use jwt::VerifyWithStore;

                let id_token = auth
                    .id_token(CALENDAR_SCOPE)
                    .await
                    .context("Failed to get id_token")?
                    .ok_or_else(|| anyhow!("id_token is empty"))?;
                let mut claims: BTreeMap<String, serde_json::Value> = if let Ok(claims) = {
                    let keyring = storage.keyring.read().await;
                    id_token.verify_with_store(&*keyring)
                } {
                    claims
                } else {
                    let mut keyring = storage.keyring.write().await;
                    keyring.fetch().await.context("Failed to update certs")?;
                    let keyring = keyring.downgrade();
                    id_token
                        .verify_with_store(&*keyring)
                        .context("jwt verification failed")?
                };

                claims
                    .remove("sub")
                    .context("sub is not in received claims")?
            };

            auth.token(CALENDAR_SCOPE)
                .await
                .context("Failed to get access token")?;

            info!("Login succeed");
            let subject = subject
                .as_str()
                .context("received subject in claims is not string")?;

            let user_info = sqlx::query!(
                "SELECT `user_id` as `user_id:UserId` FROM `google_user` WHERE `subject` = ?",
                subject
            )
            .fetch_optional(&db)
            .await
            .context("Failed to query logged in user")?
            .map(|record| record.user_id);

            let user_id = if let Some(user_id) = user_info {
                user_id
            } else {
                let user_id = UserId(
                    sqlx::query!("INSERT INTO `user` (`dummy`) VALUES (0)")
                        .execute(&db)
                        .await
                        .context("Failed to insert new user")?
                        .last_insert_rowid() as _,
                );

                let minimum_date_time = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
                    chrono::NaiveDateTime::MIN,
                    chrono::Utc,
                );
                sqlx::query!(
                    r#"INSERT INTO `google_user`
                    (`user_id`, `calendar_id`, `last_synced`, `subject`)
                    VALUES
                    (?, "", ?, ?)"#,
                    user_id,
                    minimum_date_time,
                    subject
                )
                .execute(&db)
                .await
                .context("Failed to insert into google_user")?;

                user_id
            };

            Ok(user_id)
        })
        .await;

        if let Err(_) = user_id_sender.send(user_id) {
            error!("Failed to send user_id to callback handler");
        }
    });

    let url = url_receiver.await.unwrap();
    Redirect::to(&url.0).into_response()
}

#[derive(serde::Deserialize)]
struct LoginCallbackQuery {
    state: Uuid,
    code: String,
    #[allow(dead_code)]
    scope: String,
}

async fn login_callback(
    session: Session,
    Extension(db): Extension<SqlitePool>,
    Extension(contexts): Extension<Arc<Mutex<LoginContextMap>>>,
    Query(query): Query<LoginCallbackQuery>,
) -> Response {
    if let Some((code_sender, user_id_receiver)) = contexts.lock().await.remove(&query.state) {
        debug!("found context");
        if let Err(e) = code_sender.send(LoginCallbackCode(query.code)) {
            error!("Failed to send auth code - {e:?}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        let Ok(user_id) = user_id_receiver
            .await
            .map_err(|e| error!("Failed to receive logged in user id - {e:?}"))
        else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        if let Some(user_id) = user_id {
            debug!("Successfully logged in");

            let Ok(key_chain_ret) = sqlx::query!(
                "SELECT count(*) as `count` FROM `keychain` WHERE `user_id` = ?",
                user_id
            )
            .fetch_optional(&db)
            .await
            .map_err(|e| error!("Failed to check keychain - {e:?}")) else {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            };

            if let Err(e) = session
                .insert(
                    UserSession::SESSION_KEY,
                    UserSession {
                        user_id,
                        key: match key_chain_ret {
                            Some(_) => UserKey::Locked(0),
                            None => UserKey::NotExist,
                        },
                        key_pair: None,
                    },
                )
                .await
            {
                error!("Failed to insert user_id into session - {e:?}");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            Redirect::to("/").into_response()
        } else {
            debug!("Not allowed");
            StatusCode::FORBIDDEN.into_response()
        }
    } else {
        debug!("Invalid request");
        StatusCode::BAD_REQUEST.into_response()
    }
}

struct GoogleAuthStorage {
    api_secret: ApplicationSecret,
    keyring: RwLock<Keyring>,
}

pub fn web_router<S: Sync + Send + Clone + 'static>(
    api_secret: ApplicationSecret,
) -> axum::Router<S> {
    let login_contexts = Arc::new(Mutex::new(LoginContextMap::new()));
    // TODO read api secret
    let storage = Arc::new(GoogleAuthStorage {
        api_secret,
        keyring: RwLock::new(Keyring::default()),
    });
    axum::Router::new()
        .route("/login", get(begin_login))
        .route("/callback", get(login_callback))
        .layer(Extension(login_contexts))
        .layer(Extension(storage))
}
