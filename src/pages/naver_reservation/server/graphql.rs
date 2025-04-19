use std::{fmt::Display, ops::Add};

use anyhow::{anyhow, Context};
use reqwest::{
    cookie::{CookieStore, Jar},
    Url,
};
use serde_with::serde_as;

use crate::server::prelude::reservation::*;

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

#[derive(Debug, Clone, serde::Deserialize)]
struct UpcomingBookingResponse {
    data: Data,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct Data {
    me: Me,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Me {
    upcoming_bookings: UpcomingBooking,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpcomingBooking {
    bookings: Vec<UpcomingBookingItem>,
    // page_info: u32,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpcomingBookingItem {
    id: String,
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
    let client = reqwest::Client::builder().user_agent(USER_AGENT).build()?;
    let ids: Vec<_> = {
        let graphql_url = url!("https://bff-gateway.place.naver.com/graphql");
        let payload = serde_json::json!({
            "operationName": "UpcomingBookingQuery",
            "variables": {
                "limit": 10,
            },
            "query": r#"query UpcomingBookingQuery($page: Int, $limit: Int) {
  me {
    ... on MeSucceed {
      ...UpcomingSection_UpcomingBookings
      __typename
    }
    __typename
  }
}

fragment UpcomingSection_UpcomingBooking on Booking {
  id
  bizItemThumbImage
  formattedBookingDateText
  bizItemName
  businessName
  businessId
  label
  price
  bookingStatusCode
  landingUrl
  displayOrderTimestamp
  remainDays
  directionLandingUrl(appName: "m.place.naver.com/my") {
    car {
      appScheme
      webUrl
      __typename
    }
    __typename
  }
  placeSummary {
    id
    name
    directionLandingUrl(appName: "m.place.naver.com/my") {
      car {
        appScheme
        webUrl
        __typename
      }
      __typename
    }
    coordinate {
      longitude
      latitude
      __typename
    }
    __typename
  }
  __typename
}

fragment UpcomingSection_UpcomingBookings on MeSucceed {
  upcomingBookings(page: $page, limit: $limit) {
    bookings {
      ... on Booking {
        ...UpcomingSection_UpcomingBooking
        __typename
      }
      __typename
    }
    pageInfo {
      page
      nextPage
      totalCount
      hasNextPage
      __typename
    }
    __typename
  }
  __typename
}"#,
        });
        let req = client
            .post(graphql_url.as_ref())
            .header(reqwest::header::COOKIE, jar.cookies(graphql_url).unwrap())
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .json(&payload)
            .build()?;
        let res = client.execute(req).await?;
        let res = res.bytes().await?;
        let res: UpcomingBookingResponse = serde_json::from_slice(&res).with_context(|| {
            format!("Failed to parse\n{}", unsafe {
                std::str::from_utf8_unchecked(&res)
            })
        })?;

        res.data
            .me
            .upcoming_bookings
            .bookings
            .into_iter()
            .map(|item| item.id)
            .collect()
    };

    let mut result = Vec::with_capacity(ids.len());
    for id in ids {
        let url = reqwest::Url::parse(&format!(
            "https://booking.naver.com/my/bookings/{id}?from=myp"
        ))?;
        let detail = fetch_detail(jar, url).await?;
        result.push(detail);
    }

    Ok(result)
}

#[derive(Debug)]
struct MainPageApolloState {
    bookings: Vec<BookingWrap>,
}

impl<'de> serde::Deserialize<'de> for MainPageApolloState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'dv> serde::de::Visitor<'dv> for V {
            type Value = MainPageApolloState;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(formatter, "only map is allowed")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'dv>,
            {
                use serde::de::Error;
                let mut bookings = Vec::with_capacity(1);
                while let Some(key) = map.next_key::<String>()? {
                    if key.starts_with("BookingDetails:") {
                        let value = map.next_value::<BookingWrap>().map_err(|e| {
                            A::Error::custom(format!("Failed to parse BookingDetail({key}) - {e}"))
                        })?;
                        bookings.push(value);
                    } else {
                        map.next_value::<serde::de::IgnoredAny>()?;
                    }
                }

                Ok(Self::Value { bookings })
            }
        }
        let visitor = V;
        deserializer.deserialize_map(visitor)
    }
}

async fn fetch_detail(jar: &Jar, url: Url) -> anyhow::Result<CalendarEvent> {
    use itertools::Itertools;
    use scraper::Html;

    let client = reqwest::Client::builder().user_agent(USER_AGENT).build()?;
    let req = client
        .get(url.as_ref())
        .header(reqwest::header::COOKIE, jar.cookies(&url).unwrap())
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .build()?;
    let res = client.execute(req).await?;
    let res = res.bytes().await?;

    let html = std::str::from_utf8(&res)?;
    let fragment = Html::parse_fragment(html);

    for script in fragment.select(selector!("script:not([src]):not([id])")) {
        let text = script.text().join("");
        let text = text.trim();
        let Some(apollo_state_text) = text.strip_prefix("window.__APOLLO_STATE__=") else {
            continue;
        };
        let mut state: MainPageApolloState =
            json5::from_str(apollo_state_text).context("Failed to parse apollo_context")?;

        return state
            .bookings
            .drain(..)
            .next()
            .context("No booking found")?
            .try_into();
    }

    Err(anyhow!("Failed to find apollo state"))
}
