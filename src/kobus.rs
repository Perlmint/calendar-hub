// https://kobus.co.kr/mrs/mrscfm.do

// section.newMobileTicket
// .date
// .departure
// .arrive
// .detail_info
// section.mobileTicket
// JSESSIONID

use anyhow::Context;
use axum::{async_trait, Router};
use chrono::LocalResult;
use futures::StreamExt;
use hyper::StatusCode;
use itertools::Itertools;
use log::info;
use reqwest::cookie::CookieStore;
use scraper::{ElementRef, Html};
use sqlx::SqlitePool;

use crate::{regex, selector, url, CalendarEvent, UserId};

fn parse_ticket(element: ElementRef<'_>, canceled: bool) -> anyhow::Result<CalendarEvent> {
    use chrono::TimeZone;
    let date = element
        .select(selector!(".date"))
        .next()
        .ok_or_else(|| anyhow::anyhow!("Failed to find date from ticket"))?
        .text()
        .join("");
    let date = date.trim();
    let date_matched = regex!(r#"^(\d+)\.\s*(\d+)\.\s*(\d+)[^\d]+(\d+):(\d+)"#)
        .captures(&date)
        .ok_or_else(|| anyhow::anyhow!("Failed to parse date - {}", date))?;
    let LocalResult::Single(date_time) =
        unsafe { chrono::FixedOffset::east_opt(9 * 60 * 60).unwrap_unchecked() }.with_ymd_and_hms(
            date_matched.get(1).unwrap().as_str().parse().unwrap(),
            date_matched.get(2).unwrap().as_str().parse().unwrap(),
            date_matched.get(3).unwrap().as_str().parse().unwrap(),
            date_matched.get(4).unwrap().as_str().parse().unwrap(),
            date_matched.get(5).unwrap().as_str().parse().unwrap(),
            0,
        )
    else {
        return Err(anyhow::anyhow!(
            "Ambiguous or invalid date - {:?}",
            date_matched
        ));
    };
    let begin_date_time = date_time.naive_utc();
    let departure = element
        .select(selector!(".departure"))
        .next()
        .ok_or_else(|| anyhow::anyhow!("Failed to find departure from ticket"))?
        .text()
        .join("");
    let departure = departure.trim();
    let arrive = element
        .select(selector!(".arrive"))
        .next()
        .ok_or_else(|| anyhow::anyhow!("Failed to find arrive from ticket"))?
        .text()
        .join("");
    let arrive = arrive.trim();
    let detail_info = element
        .select(selector!(".detail_info"))
        .next()
        .ok_or_else(|| anyhow::anyhow!("Failed to find detail_info from ticket"))?
        .text()
        .join("");
    let detail_info = detail_info.trim();
    let detail_info = regex!(r#"^((\d+)시간)?\s*((\d+)분)?\s*소요$"#)
        .captures(&detail_info)
        .ok_or_else(|| anyhow::anyhow!("Failed to parse duration - {:?}", detail_info))?;
    let duration = chrono::Duration::minutes(
        (detail_info
            .get(2)
            .map(|i| i.as_str().parse().unwrap())
            .unwrap_or(0)
            * 60)
            + detail_info
                .get(4)
                .map(|i| i.as_str().parse().unwrap())
                .unwrap_or(0),
    );
    let end_date_time = begin_date_time + duration;
    let reservation_number = element
        .select(selector!(".tbl_info tr:first-child td"))
        .next()
        .ok_or_else(|| anyhow::anyhow!("Failed to find reservation_number from ticket"))?
        .text()
        .join("");
    let reservation_number = reservation_number.trim();

    Ok(CalendarEvent {
        id: format!("kobus_{reservation_number}"),
        title: format!("{departure}발 {arrive}행 고속버스"),
        detail: "".to_string(),
        invalid: canceled,
        date_begin: begin_date_time.date(),
        time_begin: Some(begin_date_time.time()),
        date_end: Some(end_date_time.date()),
        time_end: Some(end_date_time.time()),
        location: None,
        url: None,
    })
}

crate::define_user_data! {
    #[table_name = "kobus"]
    #[base_url = "https://kobus.co.kr/"]
    struct KobusUser {
        #[session_name = "JSESSIONID"]
        jsessionid: String,
    }
}

impl KobusUser {
    pub fn all(db: &SqlitePool) -> impl futures::Stream<Item = anyhow::Result<Self>> + '_ {
        sqlx::query_as!(
            Self,
            "SELECT `user_id` as `user_id: UserId`, `jsessionid` FROM `kobus_user`"
        )
        .fetch(db)
        .map(|result| result.context("Failed to get naver_user"))
    }
}

#[async_trait]
impl crate::UserImpl for KobusUser {
    type Detail = KobusUserDetail;
    const PING_INTERVAL: Option<std::time::Duration> =
        Some(std::time::Duration::from_secs(29 * 60));

    async fn fetch(&self, db: SqlitePool) -> anyhow::Result<bool> {
        let jar = self.to_cookie_jar();
        let planned_url = url!("https://kobus.co.kr/mrs/mrscfm.do");
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
        let res = res.bytes().await?;

        let html = std::str::from_utf8(&res)?;
        let events = {
            let fragment = Html::parse_fragment(html);

            fragment
                .select(selector!("section.newMobileTicket"))
                .into_iter()
                .map(|ticket| parse_ticket(ticket, false))
                .collect::<Result<Vec<_>, _>>()?
        };

        let updated_item_count = if events.is_empty() {
            0
        } else {
            CalendarEvent::upsert_events_to_db(self.user_id, &db, events.iter()).await?
        };
        let updated_item_count = updated_item_count
            + CalendarEvent::cancel_not_expired_and_not_in(
                self.user_id,
                &db,
                "kobus_",
                events.iter().map(|event| event.id.as_str()),
            )
            .await?;
        info!("updated item count: {updated_item_count}",);

        Ok(true)
    }

    async fn from_user_id(db: SqlitePool, user_id: UserId) -> anyhow::Result<Option<Self>> {
        sqlx::query_as!(
            Self,
            "SELECT `user_id` as `user_id: UserId`, `jsessionid` FROM `kobus_user`"
        )
        .fetch_optional(&db)
        .await
        .with_context(|| format!("Failed to get kobus_user of {user_id:?}"))
    }

    async fn update_session(&self, db: SqlitePool) -> anyhow::Result<()> {
        sqlx::query!(
            r#"INSERT INTO `kobus_user` (`user_id`, `jsessionid`) VALUES (?, ?)
            ON CONFLICT (`user_id`)
            DO UPDATE SET `jsessionid`=`excluded`.`jsessionid`"#,
            self.user_id,
            self.jsessionid
        )
        .execute(&db)
        .await
        .context("Failed to update kobus user session data")
        .map(|_| ())
    }

    async fn ping(&self) -> anyhow::Result<()> {
        let jar = self.to_cookie_jar();
        let planned_url = url!("https://kobus.co.kr/mrs/mrscfm.do");
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
    crate::user_web_router::<KobusUser>()
}
