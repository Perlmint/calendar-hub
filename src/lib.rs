use axum::{
    async_trait,
    response::{IntoResponse, Response},
    Extension, Json, Router,
};
use axum_sessions::extractors::ReadableSession;
use hyper::StatusCode;
use log::{debug, error, info};
use sqlx::SqlitePool;

pub mod catch_table;
pub mod google_calendar;
pub mod naver_reservation;

#[repr(transparent)]
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    sqlx::Type,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(transparent)]
#[sqlx(transparent)]
pub struct UserId(u32);

impl From<i64> for UserId {
    fn from(value: i64) -> Self {
        Self(value as _)
    }
}

#[repr(transparent)]
#[derive(Debug, Clone, PartialEq, Eq, sqlx::Type)]
#[sqlx(transparent)]
pub struct ReservationId(String);

impl From<String> for ReservationId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl<'a> From<&'a str> for ReservationId {
    fn from(value: &'a str) -> Self {
        Self(value.to_string())
    }
}

impl AsRef<str> for ReservationId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[repr(transparent)]
#[derive(Debug, Clone, PartialEq, Eq, sqlx::Type)]
#[sqlx(transparent)]
pub struct EventId(String);

impl From<String> for EventId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl<'a> From<&'a str> for EventId {
    fn from(value: &'a str) -> Self {
        Self(value.to_string())
    }
}

impl AsRef<str> for EventId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/15.5 Safari/605.1.15";

#[derive(Debug, Clone)]
pub struct CalendarEvent {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub invalid: bool,
    pub date_begin: chrono::NaiveDate,
    pub time_begin: Option<chrono::NaiveTime>,
    pub date_end: Option<chrono::NaiveDate>,
    pub time_end: Option<chrono::NaiveTime>,
    pub location: Option<String>,
    pub url: Option<String>,
}

impl CalendarEvent {
    pub(crate) async fn upsert_events_to_db(
        user_id: UserId,
        db: &SqlitePool,
        items: impl Iterator<Item = &Self>,
    ) -> anyhow::Result<u64> {
        info!("Update events for {user_id:?}");
        let mut builder = sqlx::query_builder::QueryBuilder::new(
            r#"INSERT INTO `reservation` (
            `id`, `user_id`,
            `title`, `detail`,
            `date_begin`, `time_begin`,
            `date_end`, `time_end`,
            `invalid`, `url`, `location`,
            `updated_at`
        ) "#,
        );

        let now = chrono::Utc::now().naive_utc();

        let result = builder
            .push_values(items, |mut builder, event| {
                builder
                    .push_bind(&event.id)
                    .push_bind(user_id)
                    .push_bind(&event.title)
                    .push_bind(&event.detail)
                    .push_bind(event.date_begin)
                    .push_bind(event.time_begin)
                    .push_bind(event.date_end)
                    .push_bind(event.time_end)
                    .push_bind(event.invalid)
                    .push_bind(&event.url)
                    .push_bind(&event.location)
                    .push_bind(now);
            })
            .push(
                r#"ON CONFLICT(`id`, `user_id`) DO UPDATE SET
                `title`=`excluded`.`title`, `detail`=`excluded`.`detail`,
                `date_begin`=`excluded`.`date_begin`, `time_begin`=`excluded`.`time_begin`,
                `date_end`=`excluded`.`date_end`, `time_end`=`excluded`.`time_end`,
                `invalid`=`excluded`.`invalid`, `url`=`excluded`.`url`, `location`=`excluded`.`location`,
                `updated_at`="#,
            )
            .push_bind(now)
            .push(
                r#"WHERE 
                `reservation`.`title` IS NOT `excluded`.`title` OR `reservation`.`detail` IS NOT `excluded`.`detail` OR
                `reservation`.`date_begin` IS NOT `excluded`.`date_begin` OR `reservation`.`time_begin` IS NOT `excluded`.`time_begin` OR
                `reservation`.`date_end` IS NOT `excluded`.`date_end` OR `reservation`.`time_end` IS NOT `excluded`.`time_end` OR
                `reservation`.`invalid` IS NOT `excluded`.`invalid` OR `reservation`.`url` IS NOT `excluded`.`url` OR
                `reservation`.`location` IS NOT `excluded`.`location`"#,
            )
            .build()
            .execute(db)
            .await?;

        Ok(result.rows_affected())
    }

    #[allow(dead_code)]
    pub(crate) async fn upsert_to_db(
        &self,
        user_id: UserId,
        db: &SqlitePool,
    ) -> anyhow::Result<()> {
        let now = chrono::Utc::now().naive_utc();

        sqlx::query!(
            r#"INSERT OR REPLACE INTO `reservation` (
                `id`, `user_id`,
                `title`, `detail`,
                `date_begin`, `time_begin`,
                `date_end`, `time_end`,
                `invalid`, `updated_at`
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
            self.id,
            user_id,
            self.title,
            self.detail,
            self.date_begin,
            self.time_begin,
            self.date_end,
            self.time_end,
            self.invalid,
            now
        )
        .execute(db)
        .await?;

        Ok(())
    }
}

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

