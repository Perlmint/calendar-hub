use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    pin::Pin,
    sync::Arc,
};

use axum::{
    body::HttpBody,
    extract::Query,
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Extension,
};
use axum_sessions::extractors::{ReadableSession, WritableSession};
use chrono::{NaiveDate, NaiveDateTime, NaiveTime, Utc};
use hyper::StatusCode;
use jwt::VerifyingAlgorithm;
use log::error;
use rsa::{pkcs8::AssociatedOid, Pkcs1v15Sign, RsaPublicKey};
use sha2::Digest;
use sqlx::{Row, SqlitePool};

use anyhow::Context;
use google_calendar3::{
    api::{Calendar, Event, EventDateTime},
    hyper, hyper_rustls,
    oauth2::{self, authenticator_delegate::InstalledFlowDelegate},
    CalendarHub,
};
use tokio::sync::{oneshot, Mutex};
use uuid::Uuid;

use crate::{CalendarEvent, UserId};

trait IntoGoogleEventDateTime {
    fn into_google(self) -> EventDateTime;
}

impl IntoGoogleEventDateTime for Option<(NaiveDate, Option<NaiveTime>)> {
    fn into_google(self) -> EventDateTime {
        match self {
            Some(val) => val.into_google(),
            None => EventDateTime {
                date: None,
                date_time: None,
                time_zone: None,
            },
        }
    }
}

impl IntoGoogleEventDateTime for (NaiveDate, Option<NaiveTime>) {
    fn into_google(self) -> EventDateTime {
        if let Some(time) = self.1 {
            EventDateTime {
                date_time: Some(
                    NaiveDateTime::new(self.0, time)
                        .and_local_timezone(Utc)
                        .unwrap(),
                ),
                date: None,
                time_zone: Some("GMT+00:00".to_string()),
            }
        } else {
            EventDateTime {
                date: Some(self.0),
                date_time: None,
                time_zone: Some("GMT+00:00".to_string()),
            }
        }
    }
}

impl From<CalendarEvent> for Event {
    fn from(event: CalendarEvent) -> Event {
        let start = (event.date_begin, event.time_begin).into_google();
        Event {
            description: Some(event.detail),
            end: Some(
                event
                    .date_end
                    .map(|date| (date, event.time_end).into_google())
                    .unwrap_or_else(|| start.clone()),
            ),
            start: Some(start),
            summary: Some(event.title),
            ..Default::default()
        }
    }
}

const CALENDAR_SCOPE: &[&str] = &[
    "https://www.googleapis.com/auth/calendar",
    "https://www.googleapis.com/auth/calendar.events",
    "openid",
];

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
        oneshot::Receiver<UserId>,
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

                Ok(code.0)
            }
        })
    }
}

enum RsAlgorithm {
    Rs256,
    Rs384,
    Rs512,
}
struct RsaVerifying(RsaPublicKey, RsAlgorithm);

impl RsaVerifying {
    fn verify_with_hash<H: Digest + AssociatedOid>(
        &self,
        header: &str,
        claims: &str,
        signature: &[u8],
    ) -> Result<bool, jwt::Error> {
        match self.0.verify(
            Pkcs1v15Sign::new::<H>(),
            {
                let mut hasher = H::new();
                hasher.update(header);
                hasher.update(".");
                hasher.update(claims);
                &hasher.finalize()
            },
            signature,
        ) {
            Ok(()) => Ok(true),
            Err(e) if e == rsa::Error::Verification => Ok(false),
            Err(_) => Err(jwt::Error::InvalidSignature),
        }
    }
}

impl VerifyingAlgorithm for RsaVerifying {
    fn algorithm_type(&self) -> jwt::AlgorithmType {
        match self.1 {
            RsAlgorithm::Rs256 => jwt::AlgorithmType::Rs256,
            RsAlgorithm::Rs384 => jwt::AlgorithmType::Rs384,
            RsAlgorithm::Rs512 => jwt::AlgorithmType::Rs512,
        }
    }

