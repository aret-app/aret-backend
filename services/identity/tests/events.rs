//! user.created end to end, minus the real Kratos and NATS: web hook → users + outbox → relay,
//! and the catch-up loop against a fake Kratos admin API. Real PostgreSQL (`aret-testing`).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use aret_events::outbox::{self, PublishError, Publisher};
use aret_events::{Envelope, Event};
use aret_identity::AppState;
use aret_identity::kratos::{self, KratosAdmin};
use aret_identity::webhook::{SECRET_HEADER, WebhookSecret};
use aret_testing::TestDb;
use axum::Router;
use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, Request, StatusCode};
use serde_json::json;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const SECRET: &str = "0123456789abcdef0123456789abcdef";

fn app(pool: &PgPool) -> Router {
    let webhook_secret = WebhookSecret::try_new(SECRET).expect("valid test secret");
    let keys = aret_auth::testing::TestKeys::generate("test-key");
    // Nothing here calls Kratos; the address is never contacted
    let kratos = KratosAdmin::try_new("http://127.0.0.1:9").expect("client");
    aret_identity::router(AppState {
        pool: pool.clone(),
        webhook_secret,
        verifier: Arc::new(keys.verifier()),
        kratos,
    })
}

/// Calls the registration web hook like Kratos does, with `secret` (or none).
async fn registration_hook(router: &Router, secret: Option<&str>, body: &str) -> StatusCode {
    let mut request =
        Request::post("/internal/kratos/registration").header(CONTENT_TYPE, "application/json");
    if let Some(secret) = secret {
        request = request.header(SECRET_HEADER, secret);
    }
    let request = request
        .body(Body::from(body.to_owned()))
        .expect("valid request");
    router
        .clone()
        .oneshot(request)
        .await
        .expect("router never fails")
        .status()
}

/// Rows in the outbox: (subject, payload), oldest first.
async fn outbox_rows(pool: &PgPool) -> Vec<(String, serde_json::Value)> {
    sqlx::query_as("SELECT subject, payload FROM outbox ORDER BY created_at, id")
        .fetch_all(pool)
        .await
        .expect("read outbox")
}

async fn unpublished_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM outbox WHERE published_at IS NULL")
        .fetch_one(pool)
        .await
        .expect("count outbox")
}

/// Remembers what it was asked to publish, or fails every call.
#[derive(Default)]
struct FakePublisher {
    published: Mutex<Vec<(String, Uuid, serde_json::Value)>>,
    fail: bool,
}

impl Publisher for FakePublisher {
    async fn publish(
        &self,
        subject: &str,
        message_id: Uuid,
        payload: Vec<u8>,
    ) -> Result<(), PublishError> {
        if self.fail {
            return Err(PublishError("broker down".to_owned()));
        }
        let payload: serde_json::Value = serde_json::from_slice(&payload).expect("JSON payload");
        // Give other tasks a chance to run mid-publish, like a real network call would
        tokio::task::yield_now().await;
        self.published.lock().expect("not poisoned").push((
            subject.to_owned(),
            message_id,
            payload,
        ));
        Ok(())
    }
}

#[tokio::test]
async fn hook_without_the_secret_is_rejected() {
    let Some(db) = TestDb::try_create(&aret_identity::db::MIGRATOR).await else {
        return;
    };
    let router = app(&db.pool);
    let body = json!({ "identity_id": Uuid::new_v4() }).to_string();

    assert_eq!(
        registration_hook(&router, None, &body).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        registration_hook(&router, Some("fedcba9876543210fedcba9876543210"), &body).await,
        StatusCode::UNAUTHORIZED
    );
    // The secret is checked first: a stranger can't even probe the body format
    assert_eq!(
        registration_hook(&router, None, "not json").await,
        StatusCode::UNAUTHORIZED
    );
    assert!(outbox_rows(&db.pool).await.is_empty());
    db.drop_database().await;
}

