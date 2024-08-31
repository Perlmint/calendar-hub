use dioxus::prelude::*;

use crate::{pages::Route, user::UserContext};

#[component]
pub fn Page() -> Element {
    let user = use_context::<UserContext>();

    rsx! {
        if user.as_ref().map(|u| u.is_signed_in()).unwrap_or_default() {
            span {
                "Home"
            }
        } else {
            Link {
                to: Route::UserLogin,
                "Login"
            }
        }
    }
}
