use std::{collections::BTreeSet, ffi::OsStr, path::Path, sync::Arc};

use axum::{
    body::{Bytes, StreamBody},
    http::HeaderValue,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    BoxError, Extension, Json, Router,
};
use axum_sessions::{
    async_session::MemoryStore,
    extractors::{ReadableSession, WritableSession},
    PersistencePolicy, SessionLayer,
};
use calendar_hub::{
    bustago::BustagoUser,
    catch_table::CatchTableUser,
    cgv::CgvUser,
    google_calendar::{self, GoogleUser},
    kobus::KobusUser,
    megabox::MegaboxUser,
    naver_reservation::NaverUser,
    UserId, UserImpl,
};
use futures::{Future, TryStream};
use hyper::{header, Uri};
use log::{debug, error, info};
use sqlx::SqlitePool;
use tokio::sync::mpsc;
use tokio_cron_scheduler::{Job, JobScheduler};
use tokio_stream::StreamExt;
use uuid::Uuid;

async fn serve_static_res<S, F, FUT>(uri: Uri, f: F) -> Response
where
    F: FnOnce(&str) -> FUT,
    FUT: Future<Output = StreamBody<S>>,
    S: TryStream + Send + 'static,
    S::Ok: Into<Bytes>,
    S::Error: Into<BoxError>,
{
    let path = uri.path();
    debug!("static resource requested - {path}");

    let body = f(&path).await;
    let mime = {
        let path: &Path = path.as_ref();
        let extension = path
            .extension()
            .map(OsStr::to_str)
            .flatten()
            .unwrap_or("html");
        mime_guess::from_ext(extension)
    };
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_str(&mime.first_or_octet_stream().to_string()).unwrap(),
        )],
        body,
    )
        .into_response()
}

#[cfg(feature = "embed_web")]
mod static_res {
    use axum::{body::StreamBody, response::Response};
    use futures::Future;
    use hyper::Uri;
    use include_dir::{include_dir, Dir};
    use tokio_util::io::ReaderStream;

    static DATA: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/dist");

    pub async fn serve(uri: Uri) -> Response {
        super::serve_static_res(uri, |path| {
            let path = path.strip_prefix('/').unwrap_or(path);
            let file = DATA.get_file(&path).unwrap_or_else(|| {
                log::debug!("Fallback to index.html");
                DATA.get_file("index.html").unwrap()
            });
            futures::future::ready(StreamBody::new(ReaderStream::new(file.contents())))
        })
        .await
    }

    pub async fn init() {}
}
#[cfg(not(feature = "embed_web"))]
mod static_res {
    use std::path::Path;

    use axum::{body::StreamBody, response::Response};
    use hyper::Uri;
    use log::debug;
    use tokio_util::io::ReaderStream;

    pub async fn serve(uri: Uri) -> Response {
        super::serve_static_res(uri, |path| {
            let path = format!("dist{path}");
            debug!("Try serve {path}");

            async move {
                let file = if Path::new(&path).is_file() {
                    tokio::fs::File::open(&path).await
                } else {
                    debug!("Fallback to index.html");
                    tokio::fs::File::open("dist/index.html").await
                }
                .unwrap();
                StreamBody::new(ReaderStream::new(tokio::io::BufReader::new(file)))
            }
        })
        .await
    }

    pub async fn init() {
        tokio::spawn(async {
            std::process::Command::new("sh")
                .args(["-c", "npx webpack --watch"])
                .output()
                .unwrap();
        });
    }
}

