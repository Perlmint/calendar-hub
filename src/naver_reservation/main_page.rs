use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Context};
#[allow(unused_imports)]
use chrono::Timelike; // false warning
use itertools::Itertools;
use reqwest::cookie::{CookieStore, Jar};
use scraper::Html;
use serde::de::Visitor;

use crate::{selector, url, CalendarEvent, USER_AGENT};

use super::graphql::BookingWrap;

#[derive(Debug)]
struct MainPageApolloState {
    upcoming_bookings: HashSet<String>,
    bookings: HashMap<String, BookingWrap>,
}

impl<'de> serde::Deserialize<'de> for MainPageApolloState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'dv> Visitor<'dv> for V {
            type Value = MainPageApolloState;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(formatter, "only map is allowed")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'dv>,
            {
                use serde::de::Error;
                let mut upcoming_bookings = HashSet::new();
                let mut bookings = HashMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    if key == "ROOT_QUERY" {
                        #[derive(serde::Deserialize)]
                        #[serde(rename_all = "camelCase")]
                        struct RootQuery {
                            upcoming_booking: UpcomingBooking,
                        }
                        #[derive(serde::Deserialize)]
                        #[serde(rename_all = "camelCase")]
                        struct UpcomingBooking {
                            bookings: Vec<DataRef>,
                        }
                        #[derive(serde::Deserialize)]
                        struct DataRef {
                            __ref: String,
                        }
                        upcoming_bookings.extend(
                            map.next_value::<RootQuery>()
                                .map_err(|e| {
                                    A::Error::custom(format!("Failed to parse RootQuery - {e}"))
                                })?
                                .upcoming_booking
                                .bookings
                                .into_iter()
                                .map(|data_ref| data_ref.__ref),
                        );
                    } else if key.starts_with("BookingDetails:") {
                        let value = map.next_value::<BookingWrap>().map_err(|e| {
                            A::Error::custom(format!("Failed to parse BookingDetail({key}) - {e}"))
                        })?;
                        bookings.insert(key, value);
                    } else {
                        map.next_value::<serde::de::IgnoredAny>()?;
                    }
                }

                Ok(Self::Value {
                    upcoming_bookings,
                    bookings,
                })
            }
        }
        let visitor = V;
        deserializer.deserialize_map(visitor)
    }
}

pub(super) async fn fetch(jar: &Jar) -> anyhow::Result<Vec<CalendarEvent>> {
    let client = reqwest::Client::new();
    let main_url = url!("https://m.booking.naver.com/my/bookings");
    let req = client
        .post(main_url.as_ref())
        .header(reqwest::header::COOKIE, jar.cookies(main_url).unwrap())
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
        let state: MainPageApolloState =
            json5::from_str(apollo_state_text).context("Failed to parse apollo_context")?;

        let upcoming_bookings = state.upcoming_bookings;

        return state
            .bookings
            .into_iter()
            .filter(|(key, _)| upcoming_bookings.contains(key.as_str()))
            .map(|(_, value)| value.try_into())
            .collect();
    }

    Err(anyhow!("Cannot find apollo state from main page"))
}
