use sqlx::SqlitePool;

use crate::{
    prelude::*,
    server::prelude::{reservation::*, user::*},
};

mod graphql;

define_user_data! {
    #[domain = ".naver.com"]
    #[base_url = "https://m.booking.naver.com/"]
    struct NaverUserCookie(
        "NID_SES",
        "NID_AUT"
    )
}

pub(super) async fn crawl(
    config: super::NaverConfig,
    user_id: UserId,
    db: &SqlitePool,
) -> anyhow::Result<usize> {
    let jar = NaverUserCookie::from_iter([config.ses, config.aut].into_iter())?;

    let scrapped_reservations = graphql::fetch(&jar).await?;

    let updated_item_count = if scrapped_reservations.is_empty() {
        0
    } else {
        CalendarEvent::upsert_events_to_db(user_id, db, scrapped_reservations.iter()).await?
    };
    info!("updated item count: {updated_item_count}");

    Ok(updated_item_count as _)
}
