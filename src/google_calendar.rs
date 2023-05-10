use std::{
    collections::{BTreeMap, HashMap, HashSet},
    future::Future,
    io::BufRead,
    path::Path,
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
use log::{debug, error, info};
use notify::{event::ModifyKind, EventKind, RecommendedWatcher};
use rsa::{pkcs8::AssociatedOid, Pkcs1v15Sign, RsaPublicKey};
use sha2::Digest;
use sqlx::{Row, SqlitePool};

use anyhow::Context;
use google_calendar3::{
    api::{AclRule, AclRuleScope, Calendar, Event, EventDateTime},
    hyper, hyper_rustls,
    oauth2::{self, authenticator_delegate::InstalledFlowDelegate},
    CalendarHub,
};
use tokio::sync::{oneshot, Mutex, RwLock};
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
            description: Some(
                event
                    .url
                    .map(|url| format!("{}\n{}", event.detail, url))
                    .unwrap_or(event.detail),
            ),
            end: Some(
                event
                    .date_end
                    .map(|date| (date, event.time_end).into_google())
                    .unwrap_or_else(|| start.clone()),
            ),
            start: Some(start),
            summary: Some(event.title),
            location: event.location,
            ..Default::default()
        }
    }
}

const CALENDAR_SCOPE: &[&str] = &[
    "https://www.googleapis.com/auth/calendar",
    "https://www.googleapis.com/auth/calendar.readonly",
    "https://www.googleapis.com/auth/calendar.events",
    "openid",
    "email",
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

pub struct Config {
    secret: oauth2::ApplicationSecret,
    url_prefix: String,
    google_key_store: BTreeMap<String, RsaVerifying>,
    service_account: google_calendar3::oauth2::ServiceAccountKey,
    allowed_emails: AllowedEmails,
    _watcher: RecommendedWatcher,
}

static SHARED_CONFIG: once_cell::sync::OnceCell<Arc<Config>> = once_cell::sync::OnceCell::new();

impl Config {
    pub async fn init(url_prefix: String) -> anyhow::Result<()> {
        let secret = google_calendar3::oauth2::read_application_secret("google.json").await?;
        let google_key_store = fetch_google_key_store().await?;
        let (allowed_emails, watcher) = AllowedEmails::new("allowed-emails").await?;
        let service_account =
            google_calendar3::oauth2::read_service_account_key("service_account.json").await?;

        SHARED_CONFIG
            .set(Arc::new(Self {
                secret,
                url_prefix,
                google_key_store: google_key_store,
                service_account,
                allowed_emails,
                _watcher: watcher,
            }))
            .map_err(|_| anyhow::anyhow!("Config init should be called only once"))
    }

    pub fn get() -> Arc<Self> {
        SHARED_CONFIG
            .get()
            .expect("google config is not initialized yet")
            .clone()
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

#[derive(Debug, Clone)]
#[repr(transparent)]
struct AllowedEmails(Arc<RwLock<HashSet<String>>>);

impl AsRef<RwLock<HashSet<String>>> for AllowedEmails {
    fn as_ref(&self) -> &RwLock<HashSet<String>> {
        self.0.as_ref()
    }
}

impl AllowedEmails {
    async fn new(
        path: impl AsRef<Path> + Send + Clone + 'static,
    ) -> anyhow::Result<(AllowedEmails, RecommendedWatcher)> {
        let ret = tokio::task::block_in_place(|| Self::read_from_file(path.as_ref()))?;
        let ret = Arc::new(RwLock::new(ret));

        let data = ret.clone();
        let mut watcher = {
            let path = path.clone();
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(res) = res {
                    if let EventKind::Modify(ModifyKind::Data(_)) = res.kind {
                        match Self::read_from_file(path.as_ref()) {
                            Ok(value) => {
                                info!("allowed-emails {} items reloaded", value.len());
                                *data.blocking_write() = value;
                            }
                            Err(e) => error!("Failed to reload allowed-emails - {e:?}"),
                        }
                    }
                }
            })?
        };
        use notify::{RecursiveMode, Watcher};
        watcher.watch(path.as_ref(), RecursiveMode::NonRecursive)?;

        Ok((Self(ret), watcher))
    }

    fn read_from_file(path: impl AsRef<Path>) -> anyhow::Result<HashSet<String>> {
        let file = std::io::BufReader::new(std::fs::File::open(path.as_ref())?);
        let lines = file.lines();

        lines
            .filter_map(|line| {
                line.map(|mut line| {
                    (!line.is_empty()).then(move || {
                        line.shrink_to(line.trim().len());
                        line
                    })
                })
                .context("Failed to read allowed-emails")
                .transpose()
            })
            .collect()
    }
}

async fn begin_login(
    session: ReadableSession,
    Extension(db): Extension<SqlitePool>,
    Extension(contexts): Extension<Arc<Mutex<LoginContextMap>>>,
) -> Response {
    if let Some(session) = session.get::<UserId>("user_id") {
        debug!("Already logged in redirect to main - {session:?}");
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
        let config = Config::get();

        let auth = oauth2::InstalledFlowAuthenticator::builder(
            config.secret.clone(),
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

        let (subject, email) = {
            use jwt::VerifyWithStore;

            let id_token = auth.id_token(CALENDAR_SCOPE).await.unwrap().unwrap();
            let mut claims: BTreeMap<String, serde_json::Value> = id_token
                .verify_with_store(&config.google_key_store)
                .context("jwt verification failed")
                .unwrap();
            (
                claims
                    .remove("sub")
                    .context("sub is not in received claims")
                    .unwrap(),
                claims
                    .remove("email")
                    .context("email is not in received claims")
                    .unwrap(),
            )
        };

        auth.token(CALENDAR_SCOPE)
            .await
            .context("Failed to get access token")
            .unwrap();

        let email = email
            .as_str()
            .context("received email in claims is not string")
            .unwrap();

        if config
            .allowed_emails
            .as_ref()
            .read()
            .await
            .get(email)
            .is_none()
        {
            user_id_sender.send(None).unwrap();
            return;
        }

        info!("Login succeed {email}");
        let subject = subject
            .as_str()
            .context("received subject in claims is not string")
            .unwrap();

        let user_info = sqlx::query!(
            "SELECT `user_id` as `user_id:UserId`, `calendar_id`, `acl_id` FROM `google_user` WHERE `subject` = ?",
            subject
        )
        .fetch_optional(&db)
        .await
        .context("Failed to query logged in user")
        .unwrap()
        .map(|record| (record.user_id, record.calendar_id, record.acl_id));

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

        // validate calendar_id & acl_id, make sure user_id is valid
        let (user_id, calendar_id, acl_id) = if let Some((user_id, calendar_id, acl_id)) = user_info
        {
            if let Err(e) = calendar_hub.calendars().get(&calendar_id).doit().await {
                info!("Saved calendar_id({calendar_id}) is invalid - {e:?}");
                (user_id, None, None)
            } else if let Some(acl_id) = acl_id {
                let acl_id =
                    if let Err(e) = calendar_hub.acl().get(&calendar_id, &acl_id).doit().await {
                        info!("Saved acl_id is invalid - {e:?}");
                        None
                    } else {
                        Some(acl_id)
                    };
                (user_id, Some(calendar_id), acl_id)
            } else {
                (user_id, Some(calendar_id), None)
            }
        } else {
            let user_id = UserId(
                sqlx::query!("INSERT INTO `user` (`dummy`) VALUES (0)")
                    .execute(&db)
                    .await
                    .context("Failed to insert new user")
                    .unwrap()
                    .last_insert_rowid() as _,
            );

            (user_id, None, None)
        };

        let calendar_id = match calendar_id {
            Some(calendar_id) => calendar_id,
            None => {
                info!("Create new calendar");
                calendar_hub
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
                    .unwrap()
            }
        };

        let acl_id = match acl_id {
            Some(id) => id,
            None => {
                info!("Share calendar {calendar_id} to service account");
                calendar_hub
                    .acl()
                    .insert(
                        AclRule {
                            etag: None,
                            id: None,
                            kind: None,
                            role: Some("writer".to_string()),
                            scope: Some(AclRuleScope {
                                type_: Some("user".to_string()),
                                value: Some(config.service_account.client_email.clone()),
                            }),
                        },
                        &calendar_id,
                    )
                    .doit()
                    .await
                    .unwrap()
                    .1
                    .id
                    .expect("Id of AclRule in Response should be set")
            }
        };

        let minimum_date_time =
            chrono::DateTime::<chrono::Utc>::from_utc(chrono::NaiveDateTime::MIN, chrono::Utc);
        sqlx::query!(
            r#"INSERT INTO `google_user`
            (`user_id`, `calendar_id`, `acl_id`, `last_synced`, `subject`)
            VALUES
            (?, ?, ?, ?, ?)
            ON CONFLICT DO UPDATE SET
            `calendar_id`=`excluded`.`calendar_id`, `acl_id`=`excluded`.`acl_id`"#,
            user_id,
            calendar_id,
            acl_id,
            minimum_date_time,
            subject
        )
        .execute(&db)
        .await
        .context("Failed to insert into google_user")
        .unwrap();

        user_id_sender
            .send(Some(user_id))
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
            .map_err(|e| format!("Failed to receive logged in user id - {e:?}"))
            .unwrap();
        if let Some(user_id) = user_id {
            debug!("Successfully logged in");
            session
                .insert("user_id", user_id)
                .context("Failed to insert user_id into session")
                .unwrap();
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

pub fn web_router<S: Sync + Send + Clone + 'static, B: HttpBody + Send + 'static>(
) -> axum::Router<S, B> {
    let login_contexts = Arc::new(Mutex::new(LoginContextMap::new()));
    axum::Router::new()
        .route("/login", get(begin_login))
        .route("/callback", get(login_callback))
        .layer(Extension(login_contexts))
}

pub struct GoogleUser {
    user_id: UserId,
    calendar_id: String,
    last_synced: NaiveDateTime,
}

impl GoogleUser {
    pub async fn from_user_id(db: &SqlitePool, user_id: UserId) -> anyhow::Result<Option<Self>> {
        sqlx::query_as!(
            GoogleUser,
            r#"SELECT
                `user_id` as `user_id: UserId`,
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
                `invalid`,
                `location`,
                `url`
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

        let config = Config::get();

        let auth = oauth2::ServiceAccountAuthenticator::builder(config.service_account.clone())
            .build()
            .await?;

        if !reservations.is_empty() {
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
        }

        let now = chrono::Utc::now().naive_utc();
        sqlx::query!(
            "UPDATE `google_user` SET `last_synced` = ? WHERE `user_id` = ?",
            now,
            self.user_id
        )
        .execute(db)
        .await
        .unwrap();

        Ok(())
    }
}

pub async fn get_last_synced(
    db: SqlitePool,
    user_id: UserId,
) -> anyhow::Result<chrono::DateTime<chrono::Utc>> {
    sqlx::query!(
        "SELECT `last_synced` FROM `google_user` WHERE `user_id` = ?",
        user_id
    )
    .fetch_one(&db)
    .await
    .context("Failed to get last_synced for ({user_id:?}) from DB")
    .map(|row| chrono::DateTime::from_utc(row.last_synced, chrono::Utc))
}