async fn ping_user<U: UserImpl>(user: anyhow::Result<U>) -> anyhow::Result<()> {
    user?.ping().await
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let url_prefix =
        std::env::var("URL_PREFIX").unwrap_or_else(|_| "http://127.0.0.1:3000".to_string());

    let db_pool = sqlx::SqlitePool::connect("./db.db").await?;
    sqlx::migrate!().run(&db_pool).await?;
    info!("DB migration completed");

    let scheduler = JobScheduler::new().await?;
    scheduler
        .add(Job::new_async("0 0,30 * * * *", {
            let db = db_pool.clone();
            move |_, _| {
                let db = db.clone();
                Box::pin(async move {
                    if let Err(e) = poll(db).await {
                        error!("Failed to poll reservation - {}", e);
                    }
                })
            }
        })?)
        .await
        .unwrap();

    if let Some(duration) = NaverUser::PING_INTERVAL {
        scheduler
            .add(Job::new_repeated_async(duration, {
                let db = db_pool.clone();
                move |_, _| {
                    let db = db.clone();
                    Box::pin(async move {
                        let mut users = NaverUser::all(&db);
                        while let Some(user) = users.next().await {
                            if let Err(e) = ping_user(user).await {
                                error!("Failed to ping - {}", e);
                            } else {
                                info!("Success ping for kobus");
                            }
                        }
                    })
                }
            })?)
            .await
            .unwrap();
    }

    if let Some(duration) = CatchTableUser::PING_INTERVAL {
        scheduler
            .add(Job::new_repeated_async(duration, {
                let db = db_pool.clone();
                move |_, _| {
                    let db = db.clone();
                    Box::pin(async move {
                        let mut users = CatchTableUser::all(&db);
                        while let Some(user) = users.next().await {
                            if let Err(e) = ping_user(user).await {
                                error!("Failed to ping - {}", e);
                            } else {
                                info!("Success ping for kobus");
                            }
                        }
                    })
                }
            })?)
            .await
            .unwrap();
    }

    if let Some(duration) = KobusUser::PING_INTERVAL {
        scheduler
            .add(Job::new_repeated_async(duration, {
                let db = db_pool.clone();
                move |_, _| {
                    let db = db.clone();
                    Box::pin(async move {
                        let mut users = KobusUser::all(&db);
                        while let Some(user) = users.next().await {
                            if let Err(e) = ping_user(user).await {
                                error!("Failed to ping - {}", e);
                            } else {
                                info!("Success ping for kobus");
                            }
                        }
                    })
                }
            })?)
            .await
            .unwrap();
    }

    if let Some(duration) = CgvUser::PING_INTERVAL {
        scheduler
            .add(Job::new_repeated_async(duration, {
                let db = db_pool.clone();
                move |_, _| {
                    let db = db.clone();
                    Box::pin(async move {
                        let mut users = CgvUser::all(&db);
                        while let Some(user) = users.next().await {
                            if let Err(e) = ping_user(user).await {
                                error!("Failed to ping - {}", e);
                            } else {
                                info!("Success ping for cgv");
                            }
                        }
                    })
                }
            })?)
            .await
            .unwrap();
    }

    if let Some(duration) = MegaboxUser::PING_INTERVAL {
        scheduler
            .add(Job::new_repeated_async(duration, {
                let db = db_pool.clone();
                move |_, _| {
                    let db = db.clone();
                    Box::pin(async move {
                        let mut users = MegaboxUser::all(&db);
                        while let Some(user) = users.next().await {
                            if let Err(e) = ping_user(user).await {
                                error!("Failed to ping - {}", e);
                            } else {
                                info!("Success ping for megabox");
                            }
                        }
                    })
                }
            })?)
            .await
            .unwrap();
    }

    tokio::spawn(async move { scheduler.start().await });
    info!("Scheduler started");

    static_res::init().await;

    calendar_hub::google_calendar::Config::init(format!("{url_prefix}/google"))
        .await
        .unwrap();

    let router = Router::new()
        .fallback(static_res::serve)
        .route("/sync", post(poll_user))
        .route("/user", get(get_user))
        .route(
            "/login",
            get(|mut session: WritableSession| async move {
                if cfg!(not(feature = "crawl_test")) {
                    Redirect::to("/google/login")
                } else {
                    session.insert("user_id", 0).unwrap();
                    Redirect::to("/")
                }
            }),
        )
        .route("/logout", get(logout));
    let router = router.nest("/google", calendar_hub::google_calendar::web_router());
    let router = router.nest("/naver", calendar_hub::naver_reservation::web_router());
    let router = router.nest("/kobus", calendar_hub::kobus::web_router());
    let router = router.nest("/catch-table", calendar_hub::catch_table::web_router());
    let router = router.nest("/cgv", calendar_hub::cgv::web_router());
    let router = router.nest("/megabox", calendar_hub::megabox::web_router());
    let router = router.nest("/bustago", calendar_hub::bustago::web_router());

    #[cfg(debug_assertions)]
    let router = router.route("/poll_force", get(poll_dev));

    let session_secret = {
        let mut buffer = std::mem::MaybeUninit::<[u8; 64]>::uninit();
        const UUID_SIZE: usize = std::mem::size_of::<Uuid>();
        {
            let mut ptr = buffer.as_mut_ptr() as *mut u8;
            for _ in 0..(64 / UUID_SIZE) {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        Uuid::new_v4().as_bytes().as_ptr(),
                        ptr,
                        UUID_SIZE,
                    );
                    ptr = ptr.add(UUID_SIZE);
                }
            }
        }
        unsafe { buffer.assume_init() }
    };
    let app = router.layer(Extension(db_pool)).layer(
        SessionLayer::new(MemoryStore::new(), &session_secret)
            .with_secure(url_prefix.starts_with("https"))
            .with_persistence_policy(PersistencePolicy::ChangedOnly),
    );

    let (stop_sender, stop_receiver) = tokio::sync::oneshot::channel();

    tokio::task::spawn(async move {
        let sig_int = tokio::signal::ctrl_c();
        #[cfg(target_family = "windows")]
        {
            sig_int.await.expect("Ctrl-C receiver is broken");
        }
        #[cfg(target_family = "unix")]
        {
            let mut sig_term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("Failed to register SIGTERM handler");
            tokio::select! {
                _ = sig_int => (),
                _ = sig_term.recv() => (),
            };
        }

        if stop_sender.send(()).is_err() {
            error!("Already all services are stopped");
        }
    });

    axum::Server::bind(&"0.0.0.0:3000".parse()?)
        .serve(app.into_make_service())
        .with_graceful_shutdown(async move {
            let _ = stop_receiver.await;
        })
        .await
        .unwrap();

    Ok(())
}

