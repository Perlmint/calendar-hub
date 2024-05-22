// cSpell:ignore runningtime Regist Reserv
use std::str::FromStr;

use anyhow::Context as _;
use axum::{async_trait, Router};
use chrono::Datelike;
use futures::StreamExt;
use hyper::StatusCode;
use itertools::Itertools as _;
use log::info;
use reqwest::{
    cookie::{CookieStore as _, Jar},
    Client,
};
use scraper::Html;
use sqlx::SqlitePool;
use std::fmt::Write;

use crate::{date_time_to_utc, regex, selector, url, CalendarEvent, UserId};

#[derive(serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ReservationListResponseData {
    pub reservation_list_html: Option<String>,
}

#[derive(serde::Deserialize)]
struct ReservationListResponse {
    #[serde(rename = "d")]
    pub data: ReservationListResponseData,
}

crate::define_user_data! {
    #[table_name = "cgv"]
    #[base_url = "https://m.cgv.co.kr/"]
    struct CgvUser {
        #[session_name = "WEBAUTH"]
        webauth: String,
        #[session_name = ".ASPXAUTH"]
        aspxauth: String,
    }
}

impl CgvUser {
    pub fn all(db: &SqlitePool) -> impl futures::Stream<Item = anyhow::Result<Self>> + '_ {
        sqlx::query_as!(
            Self,
            "SELECT `user_id` as `user_id: UserId`, `webauth`, `aspxauth` FROM `cgv_user`"
        )
        .fetch(db)
        .map(|result| result.context("Failed to get cgv_user"))
    }
}

