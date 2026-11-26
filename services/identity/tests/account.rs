//! `DELETE /v1/account` (aret-backend#13) against a real PostgreSQL and a fake Kratos admin API.

use std::sync::{Arc, Mutex};

use aret_auth::testing::{TestKeys, kratos_claims};
use aret_events::Service;
use aret_identity::AppState;
use aret_identity::kratos::{self, KratosAdmin};
use aret_identity::users;
use aret_identity::webhook::WebhookSecret;
use aret_testing::TestDb;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::AUTHORIZATION;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

/// What the fake Kratos admin API saw, and whether it answers with errors.
#[derive(Default)]
struct FakeKratosState {
    identities: Mutex<Vec<Uuid>>,
    deleted: Mutex<Vec<Uuid>>,
    broken: Mutex<bool>,
}

/// A fake Kratos admin API: lists `identities`, deletes on `DELETE /admin/identities/{id}`.
async fn fake_kratos(state: Arc<FakeKratosState>) -> String {
    use axum::routing::{delete, get};

    async fn list(State(state): State<Arc<FakeKratosState>>) -> axum::Json<Vec<Value>> {
        let ids = state.identities.lock().expect("not poisoned").clone();
        axum::Json(ids.iter().map(|id| json!({ "id": id })).collect())
    }
    async fn remove(State(state): State<Arc<FakeKratosState>>, Path(id): Path<Uuid>) -> StatusCode {
        if *state.broken.lock().expect("not poisoned") {
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
        state
            .identities
            .lock()
            .expect("not poisoned")
            .retain(|known| *known != id);
        state.deleted.lock().expect("not poisoned").push(id);
        StatusCode::NO_CONTENT
    }

    let router = Router::new()
        .route("/admin/identities", get(list))
        .route("/admin/identities/{id}", delete(remove))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a free port");
    let address = listener.local_addr().expect("local address");
    tokio::spawn(async move { axum::serve(listener, router).await });
    format!("http://{address}")
}

struct TestApi {
    router: Router,
    keys: TestKeys,
    kratos: Arc<FakeKratosState>,
    kratos_admin: KratosAdmin,
}

impl TestApi {
    async fn new(pool: &PgPool) -> Self {
        let keys = TestKeys::generate("test-key");
        let kratos = Arc::new(FakeKratosState::default());
        let kratos_admin =
            KratosAdmin::try_new(&fake_kratos(Arc::clone(&kratos)).await).expect("client");
        let state = AppState {
            pool: pool.clone(),
            webhook_secret: WebhookSecret::try_new("0123456789abcdef0123456789abcdef")
                .expect("secret"),
            verifier: Arc::new(keys.verifier()),
            kratos: kratos_admin.clone(),
        };
        Self {
            router: aret_identity::router(state),
            keys,
            kratos,
            kratos_admin,
        }
    }

    /// `DELETE /v1/account` with `claims` signed as a Kratos session JWT (or no token).
    async fn delete_account(&self, claims: Option<Value>) -> StatusCode {
        let mut request = Request::delete("/v1/account");
        if let Some(claims) = claims {
            request = request.header(AUTHORIZATION, format!("Bearer {}", self.keys.sign(&claims)));
        }
        let request = request.body(Body::empty()).expect("valid request");
        self.router
            .clone()
            .oneshot(request)
            .await
            .expect("router never fails")
            .status()
    }
}

/// (deleted_at set, kratos_deleted_at set) for `user`, or `None` without a row.
async fn deletion_state(pool: &PgPool, user: Uuid) -> Option<(bool, bool)> {
    sqlx::query_as(
        "SELECT deleted_at IS NOT NULL, kratos_deleted_at IS NOT NULL FROM users WHERE id = $1",
    )
    .bind(user)
    .fetch_optional(pool)
    .await
    .expect("read users")
}

/// Events in the outbox on `subject`.
async fn events(pool: &PgPool, subject: &str) -> Vec<Value> {
    sqlx::query_scalar("SELECT payload FROM outbox WHERE subject = $1 ORDER BY created_at")
        .bind(subject)
        .fetch_all(pool)
        .await
        .expect("read outbox")
}

#[tokio::test]
async fn deletes_the_account_after_a_fresh_login() {
    let Some(db) = TestDb::try_create(&aret_identity::db::MIGRATOR).await else {
        return;
    };
    let api = TestApi::new(&db.pool).await;
    let user = Uuid::new_v4();
    users::record(&db.pool, &[user]).await.expect("record user");

    assert_eq!(
        api.delete_account(Some(kratos_claims(&user.to_string())))
            .await,
        StatusCode::ACCEPTED
    );

    assert_eq!(deletion_state(&db.pool, user).await, Some((true, true)));
    assert_eq!(
        *api.kratos.deleted.lock().expect("not poisoned"),
        vec![user]
    );
    let deleted = events(&db.pool, "aret.user.deleted").await;
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0]["type"], "user_deleted");
    assert_eq!(deleted[0]["user_id"], json!(user));

    // Asking again (double tap, retry): accepted, but no second event
    assert_eq!(
        api.delete_account(Some(kratos_claims(&user.to_string())))
            .await,
        StatusCode::ACCEPTED
    );
    assert_eq!(events(&db.pool, "aret.user.deleted").await.len(), 1);
    db.drop_database().await;
}

