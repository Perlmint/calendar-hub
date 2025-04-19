use std::collections::BTreeMap;

use chrono::Utc;
use dioxus::prelude::*;

use crate::{
    pages::{vault::VaultKey, Route},
    user::UserContext,
    VaultContext,
};

use super::target::TargetType;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SyncStatus {
    Synced,
    Error,
    InProgress,
}

#[derive(PartialEq, Clone, Props)]
struct SyncCardProps {
    title: String,
    last_synced: chrono::DateTime<Utc>,
    status: SyncStatus,
    onclick: EventHandler<()>,
}

#[component]
fn SyncCard(props: SyncCardProps) -> Element {
    rsx! {
        div {
            class: "card",
            div {
                class: "card-content",
                p {
                    class: "title",
                    {props.title}
                }
            }
            div {
                class: "card-content",
                div {
                    class: "content",
                    br {}
                    "last synced: "
                    time {
                        datetime: props.last_synced.format("%+").to_string(),
                        {props.last_synced.format("%Y-%m-%d %H:%M:%S").to_string()}
                    }
                }
            }
            footer {
                class: "card-footer",
                a {
                    class: "card-footer-item",
                    onclick: move |_| {
                        if props.status != SyncStatus::InProgress {
                            props.onclick.call(())
                        }
                    },
                    "Sync"
                }
            }
        }
    }
}

#[component]
pub fn Page() -> Element {
    let user: UserContext = use_context();
    let vault: VaultContext = use_context();
    let target = use_resource({
        let user = user.clone();
        move || async move {
            if user.as_ref().map(|u| u.is_signed_in()).unwrap_or_default() {
                super::target::list_targets().await.unwrap_or_default()
            } else {
                Default::default()
            }
        }
    });
    let sources_status = use_signal_sync(|| {
        enum_iterator::all::<VaultKey>()
            .map(|key| (key, SyncStatus::Synced))
            .collect::<BTreeMap<_, _>>()
    });
    let targets_status = use_signal_sync(|| {
        enum_iterator::all::<TargetType>()
            .map(|key| (key, SyncStatus::Synced))
            .collect::<BTreeMap<_, _>>()
    });
    let vault_handle = vault.clone();
    let vault = vault.as_ref()?;
    let source_list = vault.iter().map(|(v, last_synced)| {
        rsx! {
            SyncCard {
                title: v.to_string(),
                last_synced: last_synced.clone(),
                status: *sources_status.read().get(&v).unwrap(),
                onclick: {
                    let v = v.clone();
                    let mut vault = vault_handle.clone();
                    let mut sources_status = sources_status.clone();
                    move |_| {
                        sources_status.write().insert(v.clone(), SyncStatus::InProgress);
                        let v = v.clone();
                        spawn(async move {
                            if let Ok(_) = match v {
                                VaultKey::Cgv => super::cgv::crawl().await,
                                VaultKey::Bustago => super::bustago::crawl().await,
                                VaultKey::NaverReservation => super::naver_reservation::crawl().await,
                                VaultKey::CatchTable => super::catch_table::crawl().await,
                            } {
                                sources_status.write().insert(v.clone(), SyncStatus::Synced);
                                vault.restart();
                            } else {
                                sources_status.write().insert(v.clone(), SyncStatus::Error);
                            };
                        });
                    }
                },
            }
        }
    });
    let target_handle = target.clone();
    let target = target.as_ref()?;
    let target_list = target.iter().map(|(v, last_synced)| {
        rsx! {
            SyncCard {
                title: v.to_string(),
                last_synced: last_synced.clone(),
                status: *targets_status.read().get(&v).unwrap(),
                onclick: {
                    let v = v.clone();
                    let mut targets = target_handle.clone();
                    let mut targets_status = targets_status.clone();
                    move |_| {
                        targets_status.write().insert(v.clone(), SyncStatus::InProgress);
                        let v = v.clone();
                        spawn(async move {
                            if let Ok(_) = match v {
                                TargetType::GoogleCalendar => super::google_calendar::sync().await,
                            } {
                                targets_status.write().insert(v.clone(), SyncStatus::Synced);
                                targets.restart();
                            } else {
                                targets_status.write().insert(v.clone(), SyncStatus::Error);
                            };
                        });
                    }
                },
            }
        }
    });

    rsx! {
        div {
            class: "section",
            if user.as_ref().map(|u| u.is_signed_in()).unwrap_or_default() {
                div {
                    class: "fixed-grid has-5-cols",
                    div {
                        class: "grid",
                        {source_list}
                        {target_list}
                    }
                }
            } else {
                Link {
                    to: Route::UserLogin,
                    "Login"
                }
            }
        }
    }
}