async fn fetch_detail(
    client: &Client,
    jar: &Jar,
    id: &str,
    year: i32,
) -> anyhow::Result<CalendarEvent> {
    // remove prefix
    let cgv_id = &id[4..];
    info!("Crawl detail for {cgv_id}");
    let detail_url = reqwest::Url::from_str(&format!(
        "https://m.cgv.co.kr/WebApp/MyCgvV5/reservationDetail.aspx?bookingnumber={cgv_id}"
    ))?;

    let cookie = jar.cookies(&detail_url).unwrap();
    let req = client
        .get(detail_url)
        .header(reqwest::header::COOKIE, cookie)
        .build()?;
    let res = client.execute(req).await?.bytes().await?;
    let html = std::str::from_utf8(&res)?;
    let fragment = Html::parse_fragment(&html);
    let movie_title = fragment
        .select(selector!(".movie-tit"))
        .next()
        .ok_or_else(|| anyhow::anyhow!("Could not find title"))?
        .text()
        .map(|t| t.to_string())
        .join("");

    let date_time_element = fragment
        .select(selector!(".date-n-runningtime"))
        .next()
        .ok_or_else(|| anyhow::anyhow!("Could not find date/time wrap"))?;
    let mut date = None;
    let mut time_begin = None;
    let mut time_end = None;
    for div in date_time_element.select(selector!("div")) {
        let key = div
            .select(selector!(".inner-tit"))
            .next()
            .ok_or_else(|| anyhow::anyhow!("date/time inner-tit is not found"))?
            .text()
            .join("");
        let value = div
            .select(selector!(".inner-cnt"))
            .next()
            .ok_or_else(|| anyhow::anyhow!("date/time inner-cnt is not found"));
        match key.as_str() {
            "상영일" => {
                let s = value.context("date content")?.text().join("");
                if let Some(c) = regex!("(\\d+)/(\\d+)").captures(&s) {
                    let (month, day) =
                        unsafe { (c.get(1).unwrap_unchecked(), c.get(2).unwrap_unchecked()) };
                    let month: u32 = unsafe { month.as_str().parse().unwrap_unchecked() };
                    let day: u32 = unsafe { day.as_str().parse().unwrap_unchecked() };

                    date = Some(
                        chrono::NaiveDate::from_ymd_opt(year, month, day).ok_or_else(|| {
                            anyhow::anyhow!("Failed to convert begin date - {month}-{day}: {s}")
                        })?,
                    );
                }
            }
            "상영시간" => {
                let s = value.context("time content")?.text().join("");
                if let Some(c) = regex!("(\\d+):(\\d+)\\s*~\\s*(\\d+):(\\d+)").captures(&s) {
                    let (begin_hour, begin_minute, end_hour, end_minute) = unsafe {
                        (
                            c.get(1).unwrap_unchecked(),
                            c.get(2).unwrap_unchecked(),
                            c.get(3).unwrap_unchecked(),
                            c.get(4).unwrap_unchecked(),
                        )
                    };
                    let begin_hour: u32 = unsafe { begin_hour.as_str().parse().unwrap_unchecked() };
                    let begin_minute: u32 =
                        unsafe { begin_minute.as_str().parse().unwrap_unchecked() };
                    let end_hour: u32 = unsafe { end_hour.as_str().parse().unwrap_unchecked() };
                    let end_minute: u32 = unsafe { end_minute.as_str().parse().unwrap_unchecked() };

                    time_begin = Some(
                        chrono::NaiveTime::from_hms_opt(begin_hour, begin_minute, 0).ok_or_else(
                            || {
                                anyhow::anyhow!(
                                    "Failed to convert begin time - {begin_hour}:{begin_minute} - {s}"
                                )
                            },
                        )?,
                    );
                    time_end = Some(
                        chrono::NaiveTime::from_hms_opt(
                            end_hour.checked_sub(24).unwrap_or(end_hour),
                            end_minute,
                            0,
                        )
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "Failed to convert end time - {end_hour}:{end_minute} - {s}"
                            )
                        })?,
                    );
                }
            }
            _ => continue,
        }
    }
    let (Some(date), Some(time_begin), Some(time_end)) = (date, time_begin, time_end) else {
        return Err(anyhow::anyhow!("Could not find date or time element"));
    };
    let (date_begin, date_end) = if time_end > time_begin {
        (date, date)
    } else {
        (
            date,
            date.succ_opt()
                .ok_or_else(|| anyhow::anyhow!("Could not get next day of {:?}", date))?,
        )
    };
    let (date_begin, time_begin) = date_time_to_utc(
        date_begin,
        time_begin,
        chrono::FixedOffset::east_opt(9 * 60 * 60).unwrap(),
    );
    let (date_end, time_end) = date_time_to_utc(
        date_end,
        time_end,
        chrono::FixedOffset::east_opt(9 * 60 * 60).unwrap(),
    );

    let ticket_detail_element = fragment
        .select(selector!(".ticket-detail"))
        .next()
        .ok_or_else(|| anyhow::anyhow!("Could not find ticket-detail"))?;
    let mut theater = None;
    let mut hall = None;
    let mut seat = None;
    for dl in ticket_detail_element.select(selector!("dl")) {
        let (Some(dt), Some(dd)) = (
            dl.select(selector!("dt")).next(),
            dl.select(selector!("dd")).next(),
        ) else {
            continue;
        };
        let key = dt.text().join("");
        match key.as_str() {
            "극장" => {
                let value = dd.text().join("");
                theater = Some(value);
            }
            "상영관" => {
                let value = dd.text().join("");
                hall = Some(value);
            }
            "좌석" => {
                let value = dd.text().join("");
                seat = Some(value);
            }
            _ => continue,
        }
    }
    let Some(theater) = theater else {
        return Err(anyhow::anyhow!("Could not find theater name"));
    };
    let mut detail = String::new();
    if let Some(hall) = hall {
        writeln!(detail, "상영관: {}", hall)?;
    }
    if let Some(seat) = seat {
        writeln!(detail, "좌석: {}", seat)?;
    }

    let url =
        format!("https://m.cgv.co.kr/WebApp/MyCgvV5/reservationDetail.aspx?bookingnumber={cgv_id}");

    Ok(CalendarEvent {
        id: id.to_string(),
        title: format!("{movie_title} - {theater}"),
        detail,
        invalid: false,
        date_begin,
        time_begin: Some(time_begin),
        date_end: Some(date_end),
        time_end: Some(time_end),
        location: Some(theater),
        url: Some(url),
    })
}

#[async_trait]
impl crate::UserImpl for CgvUser {
    type Detail = CgvUserDetail;

    const PING_INTERVAL: Option<std::time::Duration> =
        Some(std::time::Duration::from_secs(29 * 60));

