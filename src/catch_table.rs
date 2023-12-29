use anyhow::Context;
use axum::{async_trait, Router};
use futures::StreamExt;
use log::info;
use reqwest::cookie::CookieStore;
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::{url, CalendarEvent, UserId};

#[derive(Debug, Deserialize)]
struct ReservationsResponse {
    data: ReservationsData,
}

#[derive(Debug, Deserialize)]
struct ReservationsData {
    items: Vec<Reservation>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
#[serde(tag = "reservationType")]
enum Reservation {
    Waiting,
    Dining(Dining),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReservationCommon {
    // unique id
    // https://app.catchtable.co.kr/ct/customer/reservation/detail/<reservation_ref>
    reservation_ref: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Waiting {}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Dining {
    #[serde(flatten)]
    common: ReservationCommon,
    dining: DiningDetail,
    shop: Shop,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiningDetail {
    visit_date_time: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Shop {
    shop_name: String,
    shop_address: String,
    land_name: String,
    food_kind: String,
}

impl TryFrom<Reservation> for Option<CalendarEvent> {
    type Error = anyhow::Error;

    fn try_from(value: Reservation) -> Result<Self, Self::Error> {
        let Reservation::Dining(dining) = value else {
            return Ok(None);
        };

        let id = format!("catch_table/{}", dining.common.reservation_ref);
        let title = dining.shop.shop_name;
        let location = dining.shop.shop_address;
        let detail = format!("{} - {}", dining.shop.land_name, dining.shop.food_kind);
        let date_time =
            chrono::NaiveDateTime::from_timestamp_millis(dining.dining.visit_date_time as i64)
                .context("Failed to convert from timestamp")?;
        let date_begin = date_time.date();
        let time_begin = date_time.time();
        let url = format!(
            "https://app.catchtable.co.kr/ct/customer/reservation/detail/{}",
            dining.common.reservation_ref
        );

        Ok(Some(CalendarEvent {
            id,
            title,
            detail,
            invalid: false,
            date_begin,
            time_begin: Some(time_begin),
            date_end: None,
            time_end: None,
            location: Some(location),
            url: Some(url),
        }))
    }
}

crate::define_user_data! {
    #[table_name = "catch_table"]
    #[base_url = "https://app.catchtable.co.kr/"]
    struct CatchTableUser {
        #[session_name = "x-ct-a"]
        jsessionid: String,
    }
}

impl CatchTableUser {
    pub fn all(db: &SqlitePool) -> impl futures::Stream<Item = anyhow::Result<Self>> + '_ {
        sqlx::query_as!(
            Self,
            "SELECT `user_id` as `user_id: UserId`, `jsessionid` FROM `catch_table_user`"
        )
        .fetch(db)
        .map(|result| result.context("Failed to get catch_table_user"))
    }
}

#[async_trait]
impl crate::UserImpl for CatchTableUser {
    type Detail = CatchTableUserDetail;

    const PING_INTERVAL: Option<std::time::Duration> =
        Some(std::time::Duration::from_secs(10 * 60));

    async fn fetch(&self, db: SqlitePool) -> anyhow::Result<bool> {
        let jar = self.to_cookie_jar();
        let planned_url = url!("https://app.catchtable.co.kr/api/v4/user/reservations/_list?statusGroup=PLANNED&sortCode=DESC&size=10");
        let client = reqwest::Client::new();
        let req = client
            .get(planned_url.as_ref())
            .header(reqwest::header::COOKIE, jar.cookies(planned_url).unwrap())
            .build()?;
        let res: ReservationsResponse = client.execute(req).await?.json().await?;
        if res.data.items.is_empty() {
            return Ok(false);
        }

        let reservations = res
            .data
            .items
            .into_iter()
            .filter_map(|item| <Option<CalendarEvent>>::try_from(item).transpose())
            .collect::<Result<Vec<_>, _>>()?;

        let updated_item_count =
            CalendarEvent::upsert_events_to_db(self.user_id, &db, reservations.iter()).await?;
        info!("updated item count: {updated_item_count}",);

        Ok(updated_item_count > 0)
    }

    async fn ping(&self) -> anyhow::Result<()> {
        let jar = self.to_cookie_jar();
        let url = url!("https://app.catchtable.co.kr/api/v3/user/lastLoginTime");
        let client = reqwest::Client::new();
        let req = client
            .post(url.as_ref())
            .header(reqwest::header::COOKIE, jar.cookies(url).unwrap())
            .build()?;
        client
            .execute(req)
            .await
            .context("Error occurred while sending ping")?;

        Ok(())
    }

    async fn from_user_id(db: SqlitePool, user_id: UserId) -> anyhow::Result<Option<Self>> {
        sqlx::query_as!(
            Self,
            "SELECT `user_id` as `user_id: UserId`, `jsessionid` FROM `catch_table_user`"
        )
        .fetch_optional(&db)
        .await
        .with_context(|| format!("Failed to get catch_table_user of {user_id:?}"))
    }

    async fn update_session(&self, db: SqlitePool) -> anyhow::Result<()> {
        sqlx::query!(
            "INSERT INTO `catch_table_user` (`jsessionid`, `user_id`) VALUES (?, ?)
                ON CONFLICT (`user_id`) DO UPDATE
                SET `jsessionid` = ? WHERE `user_id` = ?",
            self.jsessionid,
            self.user_id,
            self.jsessionid,
            self.user_id
        )
        .execute(&db)
        .await
        .context("Failed to update catch table user session data")
        .map(|_| ())
    }
}

pub fn web_router() -> Router {
    crate::user_web_router::<CatchTableUser>()
}
