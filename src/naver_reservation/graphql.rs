use std::{fmt::Display, ops::Add};

use anyhow::{anyhow, Context};
use reqwest::cookie::{CookieStore, Jar};

use crate::{url, CalendarEvent, USER_AGENT};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
enum ReservationStatusCode {
    #[serde(rename = "RC02")]
    Requested,
    #[serde(rename = "RC03")]
    Confirmed,
    #[serde(rename = "RC04")]
    Cancelled,
    #[serde(rename = "RC05")]
    NoShowed,
    #[serde(rename = "RC06")]
    CancelledByChange,
    #[serde(rename = "RC08")]
    Completed,
}

impl Display for ReservationStatusCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let stringified = unsafe { serde_json::to_string(self).unwrap_unchecked() };
        write!(f, "{}", stringified.trim_matches('"'))
    }
}

#[serde_with::serde_as]
#[derive(serde::Serialize)]
struct QueryType(
    #[serde_as(
        as = "serde_with::StringWithSeparator::<serde_with::formats::CommaSeparator, ReservationStatusCode>"
    )]
    Vec<ReservationStatusCode>,
);

#[derive(Debug, Clone, serde::Deserialize)]
struct NaverCalendarResponse {
    data: Data,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct Data {
    booking: Booking2,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Booking2 {
    bookings: Vec<BookingWrap>,
    // total_count: u32,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BookingWrap {
    booking_status_code: ReservationStatusCode,
    // is_completed: bool,
    // start_date: chrono::NaiveDate,
    // end_date: chrono::NaiveDate,
    snapshot_json: Booking,
}

#[derive(Debug, Clone, serde::Deserialize)]
enum BookingTimeUnitCode {
    #[serde(rename = "RT00")]
    EveryMinute,
    #[serde(rename = "RT01")]
    Every30Minute,
    #[serde(rename = "RT02")]
    Hourly,
    #[serde(rename = "RT03")]
    Daily,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Booking {
    booking_id: i64,
    // business: any
    // business_id: i64,
    // business_name: String,
    service_name: String,
    // cancelled_date_time: Option<chrono::DateTime<chrono::FixedOffset>>,
    // completed_date_time: chrono::DateTime<chrono::FixedOffset>,
    // regDatetime: any
    #[serde(rename = "bizItemName")]
    business_item_name: String,
    // #[serde(rename = "bizItemId")]
    // business_item_id: i64,
    start_date_time: chrono::DateTime<chrono::Utc>,
    end_date_time: chrono::DateTime<chrono::Utc>,
    global_timezone: String,
    // business_address_json: Address,
    // #[serde(rename = "bookingOptionJson")]
    // options: Vec<ReservationOption>,
    booking_time_unit_code: BookingTimeUnitCode,
}

impl Booking {
    fn get_date_time(
        &self,
    ) -> anyhow::Result<(
        chrono::NaiveDate,
        Option<chrono::NaiveTime>,
        Option<chrono::NaiveDate>,
        Option<chrono::NaiveTime>,
    )> {
        Ok(match self.booking_time_unit_code {
            BookingTimeUnitCode::Daily => {
                // make fit to google calendar...
                let timezone = match self.global_timezone.as_str() {
                    "Asia/Seoul" => unsafe {
                        chrono::FixedOffset::east_opt(9 * 3600).unwrap_unchecked()
                    },
                    timezone => return Err(anyhow!("Not mapped timezone found - {timezone}")),
                };
                let start_date_time = self.start_date_time.with_timezone(&timezone).date_naive();
                let end_date_time = self
                    .end_date_time
                    .add(chrono::Duration::hours(24))
                    .with_timezone(&timezone)
                    .date_naive();
                (start_date_time, None, Some(end_date_time), None)
            }
            // other cases has valid date & time info
            _ => {
                let start_date_time = self.start_date_time.naive_utc();
                (
                    start_date_time.date(),
                    Some(start_date_time.time()),
                    None,
                    None,
                )
            }
        })
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReservationOption {
    // name: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Address {
    // road_addr: String,
}

pub(super) async fn fetch(jar: &Jar) -> anyhow::Result<Vec<CalendarEvent>> {
    let client = reqwest::Client::new();
    let graphql_url = url!("https://m.booking.naver.com/graphql");
    let payload = serde_json::json!({
        "operationName": "bookings",
        "variables": {
            "input": {
                "queryType": QueryType(vec![
                    ReservationStatusCode::Cancelled,
                    ReservationStatusCode::Completed
                ]),
                "businessMainCategory": "ALL",
                "startDate": Option::<chrono::NaiveDate>::None,
                "endDate": Option::<chrono::NaiveDate>::None,
                "size": 10,
                "page": 0,
            },
        },
        "query": r#"query bookings($input: BookingParams) {
    booking(input: $input) {
        id
        totalCount
        bookings {
        bookingId
        businessName
        serviceName
        bookingStatusCode
        isCompleted
        startDate
        endDate
        regDateTime
        completedDateTime
        cancelledDateTime
        snapshotJson
        business {
            addressJson
            completedPinValue
            name
            serviceName
            isImp
            isDeleted
            isCompletedButtonImp
            phoneInformationJson
        }
        }
    }
    }
    "#,
    });
    let req = client
        .post(graphql_url.as_ref())
        .header(reqwest::header::COOKIE, jar.cookies(graphql_url).unwrap())
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .json(&payload)
        .build()?;
    let res = client.execute(req).await?;
    let res = res.bytes().await?;
    let res: NaverCalendarResponse = serde_json::from_slice(&res).with_context(|| {
        format!("Failed to parse\n{}", unsafe {
            std::str::from_utf8_unchecked(&res)
        })
    })?;

    res.data
        .booking
        .bookings
        .into_iter()
        .map(|booking| {
            let id = format!("naver/{}", booking.snapshot_json.booking_id);
            let (date_begin, time_begin, date_end, time_end) =
                booking.snapshot_json.get_date_time()?;

            Ok(CalendarEvent {
                id,
                title: booking.snapshot_json.service_name,
                detail: booking.snapshot_json.business_item_name,
                invalid: booking.booking_status_code == ReservationStatusCode::Cancelled,
                date_begin,
                time_begin,
                date_end,
                time_end,
            })
        })
        .collect()
}
