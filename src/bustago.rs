use std::collections::HashMap;

// cSpell:ignore appv birthdate cancle cardno ccard eter routecode sdate stime
use anyhow::Context;
use axum::{async_trait, Router};
use chrono::Datelike;
use futures::StreamExt;
use log::info;
use reqwest::cookie::CookieStore;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

use crate::{date_time_to_utc, url, CalendarEvent, UserId};

#[derive(Debug, Deserialize)]
struct ReservationResponse {
    #[serde(rename = "list")]
    items: Vec<Reservation>,
}

#[derive(Debug, Deserialize)]
struct Reservation {
    all_seat_status: String,
    #[serde(rename = "ccard_appv_no")]
    approval_number: String,
    #[serde(rename = "arr_ter_nm")]
    arrival_terminal_name: String,
    #[serde(rename = "arr_ter_id")]
    arrival_terminal_code: String,
    #[serde(rename = "dep_ter_nm")]
    departure_terminal_name: String,
    #[serde(rename = "dep_ter_id")]
    departure_terminal_code: String,
    #[serde(rename = "org_reserve_no")]
    reservation_number: String,
    #[serde(rename = "reserve_dt")]
    reservation_date: String,
    #[serde(rename = "sdate")]
    departure_date: String,
    #[serde(rename = "stime")]
    departure_time: String,
    #[serde(rename = "routeCode")]
    route_code: String,
    #[serde(rename = "cardNo")]
    card_number: String,
    #[serde(rename = "tot_seat_cnt")]
    total_seat_count: String,
    #[serde(rename = "transp_bizr_abbr_nm")]
    operator_name: String,
}

#[derive(Deserialize)]
struct LineInfoResponse {
    list: Vec<LineInfo>,
}

#[derive(Deserialize)]
struct LineInfo {
    #[serde(rename = "dep_ter_nm")]
    departure_terminal_name: String,
    #[serde(rename = "arr_ter_nm")]
    arrival_terminal_name: String,
    #[serde(rename = "dist_time")]
    distance_time: u64,
}

crate::define_user_data! {
    #[table_name = "bustago"]
    #[base_url = "https://www.bustago.or.kr/"]
    struct BustagoUser {
        #[session_name = "JSESSIONID"]
        jsessionid: String,
        user_number: String,
    }
}

impl BustagoUser {
    pub fn all(db: &SqlitePool) -> impl futures::Stream<Item = anyhow::Result<Self>> + '_ {
        sqlx::query_as!(
            Self,
            "SELECT `user_id` as `user_id: UserId`, `jsessionid`, `user_number` FROM `bustago_user`"
        )
        .fetch(db)
        .map(|result| result.context("Failed to get bustago_user"))
    }
}

fn to_numeric_date(date: chrono::NaiveDate) -> u32 {
    (date.year() as u32) * 10000 + date.month() * 100 + date.day()
}

const REQUIRED_STRING_FIELDS: &[&str] = &[
    "type",
    "searchDate",
    "searchTime",
    "routecode",
    "sterCode",
    "eterCode",
    "sterName",
    "eterName",
    "reserveNo",
    "sdate",
    "totalSeat",
    "startTime",
    "totCnt",
    "seatNos",
    "totAMT",
    "seatNo",
    "oldSeatNo",
    "oldSeatNos",
    "reserveTime",
    "startDate",
    "ccType",
    "cardno",
    "birthdate",
    "tel",
    "stime",
    "appv_no",
    "org_reserve_no",
    "org_reserve_time",
    "reserve_cd",
    "hd_cancle_no",
    "reserveTime1",
    "startDate1",
    "sdate1",
    "startTime1",
    "reserveNo1",
    "sterCode1",
    "appv_no1",
    "reserveTime2",
    "startDate2",
    "sdate2",
    "startTime2",
    "reserveNo2",
    "sterCode2",
    "appv_no2",
    "ccType1",
    "ccType2",
    "now_status",
    "card_No",
    "Amount",
    "ticket_no",
    "cardNumber",
    "startDateParam",
    "EndDateParam",
    "tokenId",
];

