use log::info;
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
