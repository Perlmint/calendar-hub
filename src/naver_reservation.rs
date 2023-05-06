use std::str::FromStr;

use anyhow::{anyhow, Context};
#[allow(unused_imports)]
use chrono::Timelike; // false warning
use chrono::{Datelike, FixedOffset};
use futures::StreamExt;
use itertools::Itertools;
use log::info;
use reqwest::cookie::{CookieStore, Jar};
use scraper::Html;
use sqlx::SqlitePool;

use crate::{regex, selector, url, CalendarEvent, UserId, USER_AGENT};

// when has time, converted in UTC
type DateOptionalTime = (chrono::NaiveDate, Option<chrono::NaiveTime>);

const NAVER_RESERVATION_OFFSET: i32 = 9 * 3600;

fn parse_date_time(s: &str) -> anyhow::Result<DateOptionalTime> {
    fn impl_parse(s: &str) -> anyhow::Result<DateOptionalTime> {
        let date_regex = regex!(
            r"([0-9]{1,2})\.\s*([0-9]{1,2})\s*[월화수목금토일](?:\s*(오전|오후)\s*(?:([0-9]{1,2}):([0-6][0-9])))?"
        );
        let captures = date_regex.captures(s).context("Match REGEX failed")?;
        let month: u32 = captures[1].parse().context("Could not find month")?;
        let day: u32 = captures[2].parse().context("Could not find day")?;

        let time = if let Some(hour) = captures.get(4) {
            let hour: u32 = hour.as_str().parse().context("Failed to parse hour")?;
            let minute: u32 = captures[5]
                .strip_prefix('0')
                .unwrap_or_else(|| &captures[5])
                .parse()
                .context("Failed to parse minute")?;

            Some(
                chrono::NaiveTime::from_hms_opt(
                    hour + if &captures[3] == "오전" { 0 } else { 12 },
                    minute,
                    0,
                )
                .ok_or_else(|| anyhow!("Failed to convert to time"))?,
            )
        } else {
            None
        };

        let date = chrono::NaiveDate::from_ymd_opt(chrono::Local::now().year(), month, day)
            .ok_or_else(|| anyhow!("Failed to convert to date"))?;

        if let Some(time) = time {
            let date_time = unsafe {
                chrono::NaiveDateTime::new(date, time)
                    .and_local_timezone({
                        FixedOffset::east_opt(NAVER_RESERVATION_OFFSET).unwrap_unchecked()
                    })
                    .latest()
                    .unwrap_unchecked()
            }
            .naive_utc();
            Ok((date_time.date(), Some(date_time.time())))
        } else {
            Ok((date, None))
        }
    }

    impl_parse(s).with_context(|| format!("Failed to parse date and optional time {s}"))
}

#[test]
fn test_parse_date_time() {
    let result = parse_date_time("4. 7 금").unwrap();
    assert_eq!(result.0.month(), 4);
    assert_eq!(result.0.day(), 7);
    assert_eq!(result.1, None);

    let result = parse_date_time("4. 27 목 오후 6:00").unwrap();
    assert_eq!(result.0.month(), 4);
    assert_eq!(result.0.day(), 27);
    let result = result.1.unwrap();
    assert_eq!(result.hour(), 9);
    assert_eq!(result.minute(), 0);

    let result = parse_date_time("4. 27 목 오전 6:00").unwrap();
    assert_eq!(result.0.month(), 4);
    assert_eq!(result.0.day(), 26);
    let result = result.1.unwrap();
    assert_eq!(result.hour(), 21);
    assert_eq!(result.minute(), 0);
}

fn parse_date_time_range(s: &str) -> anyhow::Result<(DateOptionalTime, Option<DateOptionalTime>)> {
    let (begin, end) = if let Some((begin, end)) = s.split_once('~') {
        (begin, Some(end))
    } else {
        (s, None)
    };

    Ok((
        parse_date_time(begin)?,
        end.map(parse_date_time).transpose()?,
    ))
}

