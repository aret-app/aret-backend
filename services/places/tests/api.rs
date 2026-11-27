//! places-svc against a real PostGIS database (`aret-testing`), with the Pania della Croce test
//! catalogue from aret-maps (`tests/fixtures/catalogue`, OpenStreetMap data, ODbL).

use std::path::PathBuf;

use aret_places::catalogue::{self, CatalogueError, Source, Update};
use aret_testing::TestDb;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/catalogue")
}

/// A test database with the fixture catalogue loaded.
async fn loaded_db() -> Option<TestDb> {
    let db = TestDb::try_create(&aret_places::db::MIGRATOR).await?;
    let source = Source::Folder(fixture());
    assert_eq!(
        catalogue::update(&db.pool, &source).await.expect("load"),
        Update::Loaded(32, 81)
    );
    Some(db)
}

async fn get(pool: &PgPool, uri: &str) -> (StatusCode, Vec<u8>) {
    let router: Router = aret_places::router(pool.clone());
    let request = Request::get(uri)
        .body(Body::empty())
        .expect("valid request");
    let response = router.oneshot(request).await.expect("router never fails");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("readable body");
    (status, body.to_vec())
}

async fn get_json(pool: &PgPool, uri: &str) -> (StatusCode, Value) {
    let (status, body) = get(pool, uri).await;
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

fn names(places: &Value) -> Vec<String> {
    places["places"]
        .as_array()
        .expect("places")
        .iter()
        .map(|p| p["name"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[tokio::test]
async fn answers_503_until_a_catalogue_is_loaded() {
    let Some(db) = TestDb::try_create(&aret_places::db::MIGRATOR).await else {
        return;
    };
    let (status, _) = get(&db.pool, "/v1/places?q=pania").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    db.drop_database().await;
}

#[tokio::test]
async fn loads_once_then_skips_an_unchanged_catalogue() {
    let Some(db) = loaded_db().await else { return };
    let source = Source::Folder(fixture());
    assert_eq!(
        catalogue::update(&db.pool, &source).await.expect("check"),
        Update::Unchanged
    );
    let counts: (i64, i64) =
        sqlx::query_as("SELECT (SELECT count(*) FROM routes), (SELECT count(*) FROM places)")
            .fetch_one(&db.pool)
            .await
            .expect("count");
    assert_eq!(counts, (32, 81));
    db.drop_database().await;
}

#[tokio::test]
async fn finds_places_despite_typos_case_and_accents() {
    let Some(db) = loaded_db().await else { return };

    let (status, exact) = get_json(&db.pool, "/v1/places?q=Pania%20della%20Croce").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(names(&exact)[0], "Pania della Croce");
    assert_eq!(exact["places"][0]["kind"], "peak");
    assert_eq!(exact["places"][0]["id"], "node/3140315349");
    assert_eq!(exact["data"]["attribution"], "© OpenStreetMap contributors");
    assert_eq!(exact["data"]["license"], "ODbL-1.0");

    // A typo and capitals
    let (_, typo) = get_json(&db.pool, "/v1/places?q=PANIA%20DELA%20CROCE").await;
    assert_eq!(names(&typo)[0], "Pania della Croce");

    // A word of the name finds all three Panias, peaks first
    let (_, word) = get_json(&db.pool, "/v1/places?q=pania").await;
    let found = names(&word);
    for pania in ["Pania della Croce", "Pania Secca", "Pania Verde"] {
        assert!(
            found.iter().any(|n| n == pania),
            "{pania} missing from {found:?}"
        );
    }
    assert_eq!(word["places"][0]["kind"], "peak");

    // Part of a hut's name, and a village with a typo
    let (_, hut) = get_json(&db.pool, "/v1/places?q=rifugio%20rossi").await;
    assert_eq!(names(&hut)[0], "Rifugio Enrico Rossi");
    let (_, village) = get_json(&db.pool, "/v1/places?q=fornovolaso").await;
    assert_eq!(names(&village)[0], "Fornovolasco");

    // Too short or too long
    let (status, _) = get_json(&db.pool, "/v1/places?q=p").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    db.drop_database().await;
}

#[tokio::test]
async fn routes_that_reach_a_summit_come_first() {
    let Some(db) = loaded_db().await else { return };
    let (status, body) = get_json(&db.pool, "/v1/routes?near=node/3140315349").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["place"]["name"], "Pania della Croce");
    let routes = body["routes"].as_array().expect("routes");
    assert!(!routes.is_empty());
    assert_eq!(routes[0]["relation"], "reaches");

    // Every "reaches" before every "nearby", and the normal route to the summit among them
    let relations: Vec<&str> = routes
        .iter()
        .map(|r| r["relation"].as_str().unwrap_or_default())
        .collect();
    let first_nearby = relations
        .iter()
        .position(|r| *r == "nearby")
        .unwrap_or(relations.len());
    assert!(
        relations[first_nearby..].iter().all(|r| *r == "nearby"),
        "{relations:?}"
    );
    assert!(
        routes[..first_nearby]
            .iter()
            .any(|r| r["name"] == "Via Normale Pania della Croce"),
        "the normal route reaches the summit"
    );
    // Each summary has what the app compares
    for key in ["length_m", "difficulty", "complete", "start", "distance_m"] {
        assert!(!routes[0][key].is_null(), "{key} missing");
    }

    // A position works too; bad input is refused
    let (status, by_position) = get_json(&db.pool, "/v1/routes?lat=44.0345&lon=10.3241").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(by_position["routes"][0]["relation"], "reaches");
    let (status, _) = get_json(&db.pool, "/v1/routes").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = get_json(&db.pool, "/v1/routes?near=node/1").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    db.drop_database().await;
}

#[tokio::test]
async fn route_detail_and_gpx() {
    let Some(db) = loaded_db().await else { return };
    let (_, near) = get_json(&db.pool, "/v1/routes?near=node/3140315349").await;
    let osm_id = near["routes"][0]["osm_id"].as_i64().expect("an id");

    let (status, detail) = get_json(&db.pool, &format!("/v1/routes/{osm_id}")).await;
    assert_eq!(status, StatusCode::OK);
    let route = &detail["route"];
    assert_eq!(route["geometry"]["type"], "MultiLineString");
    assert!(
        route["profiles"][0]
            .as_array()
            .is_some_and(|p| p.len() >= 2),
        "elevation profile"
    );
    assert_eq!(
        route["osm_url"],
        format!("https://www.openstreetmap.org/relation/{osm_id}")
    );

    let (status, gpx) = get(&db.pool, &format!("/v1/routes/{osm_id}/gpx")).await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(gpx).expect("UTF-8");
    assert!(text.contains("<trk>") && text.contains("<trkseg>") && text.contains("<trkpt"));
    assert!(
        text.contains("OpenStreetMap contributors"),
        "credit in the GPX"
    );

    let (status, _) = get(&db.pool, "/v1/routes/1").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    db.drop_database().await;
}

#[tokio::test]
async fn a_bad_download_keeps_the_loaded_catalogue() {
    let Some(db) = loaded_db().await else { return };
    // Same files, but version.json announces different content: the checksum check must fail
    let broken = std::env::temp_dir().join(format!("places-broken-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&broken);
    std::fs::create_dir_all(&broken).expect("temp folder");
    for file in ["routes.geojson.gz", "places.geojson.gz"] {
        std::fs::copy(fixture().join(file), broken.join(file)).expect("copy");
    }
    let mut version: Value =
        serde_json::from_slice(&std::fs::read(fixture().join("version.json")).expect("read"))
            .expect("json");
    version["files"]["routes"]["sha256"] = Value::from("0".repeat(64));
    std::fs::write(broken.join("version.json"), version.to_string()).expect("write");

    let result = catalogue::update(&db.pool, &Source::Folder(broken)).await;
    assert!(
        matches!(result, Err(CatalogueError::Checksum(_))),
        "{result:?}"
    );
    let (status, body) = get_json(&db.pool, "/v1/places?q=pania%20della%20croce").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        names(&body)[0],
        "Pania della Croce",
        "old catalogue still served"
    );
    db.drop_database().await;
}
