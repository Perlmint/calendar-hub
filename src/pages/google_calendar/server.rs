use std::collections::HashMap;

use chrono::{NaiveDate, NaiveDateTime, NaiveTime, Utc};
use google_calendar3::{
    api::{Event, EventDateTime},
    hyper, hyper_rustls,
    yup_oauth2::{self as oauth2, ServiceAccountKey},
    CalendarHub,
};
use sqlx::Row;

use crate::{
    prelude::*,
    server::prelude::{common::*, reservation::*, user::*},
};

trait IntoGoogleEventDateTime {
    fn into_google(self) -> EventDateTime;
}

impl IntoGoogleEventDateTime for Option<(NaiveDate, Option<NaiveTime>)> {
    fn into_google(self) -> EventDateTime {
        match self {
            Some(val) => val.into_google(),
            None => EventDateTime {
                date: None,
                date_time: None,
                time_zone: None,
            },
        }
    }
}

impl IntoGoogleEventDateTime for (NaiveDate, Option<NaiveTime>) {
    fn into_google(self) -> EventDateTime {
        if let Some(time) = self.1 {
            EventDateTime {
                date_time: Some(
                    NaiveDateTime::new(self.0, time)
                        .and_local_timezone(Utc)
                        .unwrap(),
                ),
                date: None,
                time_zone: Some("GMT+00:00".to_string()),
            }
        } else {
            EventDateTime {
                date: Some(self.0),
                date_time: None,
                time_zone: Some("GMT+00:00".to_string()),
            }
        }
    }
}

impl From<CalendarEvent> for Event {
    fn from(event: CalendarEvent) -> Event {
        let start = (event.date_begin, event.time_begin).into_google();
        Event {
            description: Some(
                event
                    .url
                    .map(|url| format!("{}\n{}", event.detail, url))
                    .unwrap_or(event.detail),
            ),
            end: Some(
                event
                    .date_end
                    .map(|date| (date, event.time_end).into_google())
                    .unwrap_or_else(|| start.clone()),
            ),
            start: Some(start),
            summary: Some(event.title),
            location: event.location,
            ..Default::default()
        }
    }
}

pub async fn sync(
    user_id: UserId,
    service_account_key: std::sync::Arc<ServiceAccountKey>,
    db: &SqlitePool,
) -> anyhow::Result<()> {
    let ret = sqlx::query!(
        "SELECT `last_synced` as `last_synced: chrono::DateTime<chrono::Utc>`, `calendar_id` FROM `google_user` WHERE `user_id` = ?",
        user_id
    )
    .fetch_one(db)
    .await?;
    let calendar_id = ret.calendar_id;
    let last_synced = ret.last_synced.naive_utc();

    let mut reservations: HashMap<_, _> = sqlx::query_as!(
        CalendarEvent,
        r#"SELECT
            `id`, `title`, `detail`,
            `date_begin` as `date_begin: chrono::NaiveDate`,
            `time_begin` as `time_begin: chrono::NaiveTime`,
            `date_end` as `date_end: chrono::NaiveDate`,
            `time_end` as `time_end: chrono::NaiveTime`,
            `invalid`,
            `location`,
            `url`
        FROM `reservation`
        WHERE `user_id` = ? AND `updated_at` > ?"#,
        user_id,
        last_synced
    )
    .fetch_all(db)
    .await
    .context("Failed to collect reservation data to update")?
    .into_iter()
    .map(|item| (item.id.clone(), item))
    .collect();
    info!("reservation count - {}", reservations.len());

    let auth = oauth2::ServiceAccountAuthenticator::builder(service_account_key.as_ref().clone())
        .build()
        .await?;

    if !reservations.is_empty() {
        let hub = CalendarHub::new(
            google_calendar3::hyper_util::client::legacy::Client::builder(
                google_calendar3::hyper_util::rt::TokioExecutor::new(),
            )
            .build(
                hyper_rustls::HttpsConnectorBuilder::new()
                    .with_native_roots()?
                    .https_or_http()
                    .enable_http1()
                    // .enable_http2()
                    .build(),
            ),
            auth,
        );

        let google_events = sqlx::QueryBuilder::new(
            "SELECT `event_id`, `reservation_id` FROM `google_event` WHERE `user_id` = ",
        )
        .push_bind(user_id)
        .push("AND `reservation_id` in ")
        .push_tuples(reservations.keys(), |mut builder, item| {
            builder.push_bind(item);
        })
        .build()
        .fetch_all(db)
        .await
        .context("Failed to get saved google events")?;

        for google_event in google_events {
            let event_id: String = google_event.get_unchecked(0);
            let reservation_id: String = google_event.get_unchecked(1);
            if let Some(reservation) = reservations.remove(&reservation_id) {
                if reservation.invalid {
                    if let Err(_e) = hub.events().delete(&calendar_id, &event_id).doit().await {
                        // TODO: handle error
                    }
                } else if let Err(_e) = hub
                    .events()
                    .patch(reservation.into(), &calendar_id, &event_id)
                    .doit()
                    .await
                {
                    // TODO: handle error
                }
            }
        }

        if !reservations.is_empty() {
            let mut builder = sqlx::QueryBuilder::new(
                "INSERT INTO `google_event` (`event_id`, `user_id`, `reservation_id`)",
            );
            let mut new_events = Vec::new();
            for (_, reservation) in reservations.into_iter() {
                if reservation.invalid {
                    continue;
                }

                let reservation_id = reservation.id.clone();

                match hub
                    .events()
                    .insert(reservation.into(), &calendar_id)
                    .doit()
                    .await
                {
                    Ok((_, e)) => {
                        new_events.push((e.id.unwrap(), reservation_id));
                    }
                    Err(e) => error!("Failed to insert event - {e:?}"),
                }
            }

            if !new_events.is_empty() {
                builder.push_values(new_events, |mut b, r| {
                    b.push_bind(r.0).push_bind(user_id).push_bind(r.1);
                });
                builder
                    .build()
                    .execute(db)
                    .await
                    .context("Failed to insert newly created events")?;
            }
        }
    }

    let now = chrono::Utc::now().naive_utc();
    sqlx::query!(
        "UPDATE `google_user` SET `last_synced` = ? WHERE `user_id` = ?",
        now,
        user_id
    )
    .execute(db)
    .await
    .unwrap();

    Ok(())
}