#[test]
fn test_parse_date_time_range() {
    let (begin, end) = parse_date_time_range("4. 7 금 ~ 4. 8 토").unwrap();
    assert_eq!(begin.0.month(), 4);
    assert_eq!(begin.0.day(), 7);
    assert_eq!(begin.1, None);
    let end = end.unwrap();
    assert_eq!(end.0.month(), 4);
    assert_eq!(end.0.day(), 8);
    assert_eq!(end.1, None);
}

pub struct NaverUser {
    user_id: UserId,
    aut: String,
    ses: String,
}

impl NaverUser {
    pub fn user_id(&self) -> UserId {
        self.user_id
    }

    fn to_cookie_jar(&self) -> Jar {
        let endpoint_base = url!("https://m.booking.naver.com/");
        let jar = Jar::default();
        jar.add_cookie_str(&format!("{}={}", "NID_AUT", self.aut), endpoint_base);
        jar.add_cookie_str(&format!("{}={}", "NID_SES", self.ses), endpoint_base);
        jar
    }

    pub async fn from_user_id(db: SqlitePool, user_id: UserId) -> anyhow::Result<Option<Self>> {
        sqlx::query_as!(
            Self,
            "SELECT `user_id` as `user_id: UserId`, `aut`, `ses` FROM `naver_user`"
        )
        .fetch_optional(&db)
        .await
        .with_context(|| format!("Failed to get naver_user of {user_id:?}"))
    }

    pub fn all(db: &SqlitePool) -> impl futures::Stream<Item = anyhow::Result<Self>> + '_ {
        sqlx::query_as!(
            Self,
            "SELECT `user_id` as `user_id: UserId`, `aut`, `ses` FROM `naver_user`"
        )
        .fetch(db)
        .map(|result| result.context("Failed to get naver_user"))
    }

    pub async fn fetch(&self, db: SqlitePool) -> anyhow::Result<bool> {
        let jar = self.to_cookie_jar();

        let main_url = url!("https://m.booking.naver.com/my/bookings");

        let client = reqwest::Client::new();
        let req = client
            .post(main_url.as_ref())
            .header(reqwest::header::COOKIE, jar.cookies(main_url).unwrap())
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .build()?;

        let res = client.execute(req).await?;
        let res = res.bytes().await?;

        let scrapped_reservations = {
            let fragment = Html::parse_fragment(std::str::from_utf8(&res)?);

            fragment
                .select(selector!(".upcoming_item .info_link_area"))
                .map(|item| {
                    let detail_link = item
                        .value()
                        .attr("href")
                        .ok_or_else(|| anyhow::anyhow!("detail link href is not found"))?;
                    let id = format!(
                        "naver/{}",
                        detail_link
                            .rsplit('/')
                            .next()
                            .ok_or_else(|| anyhow::anyhow!(""))?
                    );
                    let title = item
                        .select(selector!(".title"))
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("Cannot find title"))?
                        .text()
                        .join("");
                    let date = item
                        .select(selector!(".date"))
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("Cannot find date"))?
                        .text()
                        .join("");
                    let detail = item
                        .select(selector!(".txt"))
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("Cannot find detail"))?
                        .text()
                        .join("");
                    let ((date_begin, time_begin), end) = parse_date_time_range(&date)?;
                    let (date_end, time_end) = end.map_or((None, None), |(a, b)| (Some(a), b));
                    let canceled = false;

                    Ok(CalendarEvent {
                        id,
                        title,
                        detail,
                        invalid: canceled,
                        date_begin,
                        time_begin,
                        date_end,
                        time_end,
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?
        };

        if scrapped_reservations.is_empty() {
            Ok(false)
        } else {
            let updated_item_count =
                CalendarEvent::upsert_events_to_db(self.user_id, &db, scrapped_reservations.iter())
                    .await?;
            info!("updated item count: {updated_item_count}",);

            Ok(updated_item_count > 0)
        }
    }
}
