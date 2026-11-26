//! HTTP handlers. Every handler takes an [`AuthenticatedUser`] first, so unauthenticated requests
//! are rejected before the body is read.

use aret_auth::AuthenticatedUser;
use aret_common::error::ApiError;
use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::response::IntoResponse;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::model::{NewTrack, PageCursor, TrackDetail, TrackPage, TrackSummary, ValidTrack};
use crate::store::{self, SaveOutcome};
use crate::{gpx_format, stats};

/// Page size when the client doesn't ask for one.
const DEFAULT_PAGE_SIZE: u32 = 20;
/// Largest page size a client may ask for.
const MAX_PAGE_SIZE: u32 = 100;

/// `POST /v1/tracks`: 201 for a new track, 200 if this user already saved this id (retry),
/// 409 if another user's track has it.
pub async fn create_track(
    State(pool): State<PgPool>,
    user: AuthenticatedUser,
    body: Result<Json<NewTrack>, JsonRejection>,
) -> Result<(StatusCode, Json<TrackSummary>), ApiError> {
    let owner = owner_id(&user)?;
    let Json(new_track) =
        body.map_err(|rejection| ApiError::bad_request(&rejection.body_text()))?;
    save(&pool, owner, new_track).await
}

/// Query string of `POST /v1/tracks/gpx`.
#[derive(Debug, Deserialize)]
pub struct GpxUploadQuery {
    /// Client-generated track id (UUID v7), as for `POST /v1/tracks`: makes uploads idempotent.
    pub id: Uuid,
    /// Overrides the track name from the file.
    pub title: Option<String>,
}

/// `POST /v1/tracks/gpx?id=&title=` with a GPX file as the body: same rules and answers as
/// `POST /v1/tracks`.
pub async fn import_gpx(
    State(pool): State<PgPool>,
    user: AuthenticatedUser,
    query: Result<Query<GpxUploadQuery>, QueryRejection>,
    body: Bytes,
) -> Result<(StatusCode, Json<TrackSummary>), ApiError> {
    let owner = owner_id(&user)?;
    let Query(query) = query.map_err(|rejection| ApiError::bad_request(&rejection.body_text()))?;
    let imported = gpx_format::parse_track(&body)?;
    let new_track = NewTrack {
        id: query.id,
        title: query.title.unwrap_or(imported.title),
        points: imported.points,
    };
    save(&pool, owner, new_track).await
}

/// `GET /v1/tracks/{id}/gpx`: one of the user's tracks as a GPX 1.1 download.
pub async fn export_gpx(
    State(pool): State<PgPool>,
    user: AuthenticatedUser,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(track) = get_track(State(pool), user, id).await?;
    let bytes: Vec<u8> = gpx_format::write_track(&track).map_err(|error| {
        tracing::error!(%error, "GPX export failed");
        ApiError::internal()
    })?;
    // The file name uses only the date and id: no user text in a response header
    let date = track.summary.started_at.date();
    let disposition = format!(
        "attachment; filename=\"aret-{date}-{}.gpx\"",
        track.summary.id
    );
    Ok((
        [
            (CONTENT_TYPE, "application/gpx+xml".to_owned()),
            (CONTENT_DISPOSITION, disposition),
        ],
        bytes,
    ))
}

/// Validates and stores a track; 201 new, 200 already saved by this user, 409 id taken.
async fn save(
    pool: &PgPool,
    owner: Uuid,
    new_track: NewTrack,
) -> Result<(StatusCode, Json<TrackSummary>), ApiError> {
    let track = ValidTrack::try_from(new_track)?;
    let gain = stats::elevation_gain(&track.points);

    match store::save_track(pool, owner, &track, gain)
        .await
        .map_err(database_error)?
    {
        SaveOutcome::Created(summary) => Ok((StatusCode::CREATED, Json(summary))),
        SaveOutcome::AlreadySaved(summary) => Ok((StatusCode::OK, Json(summary))),
        SaveOutcome::IdTakenByAnotherUser => Err(ApiError::new(
            StatusCode::CONFLICT,
            "this track id is already in use",
        )),
        SaveOutcome::OwnerDeleted => Err(ApiError::unauthorized("this account was deleted")),
    }
}

/// Query string of `GET /v1/tracks`.
#[derive(Debug, Deserialize)]
pub struct ListQuery {
    /// Page size, 1 to 100 (default 20).
    pub limit: Option<u32>,
    /// `next_cursor` from the previous page.
    pub cursor: Option<String>,
}

/// `GET /v1/tracks?limit=&cursor=`: the user's tracks, newest first.
pub async fn list_tracks(
    State(pool): State<PgPool>,
    user: AuthenticatedUser,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<TrackPage>, ApiError> {
    let owner = owner_id(&user)?;
    let Query(query) = query.map_err(|rejection| ApiError::bad_request(&rejection.body_text()))?;
    let limit: u32 = query
        .limit
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE);
    let after: Option<PageCursor> = query.cursor.as_deref().map(str::parse).transpose()?;

    // One extra row tells whether another page follows
    let mut items: Vec<TrackSummary> =
        store::list_summaries(&pool, owner, after.as_ref(), i64::from(limit) + 1)
            .await
            .map_err(database_error)?;
    let page_size = usize::try_from(limit).expect("page size fits in usize");
    let next_cursor = if items.len() > page_size {
        items.truncate(page_size);
        items.last().map(|last| {
            PageCursor {
                started_at: last.started_at,
                id: last.id,
            }
            .to_string()
        })
    } else {
        None
    };
    Ok(Json(TrackPage { items, next_cursor }))
}

/// `GET /v1/tracks/{id}`: one of the user's tracks with all points; 404 for anything else,
/// including other users' tracks (no hint that they exist).
pub async fn get_track(
    State(pool): State<PgPool>,
    user: AuthenticatedUser,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<TrackDetail>, ApiError> {
    let owner = owner_id(&user)?;
    let Ok(Path(id)) = id else {
        return Err(ApiError::not_found("no such track"));
    };
    let Some(summary) = store::get_summary(&pool, owner, id)
        .await
        .map_err(database_error)?
    else {
        return Err(ApiError::not_found("no such track"));
    };
    let points = store::get_points(&pool, owner, id)
        .await
        .map_err(database_error)?;
    Ok(Json(TrackDetail { summary, points }))
}

/// The owner id for stored tracks: the Kratos identity id from the token.
fn owner_id(user: &AuthenticatedUser) -> Result<Uuid, ApiError> {
    Uuid::parse_str(&user.id).map_err(|_| ApiError::unauthorized("token subject is not a user id"))
}

/// Logs a database failure and hides it from the client.
fn database_error(error: sqlx::Error) -> ApiError {
    tracing::error!(%error, "database error");
    ApiError::internal()
}
