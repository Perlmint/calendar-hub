pub mod catch_table;
pub mod cgv;
pub mod google_calendar;
pub mod kobus;
pub mod megabox;
pub mod naver_reservation;
pub mod bustago;
pub mod reservation;
pub mod user;

pub use reservation::{date_time_to_utc, CalendarEvent, ReservationId};
pub use user::{user_web_router, UserId, UserImpl};

const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/15.5 Safari/605.1.15";

#[macro_export]
macro_rules! selector {
    ($selector:literal) => {{
        static SELECTOR: once_cell::sync::OnceCell<scraper::Selector> =
            once_cell::sync::OnceCell::new();
        SELECTOR.get_or_init(|| scraper::Selector::parse($selector).unwrap())
    }};
}

#[macro_export]
macro_rules! url {
    ($url:literal) => {{
        static URL: once_cell::sync::OnceCell<reqwest::Url> = once_cell::sync::OnceCell::new();
        URL.get_or_init(|| <reqwest::Url as std::str::FromStr>::from_str($url).unwrap())
    }};
}

#[macro_export]
macro_rules! regex {
    ($regex:literal) => {{
        static REGEX: once_cell::sync::OnceCell<regex::Regex> = once_cell::sync::OnceCell::new();
        REGEX.get_or_init(|| regex::Regex::new($regex).unwrap())
    }};
}