    fn verify_bytes(
        &self,
        header: &str,
        claims: &str,
        signature: &[u8],
    ) -> Result<bool, jwt::Error> {
        match self.1 {
            RsAlgorithm::Rs256 => self.verify_with_hash::<sha2::Sha256>(header, claims, signature),
            RsAlgorithm::Rs384 => self.verify_with_hash::<sha2::Sha384>(header, claims, signature),
            RsAlgorithm::Rs512 => self.verify_with_hash::<sha2::Sha512>(header, claims, signature),
        }
    }
}

#[derive(Clone)]
pub struct Config {
    secret: Arc<oauth2::ApplicationSecret>,
    url_prefix: Arc<String>,
    google_key_store: Arc<BTreeMap<String, RsaVerifying>>,
}

impl Config {
    pub async fn new(
        secret: Arc<oauth2::ApplicationSecret>,
        url_prefix: Arc<String>,
    ) -> anyhow::Result<Self> {
        let google_key_store = fetch_google_key_store().await?;
        Ok(Self {
            secret,
            url_prefix,
            google_key_store: Arc::new(google_key_store),
        })
    }
}

async fn fetch_google_key_store() -> anyhow::Result<BTreeMap<String, RsaVerifying>> {
    #[derive(serde::Deserialize)]
    struct Key {
        n: String,
        e: String,
        kid: String,
        alg: String,
    }
    #[derive(serde::Deserialize)]
    struct R {
        keys: Vec<Key>,
    }
    let resp: R = reqwest::get("https://www.googleapis.com/oauth2/v3/certs")
        .await?
        .json()
        .await?;

    let mut ret = BTreeMap::new();

    for key in resp.keys {
        ret.insert(
            key.kid.to_string(),
            RsaVerifying(
                rsa::RsaPublicKey::new(
                    rsa::BigUint::from_bytes_be(&base64_url::decode(&key.n).unwrap()),
                    rsa::BigUint::from_bytes_be(&base64_url::decode(&key.e).unwrap()),
                )
                .unwrap(),
                match key.alg.as_str() {
                    "RS256" => RsAlgorithm::Rs256,
                    "RS384" => RsAlgorithm::Rs384,
                    "RS512" => RsAlgorithm::Rs512,
                    alg => unreachable!("Invalid algorithm type - {alg}"),
                },
            ),
        );
    }

    Ok(ret)
}

