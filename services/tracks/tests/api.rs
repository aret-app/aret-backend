//! Tracks API against a real PostGIS database (see `common`).

mod common;

use std::sync::Arc;

use aret_auth::testing::TestKeys;
use aret_tracks::AppState;
use axum::Router;
use axum::body::Body;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use common::TestDb;

/// The API wired to a test database, with users whose tokens it accepts.
struct TestApi {
    router: Router,
    keys: TestKeys,
}

impl TestApi {
    fn new(db: &TestDb) -> Self {
        let keys = TestKeys::generate("test-key");
        let state = AppState {
            pool: db.pool.clone(),
            verifier: Arc::new(keys.verifier()),
        };
        Self {
            router: aret_tracks::router(state),
            keys,
        }
    }

    /// Sends a request as `user` (a Kratos identity id), or anonymously with `None`.
    async fn send(
        &self,
        user: Option<Uuid>,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(user) = user {
            request = request.header(
                AUTHORIZATION,
                format!("Bearer {}", self.keys.token_for(&user.to_string())),
            );
        }
        let request = match body {
            Some(json) => request
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(json.to_string())),
            None => request.body(Body::empty()),
        }
        .expect("a valid request");
        let response = self
            .router
            .clone()
            .oneshot(request)
            .await
            .expect("router never fails");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("readable body");
        let json: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("JSON body")
        };
        (status, json)
    }
}

/// A short climb near Pania della Croce: 3 points, 0.009° north (~1 km), +20 m, 10 minutes.
fn hike(id: Uuid, start_minute: u32) -> Value {
    let at = |minute: u32| format!("2026-08-01T08:{:02}:00Z", start_minute + minute);
    json!({
        "id": id,
        "title": "Test hike",
        "points": [
            { "lat": 44.000, "lon": 10.30, "ele": 1000.0, "time": at(0) },
            { "lat": 44.0045, "lon": 10.30, "ele": 1010.0, "time": at(5) },
            { "lat": 44.009, "lon": 10.30, "ele": 1020.0, "time": at(10) },
        ],
    })
}

#[tokio::test]
async fn saves_a_track_with_server_side_stats() {
    let Some(db) = TestDb::try_create().await else {
        return;
    };
    let api = TestApi::new(&db);
    let user = Uuid::new_v4();
    let id = Uuid::new_v4();

    let (status, track) = api
        .send(Some(user), "POST", "/v1/tracks", Some(hike(id, 0)))
        .await;

    assert_eq!(status, StatusCode::CREATED, "{track}");
    assert_eq!(track["id"], json!(id));
    assert_eq!(track["point_count"], 3);
    assert_eq!(track["ascent_m"], 20.0);
    assert_eq!(track["started_at"], "2026-08-01T08:00:00Z");
    let distance = track["distance_m"].as_f64().unwrap();
    assert!(
        (995.0..1005.0).contains(&distance),
        "0.009° of latitude ≈ 1 km, got {distance}"
    );
    db.drop_database().await;
}

#[tokio::test]
async fn saving_the_same_id_again_is_idempotent() {
    let Some(db) = TestDb::try_create().await else {
        return;
    };
    let api = TestApi::new(&db);
    let (user, other_user, id) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());

    let (first, created) = api
        .send(Some(user), "POST", "/v1/tracks", Some(hike(id, 0)))
        .await;
    let (retry, again) = api
        .send(Some(user), "POST", "/v1/tracks", Some(hike(id, 0)))
        .await;
    let (stolen, _) = api
        .send(Some(other_user), "POST", "/v1/tracks", Some(hike(id, 0)))
        .await;

    assert_eq!(first, StatusCode::CREATED);
    assert_eq!(
        retry,
        StatusCode::OK,
        "a retried upload returns the stored track"
    );
    assert_eq!(again, created);
    assert_eq!(stolen, StatusCode::CONFLICT);
    let (_, page) = api.send(Some(user), "GET", "/v1/tracks", None).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1, "no duplicate");
    db.drop_database().await;
}

#[tokio::test]
async fn returns_the_points_to_their_owner_only() {
    let Some(db) = TestDb::try_create().await else {
        return;
    };
    let api = TestApi::new(&db);
    let (owner, stranger, id) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    api.send(Some(owner), "POST", "/v1/tracks", Some(hike(id, 0)))
        .await;

    let (status, track) = api
        .send(Some(owner), "GET", &format!("/v1/tracks/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        track["points"],
        hike(id, 0)["points"],
        "points round-trip exactly"
    );

    let (status, _) = api
        .send(Some(stranger), "GET", &format!("/v1/tracks/{id}"), None)
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "other users' tracks look nonexistent"
    );
    db.drop_database().await;
}

#[tokio::test]
async fn lists_newest_first_with_a_cursor() {
    let Some(db) = TestDb::try_create().await else {
        return;
    };
    let api = TestApi::new(&db);
    let user = Uuid::new_v4();
    let ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();
    for (minute, id) in [0, 20, 40].into_iter().zip(&ids) {
        api.send(Some(user), "POST", "/v1/tracks", Some(hike(*id, minute)))
            .await;
    }

    let (_, first) = api
        .send(Some(user), "GET", "/v1/tracks?limit=2", None)
        .await;
    assert_eq!(first["items"][0]["id"], json!(ids[2]), "newest first");
    assert_eq!(first["items"][1]["id"], json!(ids[1]));
    let cursor = first["next_cursor"].as_str().expect("more pages");

    let (_, second) = api
        .send(
            Some(user),
            "GET",
            &format!("/v1/tracks?limit=2&cursor={cursor}"),
            None,
        )
        .await;
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert_eq!(second["items"][0]["id"], json!(ids[0]));
    assert!(second.get("next_cursor").is_none(), "last page");
    db.drop_database().await;
}

#[tokio::test]
async fn rejects_invalid_requests() {
    let Some(db) = TestDb::try_create().await else {
        return;
    };
    let api = TestApi::new(&db);
    let user = Uuid::new_v4();
    let mut one_point = hike(Uuid::new_v4(), 0);
    one_point["points"].as_array_mut().unwrap().truncate(1);

    let (status, problem) = api
        .send(Some(user), "POST", "/v1/tracks", Some(one_point))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        problem["detail"],
        "a track needs between 2 and 100000 points"
    );

    let (status, _) = api
        .send(None, "POST", "/v1/tracks", Some(hike(Uuid::new_v4(), 0)))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "no token");

    let (status, _) = api
        .send(Some(user), "GET", "/v1/tracks?cursor=garbage", None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    db.drop_database().await;
}