#[cfg(debug_assertions)]
async fn poll_dev(Extension(db): Extension<SqlitePool>) {
    if let Err(e) = poll(db).await {
        error!("Failed to poll - {:?}", e);
    }
}

#[derive(serde::Serialize)]
#[serde(tag = "type")]
enum ClientUserData {
    User {
        last_synced: chrono::DateTime<chrono::Utc>,
    },
    None,
}

async fn get_user(
    session: ReadableSession,
    Extension(_db): Extension<SqlitePool>,
) -> Json<ClientUserData> {
    let ret = match session.get::<UserId>("user_id") {
        Some(user_id) => {
            #[cfg(feature = "crawl_test")]
            let last_synced = chrono::Utc::now();
            #[cfg(not(feature = "crawl_test"))]
            let last_synced = google_calendar::get_last_synced(_db, user_id)
                .await
                .unwrap();
            ClientUserData::User { last_synced }
        }
        None => ClientUserData::None,
    };

    Json(ret)
}

async fn logout(mut session: WritableSession) -> Response {
    session.destroy();

    Redirect::to("/").into_response()
}

async fn poll_user(session: ReadableSession, Extension(db): Extension<SqlitePool>) -> Json<bool> {
    if let Some(user_id) = session.get::<UserId>("user_id") {
        #[cfg(not(feature = "crawl_test"))]
        {
            let Ok(last_synced) = google_calendar::get_last_synced(db.clone(), user_id).await
            else {
                return Json(false);
            };

            let duration = chrono::Utc::now() - last_synced;
            if duration < chrono::Duration::minutes(1) {
                info!("Recently updated");
                return Json(false);
            }
        }

        if let Ok(Some(user)) = NaverUser::from_user_id(db.clone(), user_id).await {
            if let Err(e) = user.fetch(db.clone()).await {
                error!("fetch naver - {e:?}");
            }
        }

        if let Ok(Some(user)) = KobusUser::from_user_id(db.clone(), user_id).await {
            if let Err(e) = user.fetch(db.clone()).await {
                error!("fetch kobus - {e:?}");
            }
        }

        if let Ok(Some(user)) = CatchTableUser::from_user_id(db.clone(), user_id).await {
            if let Err(e) = user.fetch(db.clone()).await {
                error!("fetch catch table - {e:?}");
            }
        }

        if let Ok(Some(user)) = CgvUser::from_user_id(db.clone(), user_id).await {
            if let Err(e) = user.fetch(db.clone()).await {
                error!("fetch cgv - {e:?}");
            }
        }

        if let Ok(Some(user)) = MegaboxUser::from_user_id(db.clone(), user_id).await {
            if let Err(e) = user.fetch(db.clone()).await {
                error!("fetch megabox - {e:?}");
            }
        }

        if let Ok(Some(user)) = BustagoUser::from_user_id(db.clone(), user_id).await {
            if let Err(e) = user.fetch(db.clone()).await {
                error!("fetch bustago - {e:?}");
            }
        }

        #[cfg(not(feature = "crawl_test"))]
        if let Ok(Some(google_user)) = GoogleUser::from_user_id(&db, user_id).await {
            if let Err(e) = google_user.sync(&db).await {
                error!("error - {e:?}");
            }
        }
    }

    return Json(true);
}