async fn begin_login(
    session: ReadableSession,
    Extension(db): Extension<SqlitePool>,
    Extension(config): Extension<Config>,
    Extension(contexts): Extension<Arc<Mutex<LoginContextMap>>>,
) -> Response {
    if session.get::<UserId>("user_id").is_some() {
        return Redirect::to("://").into_response();
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
        let auth = oauth2::InstalledFlowAuthenticator::builder(
            (*config.secret).clone(),
            oauth2::InstalledFlowReturnMethod::Interactive,
        )
        .flow_delegate(Box::new(LoginDelegate {
            channels: Mutex::new(Some((url_sender, code_receiver))),
            redirect_uri: format!("{}/callback", config.url_prefix),
            context_id: id,
        }))
        .build()
        .await
        .context("Failed to installed flow")
        .unwrap();

        let token = auth
            .token(CALENDAR_SCOPE)
            .await
            .context("Failed to fetch token")
            .unwrap();
        let token = token.token().unwrap();

        let subject = {
            use jwt::VerifyWithStore;

            let id_token = auth.id_token(CALENDAR_SCOPE).await.unwrap().unwrap();
            let mut claims: BTreeMap<String, serde_json::Value> = id_token
                .verify_with_store(&*config.google_key_store)
                .unwrap();
            claims.remove("sub").unwrap()
        };

        let subject = subject.as_str().unwrap();

        let user_id = sqlx::query!(
            "SELECT `user_id` as `user_id:UserId` FROM `google_user` WHERE `subject` = ?",
            subject
        )
        .fetch_optional(&db)
        .await
        .unwrap()
        .map(|record| record.user_id);

        let user_id = if let Some(user_id) = user_id {
            sqlx::query!(
                r#"UPDATE `google_user`
                SET `access_token` = ?
                WHERE `user_id` = ?"#,
                token,
                user_id
            )
            .execute(&db)
            .await
            .with_context(|| format!("Failed to update google_user for {user_id:?}"))
            .unwrap();
            user_id
        } else {
            let calendar_hub = CalendarHub::new(
                hyper::Client::builder().build(
                    hyper_rustls::HttpsConnectorBuilder::new()
                        .with_native_roots()
                        .https_or_http()
                        .enable_http1()
                        .enable_http2()
                        .build(),
                ),
                auth,
            );
            let calendar_id = calendar_hub
                .calendars()
                .insert(Calendar {
                    summary: Some("Calendar hub".to_string()),
                    ..Default::default()
                })
                .doit()
                .await
                .context("Failed to create calendar")
                .unwrap()
                .1
                .id
                .unwrap();

            let user_id = UserId(
                sqlx::query!("INSERT INTO `user` (`dummy`) VALUES (0)")
                    .execute(&db)
                    .await
                    .context("Failed to insert new user")
                    .unwrap()
                    .last_insert_rowid() as _,
            );

            let minimum_date_time =
                chrono::DateTime::<chrono::Utc>::from_utc(chrono::NaiveDateTime::MIN, chrono::Utc);
            sqlx::query!(
                r#"INSERT INTO `google_user`
                (`user_id`, `access_token`, `calendar_id`, `last_synced`, `subject`)
                VALUES
                (?, ?, ?, ?, ?)"#,
                user_id,
                token,
                calendar_id,
                minimum_date_time,
                subject
            )
            .execute(&db)
            .await
            .context("Failed to insert into google_user")
            .unwrap();

            user_id
        };

        user_id_sender
            .send(user_id)
            .map_err(|_| "Failed to send user_id to callback handler")
            .unwrap();
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
    mut session: WritableSession,
    Extension(contexts): Extension<Arc<Mutex<LoginContextMap>>>,
    Query(query): Query<LoginCallbackQuery>,
) -> Response {
    if let Some((code_sender, user_id_receiver)) = contexts.lock().await.remove(&query.state) {
        code_sender
            .send(LoginCallbackCode(query.code))
            .map_err(|e| format!("Failed to send auth code - {e:?}"))
            .unwrap();
        let user_id = user_id_receiver
            .await
            .map_err(|e| format!("Failed to receive logged in user id - {e:?}"));
        session
            .insert("user_id", user_id)
            .context("Failed to insert user_id into session")
            .unwrap();

        "".into_response()
    } else {
        StatusCode::BAD_REQUEST.into_response()
    }
}

pub fn web_router<S: Sync + Send + Clone + 'static, B: HttpBody + Send + 'static>(
    config: Config,
) -> axum::Router<S, B> {
    let login_contexts = Arc::new(Mutex::new(LoginContextMap::new()));
    axum::Router::new()
        .route("/login", get(begin_login))
        .route("/callback", get(login_callback))
        .layer(Extension(config))
        .layer(Extension(login_contexts))
}

pub struct GoogleUser {
    user_id: UserId,
    access_token: String,
    calendar_id: String,
    last_synced: NaiveDateTime,
}

impl GoogleUser {
    pub async fn from_user_id(db: &SqlitePool, user_id: UserId) -> anyhow::Result<Option<Self>> {
        sqlx::query_as!(
            GoogleUser,
            r#"SELECT
                `user_id` as `user_id: UserId`,
                `access_token`,
                `calendar_id`,
                `last_synced`
            FROM `google_user`
            WHERE `user_id` = ?"#,
            user_id.0
        )
        .fetch_optional(db)
        .await
        .with_context(|| format!("Failed to get google_user for user_id {user_id:?}"))
    }

    pub async fn sync(&self, db: &SqlitePool) -> anyhow::Result<()> {
        let mut reservations: HashMap<_, _> = sqlx::query_as!(
            CalendarEvent,
            r#"SELECT
                `id`, `title`, `detail`,
                `date_begin` as `date_begin: chrono::NaiveDate`,
                `time_begin` as `time_begin: chrono::NaiveTime`,
                `date_end` as `date_end: chrono::NaiveDate`,
                `time_end` as `time_end: chrono::NaiveTime`,
                `invalid`
            FROM `reservation`
            WHERE `user_id` = ? AND `updated_at` > ?"#,
            self.user_id,
            self.last_synced
        )
        .fetch_all(db)
        .await
        .context("Failed to collect reservation data to update")?
        .into_iter()
        .map(|item| (item.id.clone(), item))
        .collect();

        // Refresh access token when needed
        let auth = oauth2::AccessTokenAuthenticator::builder(self.access_token.clone())
            .build()
            .await?;
        let token = auth.token(CALENDAR_SCOPE).await?;
        let token = token.token().unwrap();
        if self.access_token != token {
            sqlx::query!(
                "UPDATE `google_user` SET `access_token` = ? WHERE `user_id` = ?",
                token,
                self.user_id,
            )
            .execute(db)
            .await
            .context("Failed to update access token")?;
        }

        if reservations.is_empty() {
            return Ok(());
        }

        let hub = CalendarHub::new(
            hyper::Client::builder().build(
                hyper_rustls::HttpsConnectorBuilder::new()
                    .with_native_roots()
                    .https_or_http()
                    .enable_http1()
                    .enable_http2()
                    .build(),
            ),
            auth,
        );

        let google_events = sqlx::QueryBuilder::new(
            "SELECT `event_id`, `reservation_id` FROM `google_event` WHERE `user_id` = ",
        )
        .push_bind(self.user_id)
        .push("AND `reservation_id` in ")
        .push_tuples(reservations.keys(), |mut builder, item| {
            builder.push_bind(item);
        })
        .build()
        .fetch_all(db)
        .await
        .context("Failed to get saved google events")?;

        for google_event in google_events {
            let event_id: String = google_event.get_unchecked(0);
            let reservation_id: String = google_event.get_unchecked(1);
            if let Some(reservation) = reservations.remove(&reservation_id) {
                if reservation.invalid {
                    if let Err(_e) = hub
                        .events()
                        .delete(&self.calendar_id, &event_id)
                        .doit()
                        .await
                    {
                        // TODO: handle error
                    }
                } else if let Err(_e) = hub
                    .events()
                    .patch(reservation.into(), &self.calendar_id, &event_id)
                    .doit()
                    .await
                {
                    // TODO: handle error
                }
            }
        }

        if !reservations.is_empty() {
            let mut builder = sqlx::QueryBuilder::new(
                "INSERT INTO `google_event` (`event_id`, `user_id`, `reservation_id`)",
            );
            let mut new_events = Vec::new();
            for (_, reservation) in reservations.into_iter() {
                if reservation.invalid {
                    continue;
                }

                let reservation_id = reservation.id.clone();

                match hub
                    .events()
                    .insert(reservation.into(), &self.calendar_id)
                    .doit()
                    .await
                {
                    Ok((_, e)) => {
                        new_events.push((e.id.unwrap(), reservation_id));
                    }
                    Err(e) => error!("Failed to insert event - {e:?}"),
                }
            }

            if !new_events.is_empty() {
                builder.push_values(new_events, |mut b, r| {
                    b.push_bind(r.0).push_bind(self.user_id).push_bind(r.1);
                });
                builder
                    .build()
                    .execute(db)
                    .await
                    .context("Failed to insert newly created events")?;
            }
        }

        Ok(())
    }
}