#[tokio::test]
async fn hook_queues_user_created_once() {
    let Some(db) = TestDb::try_create(&aret_identity::db::MIGRATOR).await else {
        return;
    };
    let router = app(&db.pool);
    let user_id = Uuid::new_v4();
    let body = json!({ "identity_id": user_id }).to_string();

    assert_eq!(
        registration_hook(&router, Some(SECRET), &body).await,
        StatusCode::NO_CONTENT
    );
    // Kratos (or a retry) calling again must not announce the account twice
    assert_eq!(
        registration_hook(&router, Some(SECRET), &body).await,
        StatusCode::NO_CONTENT
    );

    let rows = outbox_rows(&db.pool).await;
    assert_eq!(rows.len(), 1);
    let (subject, payload) = &rows[0];
    assert_eq!(subject, "aret.user.created");
    let envelope: Envelope =
        serde_json::from_value(payload.clone()).expect("outbox holds an Envelope");
    assert_eq!(envelope.event, Event::UserCreated { user_id });
    assert_eq!(envelope.id, envelope.event.id());
    db.drop_database().await;
}

#[tokio::test]
async fn hook_rejects_a_malformed_body() {
    let Some(db) = TestDb::try_create(&aret_identity::db::MIGRATOR).await else {
        return;
    };
    let router = app(&db.pool);

    assert_eq!(
        registration_hook(&router, Some(SECRET), r#"{"identity_id":"nope"}"#).await,
        StatusCode::BAD_REQUEST
    );
    assert!(outbox_rows(&db.pool).await.is_empty());
    db.drop_database().await;
}

#[tokio::test]
async fn relay_publishes_in_order_and_marks_events() {
    let Some(db) = TestDb::try_create(&aret_identity::db::MIGRATOR).await else {
        return;
    };
    let users = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
    for user in users {
        aret_identity::users::record(&db.pool, &[user])
            .await
            .expect("record user");
    }

    // A broker outage: nothing is marked, everything stays queued
    let broken = FakePublisher {
        fail: true,
        ..FakePublisher::default()
    };
    assert!(outbox::relay_once(&db.pool, &broken, 10).await.is_err());
    assert_eq!(unpublished_count(&db.pool).await, 3);

    // Batches of 2: two passes publish all three, oldest first, then there's nothing left
    let publisher = FakePublisher::default();
    assert_eq!(
        outbox::relay_once(&db.pool, &publisher, 2)
            .await
            .expect("relay"),
        2
    );
    assert_eq!(
        outbox::relay_once(&db.pool, &publisher, 2)
            .await
            .expect("relay"),
        1
    );
    assert_eq!(
        outbox::relay_once(&db.pool, &publisher, 2)
            .await
            .expect("relay"),
        0
    );
    assert_eq!(unpublished_count(&db.pool).await, 0);

    let published = publisher.published.into_inner().expect("not poisoned");
    let published_users: Vec<Uuid> = published
        .iter()
        .map(|(subject, message_id, payload)| {
            assert_eq!(subject, "aret.user.created");
            assert_eq!(payload["id"], json!(message_id));
            serde_json::from_value(payload["user_id"].clone()).expect("user id")
        })
        .collect();
    assert_eq!(published_users, users);
    db.drop_database().await;
}

/// A fake Kratos admin API serving `pages` of identities, linked like the real one.
async fn fake_kratos(pages: Vec<Vec<Uuid>>) -> String {
    use axum::extract::Query;
    use axum::routing::get;

    #[derive(serde::Deserialize)]
    struct PageQuery {
        page_token: Option<usize>,
    }

    let handler = move |Query(query): Query<PageQuery>| {
        let pages = pages.clone();
        async move {
            let page = query.page_token.unwrap_or(0);
            let identities: Vec<serde_json::Value> = pages[page]
                .iter()
                .map(|id| json!({ "id": id, "traits": { "email": "someone@example.com" } }))
                .collect();
            let mut headers = HeaderMap::new();
            let mut link =
                "</admin/identities?page_size=500&page_token=0>; rel=\"first\"".to_owned();
            if page + 1 < pages.len() {
                link.push_str(&format!(
                    ",</admin/identities?page_size=500&page_token={}>; rel=\"next\"",
                    page + 1
                ));
            }
            headers.insert("link", link.parse().expect("valid header"));
            (headers, axum::Json(identities))
        }
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a free port");
    let address = listener.local_addr().expect("local address");
    let router = Router::new().route("/admin/identities", get(handler));
    tokio::spawn(async move { axum::serve(listener, router).await });
    format!("http://{address}/")
}

#[tokio::test]
async fn catch_up_finds_accounts_the_hook_missed() {
    let Some(db) = TestDb::try_create(&aret_identity::db::MIGRATOR).await else {
        return;
    };
    let seen_by_hook = Uuid::new_v4();
    let missed = [Uuid::new_v4(), Uuid::new_v4()];
    aret_identity::users::record(&db.pool, &[seen_by_hook])
        .await
        .expect("record user");

    let url = fake_kratos(vec![vec![seen_by_hook, missed[0]], vec![missed[1]]]).await;
    let kratos_admin = KratosAdmin::try_new(&url).expect("client");

    let mut found = kratos::catch_up_once(&db.pool, &kratos_admin)
        .await
        .expect("catch-up");
    found.sort();
    let mut expected = missed.to_vec();
    expected.sort();
    assert_eq!(found, expected);
    assert_eq!(outbox_rows(&db.pool).await.len(), 3);

    // Running again finds nothing new
    assert!(
        kratos::catch_up_once(&db.pool, &kratos_admin)
            .await
            .expect("catch-up")
            .is_empty()
    );
    assert_eq!(outbox_rows(&db.pool).await.len(), 3);
    db.drop_database().await;
}

/// The web hook and catch-up loops (from several identity-svc replicas) all record the same new
/// account at the same moment: it is announced exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_hook_and_catch_up_announce_once() {
    let Some(db) = TestDb::try_create(&aret_identity::db::MIGRATOR).await else {
        return;
    };
    let router = app(&db.pool);
    let user_id = Uuid::new_v4();
    let others: Vec<Uuid> = (0..5).map(|_| Uuid::new_v4()).collect();

    let mut tasks = tokio::task::JoinSet::new();
    for round in 0..20 {
        if round % 2 == 0 {
            // web hook calls (Kratos retrying, or several Kratos replicas)
            let router = router.clone();
            let body = json!({ "identity_id": user_id }).to_string();
            tasks.spawn(async move {
                assert_eq!(
                    registration_hook(&router, Some(SECRET), &body).await,
                    StatusCode::NO_CONTENT
                );
            });
        } else {
            // catch-up pages that contain the same account among others
            let pool = db.pool.clone();
            let mut page = others.clone();
            page.push(user_id);
            tasks.spawn(async move {
                aret_identity::users::record(&pool, &page)
                    .await
                    .expect("record page");
            });
        }
    }
    while let Some(result) = tasks.join_next().await {
        result.expect("task finished");
    }

    let announced: Vec<Uuid> = outbox_rows(&db.pool)
        .await
        .iter()
        .map(|(_, payload)| serde_json::from_value(payload["user_id"].clone()).expect("user id"))
        .collect();
    assert_eq!(announced.len(), 6, "each account exactly once");
    let unique: HashSet<Uuid> = announced.into_iter().collect();
    assert!(unique.contains(&user_id));
    assert_eq!(unique.len(), 6);
    db.drop_database().await;
}

/// Several relays (identity-svc replicas) drain the outbox in parallel: every event is published
/// exactly once and every row ends up marked.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_relays_publish_each_event_once() {
    let Some(db) = TestDb::try_create(&aret_identity::db::MIGRATOR).await else {
        return;
    };
    let users: Vec<Uuid> = (0..60).map(|_| Uuid::new_v4()).collect();
    aret_identity::users::record(&db.pool, &users)
        .await
        .expect("record users");

    let publisher = Arc::new(FakePublisher::default());
    let mut relays = tokio::task::JoinSet::new();
    for _ in 0..4 {
        let pool = db.pool.clone();
        let publisher = Arc::clone(&publisher);
        relays.spawn(async move {
            // Small batches so the relays overlap many times
            while outbox::relay_once(&pool, publisher.as_ref(), 3)
                .await
                .expect("relay")
                > 0
            {}
        });
    }
    while let Some(result) = relays.join_next().await {
        result.expect("relay finished");
    }

    assert_eq!(unpublished_count(&db.pool).await, 0);
    {
        let published = publisher.published.lock().expect("not poisoned");
        assert_eq!(published.len(), users.len(), "no event published twice");
        let message_ids: HashSet<Uuid> = published.iter().map(|(_, id, _)| *id).collect();
        assert_eq!(message_ids.len(), users.len());
    }
    db.drop_database().await;
}