async fn poll(db: SqlitePool) -> anyhow::Result<()> {
    let (user_id_sender, mut user_id_receiver) = mpsc::unbounded_channel();

    let user_id_collector = tokio::spawn(async move {
        let mut user_ids = BTreeSet::new();

        while let Some(user_id) = user_id_receiver.recv().await {
            user_ids.insert(user_id);
        }

        user_ids
    });

    let naver = tokio::spawn({
        let db = db.clone();
        let user_id_sender = user_id_sender.clone();
        async move {
            let mut users = NaverUser::all(&db);
            while let Some(user) = users.next().await {
                match user {
                    Ok(user) => {
                        let user_id = user.user_id();

                        user_id_sender.send(user_id).unwrap();

                        if let Err(e) = user.fetch(db.clone()).await {
                            error!(
                                "Failed to fetch naver reservation data for {user_id:?} - {e:?}"
                            );
                        }
                    }
                    Err(e) => error!("Failed to get naver user info from DB - {e:?}"),
                }
            }
        }
    });

    let kobus = tokio::spawn({
        let db = db.clone();
        let user_id_sender = user_id_sender.clone();
        async move {
            let mut users = KobusUser::all(&db);
            while let Some(user) = users.next().await {
                match user {
                    Ok(user) => {
                        let user_id = user.user_id();

                        user_id_sender.send(user_id).unwrap();

                        if let Err(e) = user.fetch(db.clone()).await {
                            error!("Failed to fetch kobus data for {user_id:?} - {e:?}");
                        }
                    }
                    Err(e) => error!("Failed to get kobus user info from DB - {e:?}"),
                }
            }
        }
    });

    let catch_table = tokio::spawn({
        let db = db.clone();
        let user_id_sender = user_id_sender.clone();
        async move {
            let mut users = CatchTableUser::all(&db);
            while let Some(user) = users.next().await {
                match user {
                    Ok(user) => {
                        let user_id = user.user_id();

                        user_id_sender.send(user_id).unwrap();

                        if let Err(e) = user.fetch(db.clone()).await {
                            error!("Failed to fetch catch table data for {user_id:?} - {e:?}");
                        }
                    }
                    Err(e) => error!("Failed to get catch table user info from DB - {e:?}"),
                }
            }
        }
    });

    let cgv = tokio::spawn({
        let db = db.clone();
        let user_id_sender = user_id_sender.clone();
        async move {
            let mut users = CgvUser::all(&db);
            while let Some(user) = users.next().await {
                match user {
                    Ok(user) => {
                        let user_id = user.user_id();

                        user_id_sender.send(user_id).unwrap();

                        if let Err(e) = user.fetch(db.clone()).await {
                            error!("Failed to fetch cgv data for {user_id:?} - {e:?}");
                        }
                    }
                    Err(e) => error!("Failed to get cgv user info from DB - {e:?}"),
                }
            }
        }
    });

    let megabox = tokio::spawn({
        let db = db.clone();
        let user_id_sender = user_id_sender.clone();
        async move {
            let mut users = MegaboxUser::all(&db);
            while let Some(user) = users.next().await {
                match user {
                    Ok(user) => {
                        let user_id = user.user_id();

                        user_id_sender.send(user_id).unwrap();

                        if let Err(e) = user.fetch(db.clone()).await {
                            error!("Failed to fetch megabox data for {user_id:?} - {e:?}");
                        }
                    }
                    Err(e) => error!("Failed to get megabox user info from DB - {e:?}"),
                }
            }
        }
    });

    let bustago = tokio::spawn({
        let db = db.clone();
        let user_id_sender = user_id_sender.clone();
        async move {
            let mut users = BustagoUser::all(&db);
            while let Some(user) = users.next().await {
                match user {
                    Ok(user) => {
                        let user_id = user.user_id();

                        user_id_sender.send(user_id).unwrap();

                        if let Err(e) = user.fetch(db.clone()).await {
                            error!("Failed to fetch bustago data for {user_id:?} - {e:?}");
                        }
                    }
                    Err(e) => error!("Failed to get bustago user info from DB - {e:?}"),
                }
            }
        }
    });

    drop(user_id_sender);

    let user_ids = Arc::new(
        tokio::join!(
            user_id_collector,
            naver,
            kobus,
            catch_table,
            cgv,
            megabox,
            bustago
        )
        .0?,
    );

    #[cfg(not(feature = "crawl_test"))]
    {
        let google = tokio::spawn({
            let db = db.clone();
            let user_ids = user_ids.clone();

            async move {
                for &user_id in user_ids.iter() {
                    let user = match GoogleUser::from_user_id(&db, user_id).await {
                        Ok(Some(user)) => user,
                        Ok(None) => continue,
                        Err(e) => {
                            error!("Failed to get google user - {e:?}");
                            continue;
                        }
                    };

                    if let Err(e) = user.sync(&db).await {
                        error!("Failed to sync google calendar - {e:?}");
                    }
                }
            }
        });

        let _ = tokio::join!(google);
    }

    Ok(())
}
