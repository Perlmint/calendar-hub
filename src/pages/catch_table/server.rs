use serde::Deserialize;
use sqlx::SqlitePool;

use crate::{
    prelude::*,
    server::prelude::{reservation::*, user::*},
};

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
    // https://ct-api.catchtable.co.kr/api/v3/reservation/detail?reservationRef=<reservation_ref>
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
    land_name: Option<String>,
    food_kind: Option<String>,
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
        let detail = itertools::join(
            dining
                .shop
                .land_name
                .into_iter()
                .chain(dining.shop.food_kind.into_iter()),
            " - ",
        );
        let date_time = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
            dining.dining.visit_date_time as i64,
        )
        .context("Failed to convert from timestamp")?
        .naive_utc();
        let date_begin = date_time.date();
        let time_begin = date_time.time();
        let url = format!(
            "https://ct-api.catchtable.co.kr/api/v3/reservation/detail?reservationRef={}",
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

define_user_data! {
    #[base_url = "https://ct-api.catchtable.co.kr/"]
    struct CatchTableUser(
        "x-ct-a"
    )
}

pub(super) async fn crawl(
    config: super::CatchTableConfig,
    user_id: UserId,
    db: &SqlitePool,
) -> anyhow::Result<usize> {
    let jar = CatchTableUser::from_iter([config.x_ct_a].into_iter())?;
    info!("{jar:?}");
    let planned_url = url!("https://ct-api.catchtable.co.kr/api/v4/user/reservations/_list?statusGroup=PLANNED&sortCode=DESC&size=10");
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connection_verbose(true)
        .referer(false)
        .tcp_keepalive(None)
        .build()?;
    let req = client
        .get(planned_url.as_ref())
        .header(reqwest::header::COOKIE, jar.cookies(planned_url).unwrap())
        .header(reqwest::header::ACCEPT, "*/*")
        .version(reqwest::Version::HTTP_2)
        .fetch_mode_no_cors()
        .build()?;
    info!("request: {req:?}");
    let response = client.execute(req).await?;
    let body = response.text().await?;
    info!("response body: {}", body);
    let res: ReservationsResponse = serde_json::from_str(&body)?;
    if res.data.items.is_empty() {
        return Ok(0);
    }

    let reservations = res
        .data
        .items
        .into_iter()
        .filter_map(|item| <Option<CalendarEvent>>::try_from(item).transpose())
        .collect::<Result<Vec<_>, _>>()?;

    let updated_item_count =
        CalendarEvent::upsert_events_to_db(user_id, &db, reservations.iter()).await?;
    info!("updated item count: {updated_item_count}",);

    Ok(updated_item_count as usize)
}
