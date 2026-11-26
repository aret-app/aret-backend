//! Kratos admin API client, and the catch-up loop that guarantees every account is announced.
//!
//! The registration web hook is fire-and-forget, so a call can be lost (identity-svc restarting,
//! a network error). Every few minutes the catch-up loop lists all Kratos identities and records
//! any it hasn't seen ([`crate::users::record`]), which queues their `user.created`.

use std::time::Duration;

use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::users;

/// Environment variable with the Kratos admin API address, e.g. `http://kratos:4434`. Private
/// network only: the admin API has no authentication.
pub const KRATOS_ADMIN_URL_ENV: &str = "KRATOS_ADMIN_URL";

/// Identities per page when listing (Kratos allows up to 1000).
const PAGE_SIZE: u32 = 500;
/// Time limit for one admin API request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A Kratos admin API call failed.
#[must_use]
#[derive(Debug, thiserror::Error)]
pub enum KratosError {
    /// Network error, timeout, or a response that isn't the expected JSON. Holds the whole cause
    /// chain: reqwest's own message ("error sending request") hides the reason.
    #[error("Kratos admin API request failed: {0}")]
    Request(String),
}

impl From<reqwest::Error> for KratosError {
    fn from(error: reqwest::Error) -> Self {
        use std::error::Error;

        let mut message = error.to_string();
        let mut cause = error.source();
        while let Some(inner) = cause {
            message.push_str(": ");
            message.push_str(&inner.to_string());
            cause = inner.source();
        }
        Self::Request(message)
    }
}

/// The catch-up pass failed; it is retried on the next interval.
#[must_use]
#[derive(Debug, thiserror::Error)]
pub enum CatchUpError {
    /// Listing the identities failed.
    #[error(transparent)]
    Kratos(#[from] KratosError),
    /// Recording them failed.
    #[error("recording identities failed: {0}")]
    Database(#[from] sqlx::Error),
}

/// Client for the Kratos admin API.
pub struct KratosAdmin {
    client: reqwest::Client,
    base_url: String,
}

/// The only identity field identity-svc reads; traits (personal data) are skipped.
#[derive(Debug, Deserialize)]
struct IdentityId {
    id: Uuid,
}

impl KratosAdmin {
    /// Client for the admin API at `base_url`, e.g. `http://kratos:4434`.
    pub fn try_new(base_url: &str) -> Result<Self, KratosError> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_owned(),
        })
    }

    /// One page of identity ids, and the token of the next page if there is one.
    async fn identity_page(
        &self,
        page_token: Option<&str>,
    ) -> Result<(Vec<Uuid>, Option<String>), KratosError> {
        let mut url = format!("{}/admin/identities?page_size={PAGE_SIZE}", self.base_url);
        if let Some(token) = page_token {
            // Copied as-is from Kratos's own Link header, so already URL-encoded
            url.push_str("&page_token=");
            url.push_str(token);
        }
        let response = self.client.get(&url).send().await?.error_for_status()?;
        let next_token = response
            .headers()
            .get(reqwest::header::LINK)
            .and_then(|value| value.to_str().ok())
            .and_then(next_page_token);
        let identities: Vec<IdentityId> = response.json().await?;
        Ok((
            identities.into_iter().map(|identity| identity.id).collect(),
            next_token,
        ))
    }
}

/// The `page_token` of the `rel="next"` entry in a Link header, e.g.
/// `</admin/identities?page_size=500&page_token=abc>; rel="next"`.
fn next_page_token(link_header: &str) -> Option<String> {
    let next = link_header
        .split(',')
        .find(|entry| entry.contains("rel=\"next\""))?;
    let target = next.split_once('<')?.1.split_once('>')?.0;
    let query = target.split_once('?')?.1;
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix("page_token="))
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
}

/// Lists every Kratos identity and records the ones not seen before. Returns the new ids.
pub async fn catch_up_once(pool: &PgPool, kratos: &KratosAdmin) -> Result<Vec<Uuid>, CatchUpError> {
    let mut new_ids = Vec::new();
    let mut page_token: Option<String> = None;
    loop {
        let (ids, next) = kratos.identity_page(page_token.as_deref()).await?;
        new_ids.extend(users::record(pool, &ids).await?);
        match next {
            // Stop on an empty page too, in case a server keeps returning a next link
            Some(token) if !ids.is_empty() => page_token = Some(token),
            _ => return Ok(new_ids),
        }
    }
}

/// Runs [`catch_up_once`] now and then every `interval`, forever.
pub async fn run_catch_up(pool: PgPool, kratos: KratosAdmin, interval: Duration) {
    loop {
        match catch_up_once(&pool, &kratos).await {
            Ok(new_ids) if !new_ids.is_empty() => {
                tracing::info!(
                    count = new_ids.len(),
                    "catch-up recorded accounts the web hook missed"
                );
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "catch-up failed; retrying later"),
        }
        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_next_page_token() {
        let header = "</admin/identities?page_size=500&page_token=eyJ2IjoxfQ%3D%3D>; rel=\"next\"";
        assert_eq!(next_page_token(header).as_deref(), Some("eyJ2IjoxfQ%3D%3D"));
    }

    #[test]
    fn ignores_other_relations() {
        let header = "</admin/identities?page_size=500&page_token=a>; rel=\"first\", \
                      </admin/identities?page_token=b&page_size=500>; rel=\"next\"";
        assert_eq!(next_page_token(header).as_deref(), Some("b"));
        let last_page = "</admin/identities?page_size=500&page_token=a>; rel=\"first\"";
        assert_eq!(next_page_token(last_page), None);
    }
}
