use std::collections::HashSet;

use sqlx::{Row as _, SqlitePool};
use tracing::info;

use crate::server::user::UserId;

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

pub fn date_time_to_utc(
    date: chrono::NaiveDate,
    time: chrono::NaiveTime,
    tz: impl chrono::TimeZone,
) -> (chrono::NaiveDate, chrono::NaiveTime) {
    let date_time = date
        .and_time(time)
        .and_local_timezone(tz)
        .latest()
        .unwrap()
        .naive_utc();
    (date_time.date(), date_time.time())
}

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

    pub(crate) async fn filter_ids<'a>(
        user_id: UserId,
        db: &SqlitePool,
        ids: &'a [impl AsRef<str> + 'a],
    ) -> anyhow::Result<Vec<&'a str>> {
        let mut builder = sqlx::query_builder::QueryBuilder::new(
            "SELECT `id` FROM `reservation` WHERE `user_id` = ",
        );
        builder.push_bind(user_id).push(" AND `id` IN ");
        let result = builder
            .push_tuples(ids, |mut f, id| {
                f.push_bind(id.as_ref());
            })
            .build()
            .fetch_all(db)
            .await?;
        let mut existing_ids = result
            .into_iter()
            .map(|item| item.get_unchecked::<String, _>(0))
            .collect::<HashSet<String>>();
        Ok(ids
            .iter()
            .filter_map(|i| (!existing_ids.remove(i.as_ref())).then(|| i.as_ref()))
            .collect())
    }

    pub(crate) async fn cancel_not_expired_and_not_in(
        user_id: UserId,
        db: &SqlitePool,
        prefix: &str,
        event_ids: impl Iterator<Item = &str>,
    ) -> anyhow::Result<u64> {
        let date_time = chrono::Utc::now().naive_utc();
        let date = date_time.date();
        let time = date_time.time();
        let mut builder = sqlx::query_builder::QueryBuilder::new(
            "UPDATE `reservation` SET `invalid` = TRUE WHERE `user_id` = ",
        );
        builder
            .push_bind(&user_id)
            .push(format!(" AND `id` LIKE \"{prefix}%\""))
            .push(" AND `invalid` == FALSE")
            .push(" AND (`date_begin` > ")
            .push_bind(&date)
            .push("OR (`date_begin` = ")
            .push_bind(&date)
            .push(" AND `time_begin` >")
            .push_bind(&time)
            .push(")) AND `id` NOT IN (");
        let mut b = builder.separated(",");
        for i in event_ids {
            b.push_bind(i);
        }
        let query = builder.push(")").build();
        let res = query.execute(db).await?;

        Ok(res.rows_affected())
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

pub fn open_browser() -> anyhow::Result<headless_chrome::Browser> {
    if let Ok(browser) = std::env::var("BROWSER") {
        headless_chrome::Browser::connect(browser)
    } else {
        headless_chrome::Browser::default()
    }
}
