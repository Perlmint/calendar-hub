use dioxus::prelude::*;

mod user;
pub use user::*;

mod home;
use home::Page as Home;

mod google;
use google::Page as Google;

use crate::user::UserContext;

#[derive(Clone, Routable, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Route {
    #[layout(NavBar)]
    #[route("/")]
    Home {},
    #[route("/user/login")]
    UserLogin,
    #[layout(LoggedIn)]
    #[route("/google_calendar")]
    Google,
}

#[component]
fn NavBar() -> Element {
    let user = use_context::<UserContext>();

    rsx! {
        nav { id: "navbar",
            ul {
                li {
                    Link {
                        to: Route::Home {},
                        "Calendar Hub"
                    }
                }
            }
            ul {
                li {
                    Link {
                        to: Route::Google {},
                        "Google calendar"
                    }
                }
            }
            ul {
                li {
                    if user.as_ref().map(|u| u.is_signed_in()).unwrap_or_default() {
                        Link {
                            to: "/user/logout",
                            "logout"
                        }
                    }
                }
            }
        }
        div {
            Outlet::<Route> {}
        }
    }
}

#[component]
pub fn LoggedIn() -> Element {
    let user = use_context::<UserContext>();

    if !user.as_ref().map(|u| u.is_signed_in()).unwrap_or_default() {
        rsx! {
            span {
                "Need to login"
            }
        }
    } else {
        rsx! {
            Outlet::<Route> {}
        }
    }
}

#[component]
pub fn PageNotFound(route: Vec<String>) -> Element {
    rsx! {
        h1 { "Page not found" }
    }
}
