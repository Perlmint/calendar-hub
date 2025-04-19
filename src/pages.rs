use dioxus::prelude::*;

mod user;
pub use user::*;

mod home;
use home::Page as Home;

mod google_calendar;
use google_calendar::Page as GoogleCalendar;

mod cgv;
use cgv::Page as Cgv;

mod bustago;
use bustago::Page as Bustago;

mod naver_reservation;
use naver_reservation::Page as NaverReservation;

mod catch_table;
use catch_table::Page as CatchTable;

pub mod source;
pub mod target;
pub mod vault;

use crate::user::UserContext;

#[derive(Clone, Routable, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Route {
    #[layout(NavBar)]
    #[route("/")]
    Home,
    #[nest("/user")]
    #[route("/login")]
    UserLogin,
    #[route("/lock")]
    UserLock,
    #[end_nest]
    #[layout(LoggedIn)]
    #[route("/google_calendar")]
    GoogleCalendar,
    #[layout(LoggedIn)]
    #[route("/cgv")]
    Cgv,
    #[layout(LoggedIn)]
    #[route("/bustago")]
    Bustago,
    #[layout(LoggedIn)]
    #[route("/naver_reservation")]
    NaverReservation,
    #[layout(LoggedIn)]
    #[route("/catch_table")]
    CatchTable,
}

#[component]
fn NavBar() -> Element {
    let mut user = use_context::<UserContext>();
    let nav = use_navigator();

    let logout_cb = move |_e: Event<MouseData>| async move {
        user::logout().await.unwrap();
        user.restart();
    };

    let has_key = user.as_ref().map(|u| !u.has_key()).unwrap_or_default();
    if has_key {
        nav.push(Route::UserLock);
    }

    rsx! {
        div {
            class: "container",
            nav {
                class: "navbar",
                role: "navigation",
                div {
                    class: "navbar-brand",
                    Link {
                        class: "navbar-item",
                        to: Route::Home,
                        "Calendar Hub"
                    }
                }
                div {
                    class: "navbar-menu",
                    div {
                        class: "navbar-start",
                        div {
                            class: "navbar-item has-dropdown is-hoverable",
                            a {
                                class: "navbar-link",
                                "Outputs"
                            }
                            div {
                                class: "navbar-dropdown",
                                Link {
                                    class: "navbar-item",
                                    to: Route::GoogleCalendar {},
                                    "Google calendar"
                                }
                            }
                        }
                        div {
                            class: "navbar-item has-dropdown is-hoverable",
                            a {
                                class: "navbar-link",
                                "Inputs"
                            }
                            div {
                                class: "navbar-dropdown",
                                Link {
                                    class: "navbar-item",
                                    to: Route::Cgv {},
                                    "CGV"
                                }
                                Link {
                                    class: "navbar-item",
                                    to: Route::Bustago {},
                                    "버스타고"
                                }
                                Link {
                                    class: "navbar-item",
                                    to: Route::NaverReservation {},
                                    "네이버 예약"
                                }
                                Link {
                                    class: "navbar-item",
                                    to: Route::CatchTable {},
                                    "캐치테이블"
                                }
                            }
                        }
                    }
                }
                div {
                    class: "navbar-end",
                    if user.as_ref().map(|u| u.is_signed_in()).unwrap_or_default() {
                        a {
                            class: "navbar-item",
                            onclick: logout_cb,
                            "logout"
                        }
                    }
                }
            },
            Outlet::<Route> {}
        }
    }
}

#[component]
pub fn LoggedIn() -> Element {
    let user = use_context::<UserContext>();
    let nav = use_navigator();

    if !user.as_ref().map(|u| u.is_signed_in()).unwrap_or_default() {
        nav.push(Route::UserLogin);
    }

    rsx! {
        Outlet::<Route> {}
    }
}

#[component]
pub fn PageNotFound(route: Vec<String>) -> Element {
    rsx! {
        h1 { "Page not found" }
    }
}
