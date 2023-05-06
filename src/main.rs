use std::{collections::BTreeSet, sync::Arc};

use axum::{routing::get, Extension, Router};
use axum_sessions::{async_session::MemoryStore, extractors::ReadableSession, SessionLayer};
use calendar_hub::{google_calendar::GoogleUser, naver_reservation::NaverUser, UserId};
use log::{error, info};
use sqlx::SqlitePool;
use tokio::sync::mpsc;
use tokio_cron_scheduler::{Job, JobScheduler};
use tokio_stream::StreamExt;
use uuid::Uuid;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let url_prefix =
        std::env::var("URL_PREFIX").unwrap_or_else(|_| "http://127.0.0.1:3000".to_string());

    let db_pool = sqlx::SqlitePool::connect("./db.db").await?;
    sqlx::migrate!().run(&db_pool).await?;
    info!("DB migration completed");

    let google_secret =
        Arc::new(google_calendar3::oauth2::read_application_secret("google.json").await?);

    let scheduler = JobScheduler::new().await?;
    scheduler
        .add(Job::new_async("0 0,30 * * * *", {
            let db = db_pool.clone();
            move |_, _| {
                let db = db.clone();
                Box::pin(async move {
                    if let Err(e) = poll(db).await {
                        error!("Failed to poll naver reservation - {}", e);
                    }
                })
            }
        })?)
        .await
        .unwrap();

    tokio::spawn(async move { scheduler.start().await });
    info!("Scheduler started");

    let app = Router::new()
        .route("/", get(|| async { "" }))
        .route("/update", get(poll_user));
    let app = app.nest(
        "/google",
        calendar_hub::google_calendar::web_router(
            calendar_hub::google_calendar::Config::new(
                google_secret.clone(),
                Arc::new(format!("{url_prefix}/google")),
            )
            .await
            .unwrap(),
        ),
    );

    #[cfg(debug_assertions)]
    let app = app.route("/poll_force", get(poll_dev));

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
    let app = app
        .layer(Extension(db_pool))
        .layer(SessionLayer::new(MemoryStore::new(), &session_secret));

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

async fn poll_user(session: ReadableSession, Extension(db): Extension<SqlitePool>) -> String {
    if let Some(user_id) = session.get::<UserId>("user_id") {
        if let Ok(Some(naver_user)) = NaverUser::from_user_id(db.clone(), user_id).await {
            if let Err(e) = naver_user.fetch(db.clone()).await {
                error!("error - {e:?}");
            }
        }

        if let Ok(Some(google_user)) = GoogleUser::from_user_id(&db, user_id).await {
            if let Err(e) = google_user.sync(&db).await {
                error!("error - {e:?}");
            }
        }
    }

    "".to_string()
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
                    Ok(naver_user) => {
                        let user_id = naver_user.user_id();

                        user_id_sender.send(user_id).unwrap();

                        if let Err(e) = naver_user.fetch(db.clone()).await {
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

    drop(user_id_sender);

    let user_ids = Arc::new(tokio::join!(user_id_collector, naver,).0?);

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

    Ok(())
}
