use anyhow::Context;
use futures::StreamExt;
use reqwest::cookie::{CookieStore, Jar};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::{url, UserId};

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
}

pub struct User {
    user_id: UserId,
    jsessionid: String,
}

impl User {
    fn to_cookie_jar(&self) -> Jar {
        let endpoint_base = url!("https://app.catchtable.co.kr/");
        let jar = Jar::default();
        jar.add_cookie_str(
            &format!("{}={}", "JSESSIONID", self.jsessionid),
            endpoint_base,
        );
        jar
    }

    pub async fn from_user_id(db: SqlitePool, user_id: UserId) -> anyhow::Result<Option<Self>> {
        sqlx::query_as!(
            Self,
            "SELECT `user_id` as `user_id: UserId`, `jsessionid` FROM `catch_table_user`"
        )
        .fetch_optional(&db)
        .await
        .with_context(|| format!("Failed to get catch_table_user of {user_id:?}"))
    }

    pub fn all(db: &SqlitePool) -> impl futures::Stream<Item = anyhow::Result<Self>> + '_ {
        sqlx::query_as!(
            Self,
            "SELECT `user_id` as `user_id: UserId`, `jsessionid` FROM `catch_table_user`"
        )
        .fetch(db)
        .map(|result| result.context("Failed to get catch_table_user"))
    }

    async fn fetch(&self) -> anyhow::Result<()> {
        let jar = self.to_cookie_jar();
        let planned_url = url!("https://app.catchtable.co.kr/api/v4/user/reservations/_list?statusGroup=PLANNED&sortCode=DESC&size=10");
        let client = reqwest::Client::new();
        let req = client
            .post(planned_url.as_ref())
            .header(reqwest::header::COOKIE, jar.cookies(planned_url).unwrap())
            .build()?;
        let res: ReservationsResponse = client.execute(req).await?.json().await?;

        const LAST_LOGIN: &str = "https://app.catchtable.co.kr/api/v3/user/lastLoginTime";

        Ok(())
    }
}