#[macro_export]
macro_rules! define_user_data {
    (#[table_name = $table_name:literal]#[base_url = $url:literal]struct $name:ident { $(#[session_name = $session_name:literal]$field_name:ident: $field_type:ty,)+}) => {
        paste::paste!{
            #[derive(serde::Serialize, serde::Deserialize, Default)]
            pub struct [<$name Detail>] {
                $($field_name: $field_type,)+
            }

            pub struct $name {
                user_id: $crate::UserId,
                $($field_name: $field_type,)+
            }

            impl $name {
                pub fn user_id(&self) -> $crate::UserId {
                    self.user_id
                }
            }

            impl From<$name> for [<$name Detail>] {
                fn from(value: $name) -> Self {
                    Self {
                        $($field_name: value.$field_name,)+
                    }
                }
            }

            impl From<($crate::UserId, [<$name Detail>])> for $name {
                fn from(value: ($crate::UserId, [<$name Detail>])) -> Self {
                    Self {
                        user_id: value.0,
                        $($field_name: value.1.$field_name,)+
                    }
                }
            }

            impl $name {
                fn to_cookie_jar(&self) -> reqwest::cookie::Jar {
                    let endpoint_base = crate::url!($url);
                    let jar = reqwest::cookie::Jar::default();
                    $(
                        jar.add_cookie_str(&format!("{}={}", $session_name, self.$field_name), endpoint_base);
                    )+
                    jar
                }

                // async fn update_session(&self, db: SqlitePool) -> anyhow::Result<()> {
                //     sqlx::query!(
                //         concat!("UPDATE `", $table_name, "` SET `ses` = ?, `aut` = ? WHERE `user_id` = ?"),
                //         self.ses,
                //         self.aut,
                //         self.user_id
                //     )
                //     .execute(&db)
                //     .await
                //     .context("Failed to update naver user session data")
                //     .map(|_| ())
                // }
            }

            impl $crate::User for $name {
                fn default_with_user_id(user_id: UserId) -> Self {
                    Self {
                        user_id,
                        $($field_name: Default::default(),)+
                    }
                }
            }
        }
    };
}

#[async_trait]
pub trait User: UserImpl {
    fn default_with_user_id(user_id: UserId) -> Self;
    async fn from_user_id_default(db: SqlitePool, user_id: UserId) -> anyhow::Result<Self> {
        Ok(Self::from_user_id(db, user_id)
            .await?
            .unwrap_or_else(|| Self::default_with_user_id(user_id)))
    }
}

#[async_trait]
pub trait UserImpl: Sized + From<(UserId, Self::Detail)> + Send + Sync + 'static {
    type Detail: serde::Serialize
        + serde::de::DeserializeOwned
        + Default
        + From<Self>
        + Sized
        + Send
        + Sync
        + 'static;

    async fn fetch(&self, db: SqlitePool) -> anyhow::Result<bool>;
    async fn from_user_id(db: SqlitePool, user_id: UserId) -> anyhow::Result<Option<Self>>;
    async fn update_session(&self, db: SqlitePool) -> anyhow::Result<()>;
}

async fn get_info<U: User>(
    session: ReadableSession,
    Extension(db): Extension<SqlitePool>,
) -> Response {
    let Some(user_id) = session.get::<UserId>("user_id") else {
        debug!("Not logged in");
        return StatusCode::FORBIDDEN.into_response();
    };

    let naver_user = U::from_user_id(db, user_id).await.unwrap();

    Json(naver_user.map(U::Detail::from).unwrap_or_default()).into_response()
}

async fn update_info<U: User>(
    session: ReadableSession,
    Extension(db): Extension<SqlitePool>,
    Json(data): Json<U::Detail>,
) -> Response {
    let Some(user_id) = session.get::<UserId>("user_id") else {
        debug!("Not logged in");
        return StatusCode::FORBIDDEN.into_response();
    };

    if let Err(e) = U::from((user_id, data)).update_session(db).await {
        error!("Error occurred while update naver session data - {e:?}");
        StatusCode::INTERNAL_SERVER_ERROR.into_response()
    } else {
        StatusCode::ACCEPTED.into_response()
    }
}

pub fn user_web_router<U: User>() -> Router {
    Router::new()
        .route("/user", axum::routing::get(get_info::<U>))
        .route("/user", axum::routing::post(update_info::<U>))
}