    async fn fetch(&self, db: SqlitePool) -> anyhow::Result<bool> {
        let jar = self.to_cookie_jar();
        let reservation_list_page_url =
            url!("https://m.cgv.co.kr/WebApp/MyCgvV5/paymentList.aspx/GetReservationListPaging");
        let client = reqwest::Client::new();
        let now_in_utc9 = chrono::Local::now()
            .with_timezone(&unsafe { chrono::FixedOffset::east_opt(9).unwrap_unchecked() });
        let end_dt = now_in_utc9.format("%Y-%m-%d").to_string();
        let start_dt = (now_in_utc9 - chrono::Duration::days(7))
            .format("%Y-%m-%d")
            .to_string();
        let request_data = serde_json::to_string(&serde_json::json!({
            "UserId": "",
            "Ssn": "",
            "AppType": "",
            "RegistSite": "",
            "BookingStateCd": "A",
            "SortCd": "R",
            "SelectStartDT": start_dt,
            "SelectEndDT": end_dt,
            "ShowCnt": 10,
            "NowPage":1
        }))
        .unwrap();
        let req = client
            .post(reservation_list_page_url.as_ref())
            .header(
                reqwest::header::COOKIE,
                jar.cookies(reservation_list_page_url).unwrap(),
            )
            .json(&serde_json::json!({ "requestData": request_data }))
            .build()?;
        let res: ReservationListResponse = client.execute(req).await?.json().await?;
        let Some(html) = res.data.reservation_list_html else {
            return Ok(false);
        };

        let item_regex = regex!("javascript:fnReservDetail\\('([^']+)'\\)");
        let captures = item_regex.captures_iter(&html);

        let ids = captures
            .filter_map(|capture| capture.get(1).map(|i| format!("cgv/{}", i.as_str())))
            .collect::<Vec<_>>();

        let new_ids = CalendarEvent::filter_ids(self.user_id, &db, &ids).await?;
        let mut reservations = Vec::with_capacity(new_ids.len());
        for id in new_ids {
            let reservation = fetch_detail(&client, &jar, id, now_in_utc9.year()).await?;
            reservations.push(reservation);
        }

        let updated_item_count = if reservations.is_empty() {
            0
        } else {
            CalendarEvent::upsert_events_to_db(self.user_id, &db, reservations.iter()).await?
        };
        info!("updated item count: {updated_item_count}",);

        Ok(updated_item_count > 0)
    }

    async fn from_user_id(db: SqlitePool, user_id: UserId) -> anyhow::Result<Option<Self>> {
        sqlx::query_as!(
            Self,
            "SELECT `user_id` as `user_id: UserId`, `webauth`, `aspxauth` FROM `cgv_user`"
        )
        .fetch_optional(&db)
        .await
        .with_context(|| format!("Failed to get cgv_user of {user_id:?}"))
    }

    async fn update_session(&self, db: SqlitePool) -> anyhow::Result<()> {
        sqlx::query!(
            "INSERT INTO `cgv_user` (`webauth`, `aspxauth`, `user_id`) VALUES (?, ?, ?)
                ON CONFLICT (`user_id`) DO UPDATE
                SET `webauth` = `excluded`.`webauth`, `aspxauth` = `excluded`.`aspxauth` WHERE `user_id` = `excluded`.`user_id`",
            self.webauth,
            self.aspxauth,
            self.user_id
        )
        .execute(&db)
        .await
        .context("Failed to update catch table user session data")
        .map(|_| ())
    }

    async fn ping(&self) -> anyhow::Result<()> {
        let jar = self.to_cookie_jar();
        let planned_url = url!("https://m.cgv.co.kr/");
        let client = reqwest::Client::new();
        let req = client
            .post(planned_url.as_ref())
            .header(reqwest::header::COOKIE, jar.cookies(planned_url).unwrap())
            .build()?;

        let res = client.execute(req).await?;

        if res.status() != StatusCode::OK {
            return Err(anyhow::anyhow!(
                "Failed to fetch data. Session could be expired"
            ));
        }

        Ok(())
    }
}

pub fn web_router() -> Router {
    crate::user_web_router::<CgvUser>()
}
