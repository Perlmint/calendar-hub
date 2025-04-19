// cSpell:ignore appv birthdate cancle cardno ccard eter routecode sdate stime bizr txtid txtpw reservejson reserveline
use std::{collections::HashMap, sync::Arc};

use anyhow::Context;
use chrono::Datelike;
use serde::Deserialize;
use sqlx::Row;
use tracing::info;

use crate::{
    prelude::*,
    server::prelude::{common::*, reservation::*, user::*},
};

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
    original_reservation_number: String,
    #[serde(rename = "reserve_cd")]
    reservation_code: String,
    #[serde(rename = "reserve_no")]
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
    #[serde(rename = "dist_time")]
    distance_time: u64,
}

define_user_data! {
    #[base_url = "https://www.bustago.or.kr/"]
    struct BustagoCookie(
        "JSESSIONID"
    )
}

fn to_numeric_date(date: chrono::NaiveDate) -> String {
    format!("{:04}{:02}{:02}", date.year(), date.month(), date.day())
}

const REQUIRED_STRING_FIELDS: &[&str] = &[
    "fromDate",
    "toDate",
    "v_dateGb",
    "v_status",
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
    "page",
    "startDateParam",
    "EndDateParam",
    "userNumber",
    "tokenId",
];

pub(super) async fn crawl(
    config: super::Config,
    user_id: UserId,
    db: &SqlitePool,
) -> anyhow::Result<usize> {
    let (jar, user_number) = flatten_error(
        tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            use headless_chrome::protocol::cdp::types::Event;
            let browser = open_browser()?;

            let tab = browser.new_tab()?;
            info!("Open Bustago login page");
            tab.navigate_to("https://www.bustago.or.kr/newweb/kr/member/login.do")?;

            let handler = tab.add_event_listener(Arc::new({
                let tab = tab.clone();
                move |event: &Event| match event {
                    Event::PageJavascriptDialogOpening(event) => {
                        info!("dialog - {}", event.params.message);
                        let dialog = tab.get_dialog();
                        let dialog_ret = if event.params.message.contains("로그인 하시겠습니까?")
                        {
                            dialog.accept(None)
                        } else {
                            dialog.dismiss()
                        };
                        if let Err(e) = dialog_ret {
                            error!("dialog close error - {e:?}");
                        }
                    }
                    _ => {}
                }
            }))?;

            info!("Try login");
            tab.wait_for_element("#txtid")?
                .focus()?
                .type_into(&config.user_id)?;
            tab.find_element("#txtpw")?
                .focus()?
                .type_into(&config.password)?;
            tab.find_element("#loginBtn")?.click()?;
            info!("Wait page transition");
            tab.wait_for_element(".top_name")?;
            tab.navigate_to("https://www.bustago.or.kr/newweb/kr/reserve/reservelist.do")?;
            tab.wait_for_element("input#userNumber")?;

            info!("login success");

            let user_number = tab
                .evaluate("userNumberParam", false)?
                .value
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned();

            let jar = BustagoCookie::from_chrome_tab(&tab)?;
            tab.remove_event_listener(&handler)?;
            tab.close(false)?;

            Ok((jar, user_number))
        })
        .await
        .map_err(|e| anyhow::anyhow!("join error - {e:?}")),
    )?;

    let date_end = chrono::Utc::now()
        .with_timezone(
            &chrono::FixedOffset::east_opt(9 * 60 * 60)
                .ok_or_else(|| anyhow::anyhow!("Failed to get FixedOffset"))?,
        )
        .date_naive();
    let date_begin = date_end - chrono::Duration::days(7);
    let reservations_url = url!("https://www.bustago.or.kr/newweb/kr/reserve/reservejson.do");
    let client = reqwest::Client::new();
    // query by making reservation date
    let mut request: Vec<(&str, String)> = REQUIRED_STRING_FIELDS
        .iter()
        .map(|key| {
            (
                *key,
                match *key {
                    "fromDate" => to_numeric_date(date_begin),
                    "toDate" => to_numeric_date(date_end),
                    "v_dateGb" => 1.to_string(),
                    "v_status" => 0.to_string(),
                    "page" => 1.to_string(),
                    "userNumber" => user_number.to_string(),
                    _ => "".to_string(),
                },
            )
        })
        .collect();

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
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .form(&request)
        .build()?;
    debug!("{req:?}\n{}", unsafe {
        req.body()
            .and_then(|b| b.as_bytes())
            .map(|b| std::str::from_utf8_unchecked(b))
            .unwrap_unchecked()
    });
    let res = client
        .execute(req)
        .await
        .context("Failed to fetch reservejson")?;
    let res_body = res
        .text()
        .await
        .context("Failed to read reservejson body as string")?;
    let res: ReservationResponse = serde_json::from_str(&res_body)
        .with_context(|| format!("Failed to parse reservejson - raw:\n{res_body}"))?;
    if res.items.is_empty() {
        return Ok(0);
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
    .push_bind(user_id)
    .push("AND `id` in ")
    .push_tuples(&ids, |mut builder, item| {
        builder.push_bind(item);
    })
    .build()
    .fetch_all(db)
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
                        user_id,
                        id
                    )
                    .execute(db)
                    .await
                    .context("Failed to set invalid")?;
                }
            }
            continue;
        }

        for (key, value) in request.iter_mut() {
            let new_value = match *key {
                "routecode" => reservation.route_code.clone(),
                "sterCode" => reservation.departure_terminal_code.clone(),
                "eterCode" => reservation.arrival_terminal_code.clone(),
                "sterName" => reservation.departure_terminal_name.clone(),
                "eterName" => reservation.arrival_terminal_name.clone(),
                "reserveNo" => reservation.original_reservation_number.clone(),
                "sdate" => reservation.departure_date.clone(),
                "totalSeat" => reservation.total_seat_count.clone(),
                "startTime" => reservation.departure_time.clone(),
                "reserveTime" => reservation.reservation_date.clone(),
                "stime" => reservation.departure_time.clone(),
                "appv_no" => reservation.approval_number.clone(),
                "org_reserve_no" => reservation.original_reservation_number.clone(),
                "org_reserve_time" => reservation.reservation_date.clone(),
                "reserve_cd" => reservation.reservation_code.clone(),
                "card_No" => reservation.card_number.clone(),
                "ticket_no" => reservation.original_reservation_number.clone(),
                "cardNumber" => reservation.card_number.clone(),
                "page" => 1.to_string(),
                _ => {
                    continue;
                }
            };
            *value = new_value;
        }

        let line_info_url = url!("https://www.bustago.or.kr/newweb/kr/reserve/reserveline.do");
        let req = client
            .post(line_info_url.as_ref())
            .header(
                reqwest::header::REFERER,
                "https://www.bustago.or.kr/newweb/kr/reserve/reservelist.do",
            )
            .header(
                reqwest::header::COOKIE,
                jar.cookies(line_info_url)
                    .ok_or_else(|| anyhow::anyhow!("Failed to get cookie"))?,
            )
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .form(&request)
            .build()?;
        let res: LineInfoResponse = client
            .execute(req)
            .await
            .context("Failed to fetch reserveline")?
            .json()
            .await
            .context("Failed to parse reserveline")?;

        let distance_time: u64 = res
            .list
            .iter()
            .map(|v| v.distance_time)
            .sum();

        let date_begin = chrono::NaiveDate::parse_from_str(&reservation.departure_date, "%Y%m%d")?;
        let time_begin = chrono::NaiveTime::parse_from_str(&reservation.departure_time, "%H%M")?;

        let (date_begin, time_begin) = date_time_to_utc(
            date_begin,
            time_begin,
            chrono::FixedOffset::east_opt(9 * 60 * 60)
                .ok_or_else(|| anyhow::anyhow!("Failed to get FixedOffset"))?,
        );
        let mut dt = chrono::NaiveDateTime::new(date_begin, time_begin);
        dt += std::time::Duration::from_secs(distance_time * 60);

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

    let logout_url = url!("https://www.bustago.or.kr/newweb/kr/member/loginOut.do");

    let req = client
        .post(logout_url.as_ref())
        .header(
            reqwest::header::REFERER,
            "https://www.bustago.or.kr/newweb/kr/reserve/reservelist.do",
        )
        .header(
            reqwest::header::COOKIE,
            jar.cookies(logout_url)
                .ok_or_else(|| anyhow::anyhow!("Failed to get cookie"))?,
        )
        .form(&request)
        .build()?;

    if let Err(e) = client.execute(req).await {
        error!("Failed to logout - {e:?}");
    }

    let updated_item_count = if !new_reservations.is_empty() {
        CalendarEvent::upsert_events_to_db(user_id, &db, new_reservations.iter()).await?
    } else {
        0
    };
    info!("updated item count: {updated_item_count}",);

    Ok(updated_item_count as _)
}
