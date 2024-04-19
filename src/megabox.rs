use anyhow::Context;
use axum::{async_trait, Router};
use futures::StreamExt;
use log::info;
use reqwest::cookie::CookieStore;
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::{date_time_to_utc, url, CalendarEvent, UserId};

#[derive(Debug, Deserialize)]
struct ReservationResponse {
    #[serde(rename = "statCd")]
    status_code: i32,
    #[serde(rename = "msg")]
    message: String,
    // #[serde(rename = "imgSvrUrl")]
    // image_server_url: String,
    #[serde(rename = "list")]
    items: Vec<Reservation>,
}

#[derive(Debug, Deserialize)]
struct Reservation {
    // sellTranNo: String,
    // sellStatCd: String,
    #[serde(rename = "bokdNo")]
    booking_id: String,
    // sellItemNo: String,
    // payDe: String,
    // payDt: String,
    // imgPath: String,
    // playDt: String,
    #[serde(rename = "movieNm")]
    movie_name: String,
    #[serde(rename = "brchNm")]
    branch_name: String,
    #[serde(rename = "theabNm")]
    theater_name: String,
    // #[serde(rename = "movieKindNm")]
    // movie_kind_name: String,
    // movieEventTyNm: Option<?>,
    #[serde(rename = "theabFlrNm")]
    theater_floor_name: String,
    // brchNo: String,
    // movieNo: String,
    // movieCttsTyCd: String,
    // playSeq: i32,
    // hotdealStatCd: Option<()>,
    // #[serde(rename = "rpstMovieNo")]
    // movie_id: String,
    #[serde(rename = "seatNm")]
    seat_name: String,
    // admisPcnt: String,
    // resvrPoint: i32,
    // drnkAddStatCd: String,
    // playDayAt: String,
    // playOverAt: String,
    // playAt: String,
    // mbIdntfcDivCd: String,
    // theabKindCd: String,
    // payAmt: i32,
    // prdtAmt: i32,
    #[serde(rename = "playDe")]
    play_date: String,
    #[serde(rename = "playStartTime")]
    play_start_time: String,
    #[serde(rename = "playEndTime")]
    play_end_time: String,
    // dowNm: String,
    // fstRegDtFmt: String,
    // fstRegDt: String,
    // sellStatNm: String,
    // privateYn: String,
    // privateCancelYn: String,
    // privateCancelCtrlTime: Option<()>,
    // custNm: String,
    // chkStrDe: String,
    // chkEndDe: String,
    // okSaveAmt: i32,
    // privPackList: Option<()>,
    // totCnt: i32,
    // goodsAcptChk: String,
    // currentPage: i32,
    // recordCountPerPage: i32,
}

impl TryFrom<Reservation> for Option<CalendarEvent> {
    type Error = anyhow::Error;

    fn try_from(value: Reservation) -> Result<Self, Self::Error> {
        let id = format!("megabox/{}", value.booking_id);
        let title = format!("{} - MEGABOX {}", value.movie_name, value.branch_name);
        let detail = format!(
            "상영관: {}({})\n좌석: {}",
            value.theater_name, value.theater_floor_name, value.seat_name
        );
        let date_begin = chrono::NaiveDate::parse_from_str(&value.play_date, "%Y%m%d")
            .context("Failed to parse date")?;
        let time_begin: u32 = value
            .play_start_time
            .parse()
            .context("Failed to parse time")?;
        let time_end: u32 = value
            .play_end_time
            .parse()
            .context("Failed to parse time")?;
        let hour_begin = time_begin / 100;
        let minute_begin = time_begin % 100;
        let hour_end = time_end / 100;
        let minute_end = time_end % 100;
        let (date_end, time_end) = if hour_end < 24 {
            (
                date_begin,
                chrono::NaiveTime::from_hms_opt(hour_end, minute_end, 0).unwrap(),
            )
        } else {
            (
                date_begin.succ_opt().unwrap(),
                chrono::NaiveTime::from_hms_opt(hour_end - 24, minute_end, 0).unwrap(),
            )
        };
        let (date_begin, time_begin) = if hour_begin < 24 {
            (
                date_begin,
                chrono::NaiveTime::from_hms_opt(hour_begin, minute_begin, 0).unwrap(),
            )
        } else {
            (
                date_begin.succ_opt().unwrap(),
                chrono::NaiveTime::from_hms_opt(hour_begin - 24, minute_begin, 0).unwrap(),
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

        Ok(Some(CalendarEvent {
            id,
            title,
            detail,
            invalid: false,
            date_begin,
            time_begin: Some(time_begin),
            date_end: Some(date_end),
            time_end: Some(time_end),
            location: None,
            url: None,
        }))
    }
}

crate::define_user_data! {
    #[table_name = "megabox"]
    #[base_url = "https://www.megabox.co.kr/"]
    struct MegaboxUser {
        #[session_name = "JSESSIONID"]
        jsessionid: String,
        #[session_name = "SESSION"]
        session: String,
    }
}

impl MegaboxUser {
    pub fn all(db: &SqlitePool) -> impl futures::Stream<Item = anyhow::Result<Self>> + '_ {
        sqlx::query_as!(
            Self,
            "SELECT `user_id` as `user_id: UserId`, `jsessionid`, `session` FROM `megabox_user`"
        )
        .fetch(db)
        .map(|result| result.context("Failed to get megabox_user"))
    }
}

#[async_trait]
impl crate::UserImpl for MegaboxUser {
    type Detail = MegaboxUserDetail;

    const PING_INTERVAL: Option<std::time::Duration> =
        Some(std::time::Duration::from_secs(10 * 60));

    async fn fetch(&self, db: SqlitePool) -> anyhow::Result<bool> {
        let jar = self.to_cookie_jar();
        let planned_url = url!("https://www.megabox.co.kr/on/oh/ohh/MyBokdPurc/selectBokdList.do");
        let client = reqwest::Client::new();
        let req = client
            .get(planned_url.as_ref())
            .header(
                reqwest::header::REFERER,
                "https://www.megabox.co.kr/mypage/bookinglist",
            )
            .header(reqwest::header::COOKIE, jar.cookies(planned_url).unwrap())
            .json(&serde_json::json!({
                "divCd": "B",
                "localeCode": "kr"
            }))
            .build()?;
        let res: ReservationResponse = client.execute(req).await?.json().await?;
        if res.status_code != 0 {
            return Err(anyhow::anyhow!("Receive error response - {}", res.message));
        }
        if res.items.is_empty() {
            return Ok(false);
        }

        let reservations = res
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
        let url = url!("https://www.megabox.co.kr/sessionChk.do");
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
            "SELECT `user_id` as `user_id: UserId`, `jsessionid`, `session` FROM `megabox_user`"
        )
        .fetch_optional(&db)
        .await
        .with_context(|| format!("Failed to get megabox_user of {user_id:?}"))
    }

    async fn update_session(&self, db: SqlitePool) -> anyhow::Result<()> {
        sqlx::query!(
            "INSERT INTO `megabox_user` (`jsessionid`, `session`, `user_id`) VALUES (?, ?, ?)
                ON CONFLICT (`user_id`) DO UPDATE
                SET `jsessionid` = `excluded`.`jsessionid`, `session` = `excluded`.`session` WHERE `user_id` = `excluded`.`user_id`",
            self.jsessionid,
            self.session,
            self.user_id
        )
        .execute(&db)
        .await
        .context("Failed to update megabox user session data")
        .map(|_| ())
    }
}

pub fn web_router() -> Router {
    crate::user_web_router::<MegaboxUser>()
}