#[async_trait]
impl crate::UserImpl for BustagoUser {
    type Detail = BustagoUserDetail;

    const PING_INTERVAL: Option<std::time::Duration> =
        Some(std::time::Duration::from_secs(10 * 60));

    async fn fetch(&self, db: SqlitePool) -> anyhow::Result<bool> {
        let date_begin = chrono::Utc::now()
            .with_timezone(&chrono::FixedOffset::east_opt(9).unwrap())
            .date_naive();
        let date_end = date_begin + chrono::Duration::days(7);
        let jar = self.to_cookie_jar();
        let reservations_url = url!("https://www.bustago.or.kr/newweb/kr/reserve/reservejson.do");
        let client = reqwest::Client::new();
        let user_number = self.user_number.clone();
        let mut request = serde_json::json!({
            "fromDate": to_numeric_date(date_begin),
            "toDate": to_numeric_date(date_end),
            "v_dateGb": 2,
            "v_status": 0,
            "page": 1,
            "userNumber": user_number,
        });
        unsafe { request.as_object_mut().unwrap_unchecked() }.extend(
            REQUIRED_STRING_FIELDS
                .iter()
                .map(|key| (key.to_string(), serde_json::Value::String("".to_string()))),
        );

        let req = client
            .post(reservations_url.as_ref())
            .header(
                reqwest::header::REFERER,
                "https://www.bustago.or.kr/newweb/kr/reserve/reservelist.do",
            )
            .header(
                reqwest::header::COOKIE,
                jar.cookies(reservations_url).unwrap(),
            )
            .form(&request)
            .build()?;
        let res: ReservationResponse = client
            .execute(req)
            .await
            .context("Failed to fetch reservejson")?
            .json()
            .await
            .context("Failed to parse reservejson")?;
        if res.items.is_empty() {
            return Ok(false);
        }

        let ids: Vec<_> = res
            .items
            .iter()
            .map(|reservation| format!("bustago/{}", reservation.reservation_number))
            .collect();
        let saved_reservations: HashMap<_, _> = sqlx::QueryBuilder::new(
            "
            SELECT `id`, `invalid`
            FROM `reservation`
            WHERE `user_id` =
        ",
        )
        .push_bind(self.user_id)
        .push("AND `id` in ")
        .push_tuples(&ids, |mut builder, item| {
            builder.push_bind(item);
        })
        .build()
        .fetch_all(&db)
        .await
        .context("Failed to get saved reservations")?
        .into_iter()
        .map(|item| {
            (
                item.get::<String, _>(0).split_off("bustago/".len()),
                item.get::<bool, _>(1),
            )
        })
        .collect();

        let mut new_reservations = Vec::new();
        for (reservation, id) in res.items.into_iter().zip(ids.into_iter()) {
            let current_invalid = reservation.all_seat_status == "2";
            if let Some(invalid) = saved_reservations.get(&reservation.reservation_number) {
                if current_invalid {
                    if !invalid {
                        sqlx::query!(
                            "UPDATE `reservation`
                            SET `invalid` = 1
                            WHERE
                                `user_id` = ? AND
                                `id` = ?",
                            self.user_id,
                            id
                        )
                        .execute(&db)
                        .await
                        .context("Failed to set invalid")?;
                    }
                }
                continue;
            }

            request["routecode"] = serde_json::Value::String(reservation.route_code.clone());
            request["sterCode"] =
                serde_json::Value::String(reservation.departure_terminal_code.clone());
            request["eterCode"] =
                serde_json::Value::String(reservation.arrival_terminal_code.clone());
            request["sterName"] =
                serde_json::Value::String(reservation.departure_terminal_name.clone());
            request["eterName"] =
                serde_json::Value::String(reservation.arrival_terminal_name.clone());
            request["reserveNo"] =
                serde_json::Value::String(reservation.reservation_number.clone());
            request["sdate"] = serde_json::Value::String(reservation.departure_date.clone());
            request["totalSeat"] = serde_json::Value::String(reservation.total_seat_count.clone());
            request["startTime"] = serde_json::Value::String(reservation.departure_time.clone());
            request["reserveTime"] =
                serde_json::Value::String(reservation.reservation_date.clone());
            request["stime"] = serde_json::Value::String(reservation.departure_time.clone());
            request["appv_no"] = serde_json::Value::String(reservation.approval_number.clone());
            request["org_reserve_no"] =
                serde_json::Value::String(reservation.reservation_number.clone());
            request["org_reserve_time"] =
                serde_json::Value::String(reservation.reservation_date.clone());
            request["reserve_cd"] =
                serde_json::Value::String(reservation.reservation_number.clone());
            request["card_No"] = serde_json::Value::String(reservation.card_number.clone());
            request["ticket_no"] =
                serde_json::Value::String(reservation.reservation_number.clone());
            request["cardNumber"] = serde_json::Value::String(reservation.card_number.clone());
            request["page"] = serde_json::Value::Number(1.into());

            let line_info_url = url!("https://www.bustago.or.kr/newweb/kr/reserve/reserveline.do");
            let req = client
                .post(line_info_url.as_ref())
                .header(
                    reqwest::header::REFERER,
                    "https://www.bustago.or.kr/newweb/kr/reserve/reservelist.do",
                )
                .header(reqwest::header::COOKIE, jar.cookies(line_info_url).unwrap())
                .form(&request)
                .build()?;
            let res: LineInfoResponse = client
                .execute(req)
                .await
                .context("Failed to fetch reserveline")?
                .json()
                .await
                .context("Failed to parse reserveline")?;

            let line_info = res.list.last().unwrap();

            let date_begin =
                chrono::NaiveDate::parse_from_str(&reservation.departure_date, "%Y%m%d").unwrap();
            let time_begin =
                chrono::NaiveTime::parse_from_str(&reservation.departure_time, "%H%M").unwrap();

            let mut dt = chrono::NaiveDateTime::new(date_begin, time_begin);
            dt += std::time::Duration::from_secs(line_info.distance_time * 60);

            new_reservations.push(CalendarEvent {
                id,
                title: format!(
                    "{}발 {}행 시외버스",
                    reservation.departure_terminal_name, reservation.arrival_terminal_name
                ),
                detail: format!(
                    "회사: {}\n좌석번호: {}",
                    reservation.operator_name, reservation.total_seat_count
                ),
                invalid: current_invalid,
                date_begin,
                time_begin: Some(time_begin),
                date_end: Some(dt.date()),
                time_end: Some(dt.time()),
                location: None,
                url: None,
            });
        }

        let updated_item_count = if !new_reservations.is_empty() {
            CalendarEvent::upsert_events_to_db(self.user_id, &db, new_reservations.iter()).await?
        } else {
            0
        };
        info!("updated item count: {updated_item_count}",);

        Ok(updated_item_count > 0)
    }

    async fn ping(&self) -> anyhow::Result<()> {
        let jar = self.to_cookie_jar();
        let url = url!("https://www.bustago.or.kr/newweb/kr/mypage/myPage.do");
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
            "SELECT `user_id` as `user_id: UserId`, `jsessionid`, `user_number` FROM `bustago_user`"
        )
        .fetch_optional(&db)
        .await
        .with_context(|| format!("Failed to get bustago_user of {user_id:?}"))
    }

    async fn update_session(&self, db: SqlitePool) -> anyhow::Result<()> {
        sqlx::query!(
            "INSERT INTO `bustago_user` (`jsessionid`, `user_number`, `user_id`) VALUES (?, ?, ?)
                ON CONFLICT (`user_id`) DO UPDATE
                SET `jsessionid` = `excluded`.`jsessionid`, `user_number` = `excluded`.`user_number` WHERE `user_id` = `excluded`.`user_id`",
            self.jsessionid,
            self.user_number,
            self.user_id
        )
        .execute(&db)
        .await
        .context("Failed to update bustago user session data")
        .map(|_| ())
    }
}

pub fn web_router() -> Router {
    crate::user_web_router::<BustagoUser>()
}
