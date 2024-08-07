use std::{fmt::Display, ops::Add};

use anyhow::{anyhow, Context};
use reqwest::cookie::{CookieStore, Jar};
use serde_with::serde_as;

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
pub(super) struct BookingWrap {
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

#[serde_as]
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Booking {
    booking_id: i64,
    service_name: String,
    #[serde(rename = "bizItemName")]
    business_item_name: String,
    start_date_time: chrono::DateTime<chrono::Utc>,
    end_date_time: chrono::DateTime<chrono::Utc>,
    global_timezone: String,
    business_address_json: Address,
    #[serde(rename = "bizItemAddressJson")]
    #[serde_as(deserialize_as = "serde_with::DefaultOnError")]
    business_item_address_json: Option<Address>,
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

    fn location(&self) -> String {
        let address = self
            .business_item_address_json
            .as_ref()
            .unwrap_or(&self.business_address_json);
        if let Some(place_name) = &address.place_name {
            if let Some(detail) = &address.detail {
                format!(
                    "{} {place_name} {detail}",
                    address.road_addr.as_ref().unwrap_or(&address.address)
                )
            } else {
                format!(
                    "{} {place_name}",
                    address.road_addr.as_ref().unwrap_or(&address.address)
                )
            }
        } else {
            address
                .road_addr
                .as_ref()
                .unwrap_or(&address.address)
                .clone()
        }
    }
}

impl TryFrom<BookingWrap> for CalendarEvent {
    type Error = anyhow::Error;

    fn try_from(booking: BookingWrap) -> Result<Self, Self::Error> {
        let id = format!("naver/{}", booking.snapshot_json.booking_id);
        let (date_begin, time_begin, date_end, time_end) = booking.snapshot_json.get_date_time()?;
        let url = Some(format!(
            "https://m.booking.naver.com/my/bookings/{}",
            booking.snapshot_json.booking_id
        ));
        let location = Some(booking.snapshot_json.location());

        Ok(CalendarEvent {
            id,
            title: booking.snapshot_json.service_name,
            detail: booking.snapshot_json.business_item_name,
            invalid: booking.booking_status_code == ReservationStatusCode::Cancelled,
            date_begin,
            time_begin,
            date_end,
            time_end,
            url,
            location,
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
    road_addr: Option<String>,
    address: String,
    place_name: Option<String>,
    detail: Option<String>,
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
        .map(TryFrom::try_from)
        .collect()
}