#[tokio::test]
async fn refuses_without_a_recent_login() {
    let Some(db) = TestDb::try_create(&aret_identity::db::MIGRATOR).await else {
        return;
    };
    let api = TestApi::new(&db.pool).await;
    let user = Uuid::new_v4();
    users::record(&db.pool, &[user]).await.expect("record user");

    assert_eq!(api.delete_account(None).await, StatusCode::UNAUTHORIZED);

    let mut old_login = kratos_claims(&user.to_string());
    old_login["authenticated_at"] = json!("2026-01-01T08:00:00Z");
    assert_eq!(
        api.delete_account(Some(old_login)).await,
        StatusCode::FORBIDDEN
    );

    let mut no_login_time = kratos_claims(&user.to_string());
    no_login_time
        .as_object_mut()
        .expect("object")
        .remove("authenticated_at");
    assert_eq!(
        api.delete_account(Some(no_login_time)).await,
        StatusCode::FORBIDDEN
    );

    assert_eq!(deletion_state(&db.pool, user).await, Some((false, false)));
    assert!(api.kratos.deleted.lock().expect("not poisoned").is_empty());
    assert!(events(&db.pool, "aret.user.deleted").await.is_empty());
    db.drop_database().await;
}

#[tokio::test]
async fn retries_kratos_and_never_announces_the_account_again() {
    let Some(db) = TestDb::try_create(&aret_identity::db::MIGRATOR).await else {
        return;
    };
    let api = TestApi::new(&db.pool).await;
    let user = Uuid::new_v4();
    api.kratos
        .identities
        .lock()
        .expect("not poisoned")
        .push(user);
    users::record(&db.pool, &[user]).await.expect("record user");

    // Kratos is down: the deletion is accepted and recorded, the Kratos part is pending
    *api.kratos.broken.lock().expect("not poisoned") = true;
    assert_eq!(
        api.delete_account(Some(kratos_claims(&user.to_string())))
            .await,
        StatusCode::ACCEPTED
    );
    assert_eq!(deletion_state(&db.pool, user).await, Some((true, false)));
    assert_eq!(events(&db.pool, "aret.user.deleted").await.len(), 1);

    // The catch-up loop still sees the identity in Kratos, but must not announce it as new
    assert!(
        kratos::catch_up_once(&db.pool, &api.kratos_admin)
            .await
            .expect("catch-up")
            .is_empty()
    );
    assert_eq!(events(&db.pool, "aret.user.created").await.len(), 1);

    // Kratos is back: the catch-up loop finishes the deletion
    *api.kratos.broken.lock().expect("not poisoned") = false;
    assert_eq!(
        kratos::finish_deletions(&db.pool, &api.kratos_admin)
            .await
            .expect("retry"),
        1
    );
    assert_eq!(deletion_state(&db.pool, user).await, Some((true, true)));
    assert_eq!(
        *api.kratos.deleted.lock().expect("not poisoned"),
        vec![user]
    );
    assert_eq!(
        kratos::finish_deletions(&db.pool, &api.kratos_admin)
            .await
            .expect("retry"),
        0
    );
    db.drop_database().await;
}

#[tokio::test]
async fn tracks_purge_confirmations() {
    let Some(db) = TestDb::try_create(&aret_identity::db::MIGRATOR).await else {
        return;
    };
    let user = Uuid::new_v4();
    users::mark_deleted(&db.pool, user)
        .await
        .expect("mark deleted");

    // Right after the deletion, every service with user data is still missing
    let overdue = users::overdue_purges(&db.pool, std::time::Duration::ZERO)
        .await
        .expect("query");
    assert_eq!(overdue.len(), 1);
    assert_eq!(overdue[0].user_id, user);
    assert_eq!(overdue[0].missing, vec!["tracks".to_owned()]);
    // ...but nothing is overdue before the warning delay
    let recent = users::overdue_purges(&db.pool, std::time::Duration::from_secs(3600))
        .await
        .expect("query");
    assert!(recent.is_empty());

    // tracks confirms (twice: JetStream delivers at least once)
    for _ in 0..2 {
        let confirmation = aret_events::Envelope::new(aret_events::Event::PurgeCompleted {
            user_id: user,
            service: Service::Tracks,
        });
        users::handle_purge_event(&db.pool, confirmation)
            .await
            .expect("record purge");
    }
    let overdue = users::overdue_purges(&db.pool, std::time::Duration::ZERO)
        .await
        .expect("query");
    assert!(overdue.is_empty());
    db.drop_database().await;
}
