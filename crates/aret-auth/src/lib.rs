//! Request authentication for Aret services — ADR-0008 (self-hosted Ory Kratos).
//!
//! Kratos turns a login into a short-lived **session JWT** (`GET /sessions/whoami?tokenize_as=aret`,
//! signed ES256). Services verify it themselves with Kratos's *public* keys, so they never call
//! Kratos per request. Clients send it as `Authorization: Bearer <jwt>`.
//!
//! Give the router a shared [`JwtVerifier`] as state, then take [`AuthenticatedUser`] as a handler
//! argument to require a signed-in user:
//!
//! ```ignore
//! let verifier = Arc::new(JwtVerifier::try_from_env()?);
//! let api = Router::new().route("/v1/tracks", get(list_tracks)).with_state(verifier);
//!
//! async fn list_tracks(user: AuthenticatedUser) -> Json<Vec<Track>> {
//!     // `user.id` is the Kratos identity id (the token's `sub`).
//! }
//! ```

use std::sync::Arc;

use aret_common::error::ApiError;
use axum::extract::{FromRef, FromRequestParts};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use time::{Duration, OffsetDateTime};

/// Environment variable: path of the **public** JSON Web Key Set that verifies session JWTs.
pub const JWKS_PATH_ENV: &str = "ARET_JWKS_PATH";
/// Environment variable: expected token issuer, Kratos's public URL.
pub const JWT_ISSUER_ENV: &str = "ARET_JWT_ISSUER";
/// Audience Kratos adds to tokens minted for Aret services (`dev/kratos/jwt-claims.jsonnet`).
pub const AUDIENCE: &str = "aret-api";

/// Local-development defaults, matching `scripts/dev-env.sh` and `dev/kratos/kratos.yml`.
const DEFAULT_JWKS_PATH: &str = "dev/secrets/jwks.public.json";
const DEFAULT_ISSUER: &str = "http://127.0.0.1:8080/kratos/";

/// Accepted clock difference between Kratos and a service, in seconds.
const CLOCK_LEEWAY_SECS: u64 = 30;

/// The signed-in user making the request.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthenticatedUser {
    /// Kratos identity id (the token's `sub` claim).
    pub id: String,
    /// When the user last entered their credentials (`authenticated_at` claim); `None` for tokens
    /// without it.
    pub authenticated_at: Option<OffsetDateTime>,
}

impl AuthenticatedUser {
    /// Whether the user logged in within `max_age`, as required for sensitive actions such as
    /// deleting the account. Tokens without a login time never count as recent.
    pub fn has_recent_login(&self, max_age: Duration) -> bool {
        self.authenticated_at
            .is_some_and(|at| OffsetDateTime::now_utc() - at <= max_age)
    }
}

/// The verifier can't be built from its configuration.
#[must_use]
#[derive(Debug, thiserror::Error)]
pub enum AuthConfigError {
    /// The key-set file can't be read.
    #[error("cannot read the JWKS file {path} (run scripts/dev-env.sh locally): {source}")]
    ReadJwks {
        /// Path that was read.
        path: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The file is not a JSON Web Key Set.
    #[error("the JWKS is not a valid JSON Web Key Set: {0}")]
    ParseJwks(#[source] serde_json::Error),
    /// The set contains no key that can verify ES256 signatures.
    #[error("the JWKS contains no usable ES256 key")]
    NoUsableKey,
}

/// Verifies Kratos session JWTs: ES256 signature with a public key from the set, expiry and
/// not-before (±30 s), issuer and audience.
pub struct JwtVerifier {
    keys: Vec<VerificationKey>,
    validation: Validation,
}

/// One public key from the set, with its key id (the JWT header's `kid`).
struct VerificationKey {
    id: Option<String>,
    key: DecodingKey,
}

/// The claims Aret reads from a verified token.
#[derive(Deserialize)]
struct Claims {
    sub: String,
    #[serde(default, with = "time::serde::rfc3339::option")]
    authenticated_at: Option<OffsetDateTime>,
}

impl JwtVerifier {
    /// Builds the verifier from `ARET_JWKS_PATH` and `ARET_JWT_ISSUER`, falling back to the
    /// local-development key file and Kratos URL.
    pub fn try_from_env() -> Result<Self, AuthConfigError> {
        let path: String =
            std::env::var(JWKS_PATH_ENV).unwrap_or_else(|_| DEFAULT_JWKS_PATH.to_owned());
        let issuer: String =
            std::env::var(JWT_ISSUER_ENV).unwrap_or_else(|_| DEFAULT_ISSUER.to_owned());
        let jwks: String =
            std::fs::read_to_string(&path).map_err(|source| AuthConfigError::ReadJwks {
                path: path.clone(),
                source,
            })?;
        Self::try_new(&jwks, &issuer)
    }

    /// Builds the verifier from a JSON Web Key Set (public keys) and the expected issuer.
    pub fn try_new(jwks_json: &str, issuer: &str) -> Result<Self, AuthConfigError> {
        let set: JwkSet = serde_json::from_str(jwks_json).map_err(AuthConfigError::ParseJwks)?;
        let keys: Vec<VerificationKey> = set
            .keys
            .iter()
            .filter_map(|jwk| {
                let key = DecodingKey::from_jwk(jwk).ok()?;
                Some(VerificationKey {
                    id: jwk.common.key_id.clone(),
                    key,
                })
            })
            .collect();
        if keys.is_empty() {
            return Err(AuthConfigError::NoUsableKey);
        }

        let mut validation = Validation::new(Algorithm::ES256);
        validation.set_issuer(&[issuer]);
        validation.set_audience(&[AUDIENCE]);
        validation.set_required_spec_claims(&["exp", "nbf", "iss", "aud", "sub"]);
        validation.leeway = CLOCK_LEEWAY_SECS;
        Ok(Self { keys, validation })
    }

    /// Verifies a session JWT and returns its user. Any failure is a 401 with a short reason;
    /// details stay out of the response.
    pub fn verify(&self, token: &str) -> Result<AuthenticatedUser, ApiError> {
        let Ok(header) = jsonwebtoken::decode_header(token) else {
            return Err(ApiError::unauthorized("malformed token"));
        };
        let Some(key) = self.key_for(header.kid.as_deref()) else {
            return Err(ApiError::unauthorized("token signed with an unknown key"));
        };
        match jsonwebtoken::decode::<Claims>(token, key, &self.validation) {
            Ok(data) => Ok(AuthenticatedUser {
                id: data.claims.sub,
                authenticated_at: data.claims.authenticated_at,
            }),
            Err(_) => Err(ApiError::unauthorized("invalid or expired token")),
        }
    }

    /// The key matching the token's `kid`; without a `kid`, the only key if there is just one.
    fn key_for(&self, key_id: Option<&str>) -> Option<&DecodingKey> {
        match key_id {
            Some(id) => self
                .keys
                .iter()
                .find(|k| k.id.as_deref() == Some(id))
                .map(|k| &k.key),
            None if self.keys.len() == 1 => self.keys.first().map(|k| &k.key),
            None => None,
        }
    }
}

/// Requires a valid `Authorization: Bearer <session JWT>`; answers 401 otherwise.
///
/// The router state must provide the verifier (`Arc<JwtVerifier>`): one instance shared by all
/// concurrent requests, which is why it's reference-counted.
impl<State> FromRequestParts<State> for AuthenticatedUser
where
    Arc<JwtVerifier>: FromRef<State>,
    State: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &State) -> Result<Self, Self::Rejection> {
        let Some(token) = bearer_token(parts) else {
            return Err(ApiError::unauthorized("missing bearer token"));
        };
        let verifier: Arc<JwtVerifier> = Arc::from_ref(state);
        verifier.verify(token)
    }
}

/// The token from an `Authorization: Bearer <token>` header, if present and non-empty.
fn bearer_token(parts: &Parts) -> Option<&str> {
    let value: &str = parts.headers.get(AUTHORIZATION)?.to_str().ok()?;
    value
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
}

/// Throwaway signing keys for tests in this and other crates (feature `test-support`).
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use jsonwebtoken::{Algorithm, EncodingKey, Header};
    use serde_json::{Value, json};

    use crate::{AUDIENCE, JwtVerifier};

    /// Issuer of test tokens (the local Kratos public URL).
    pub const TEST_ISSUER: &str = "http://127.0.0.1:8080/kratos/";

    /// A throwaway ES256 key pair, like the one `scripts/dev-env.sh` makes. Tests only.
    pub struct TestKeys {
        key_id: String,
        signing: EncodingKey,
        public_jwks: String,
    }

    impl TestKeys {
        /// Generates a new key pair whose public key has id `key_id`.
        pub fn generate(key_id: &str) -> Self {
            use base64::Engine;
            use base64::engine::general_purpose::URL_SAFE_NO_PAD;
            use p256::elliptic_curve::Generate;
            use p256::pkcs8::{EncodePrivateKey, LineEnding};

            let secret = p256::SecretKey::try_generate().expect("system random number generator");
            let pem = secret
                .to_pkcs8_pem(LineEnding::LF)
                .expect("PKCS#8 encoding");
            let signing = EncodingKey::from_ec_pem(pem.as_bytes()).expect("valid EC key");

            // Uncompressed SEC1 point: 0x04 || x (32 bytes) || y (32 bytes)
            let point = secret.public_key().to_sec1_bytes();
            assert_eq!(point.len(), 65, "expected an uncompressed P-256 point");
            let jwk = json!({
                "kty": "EC",
                "crv": "P-256",
                "alg": "ES256",
                "use": "sig",
                "kid": key_id,
                "x": URL_SAFE_NO_PAD.encode(&point[1..33]),
                "y": URL_SAFE_NO_PAD.encode(&point[33..65]),
            });
            let public_jwks = json!({ "keys": [jwk] }).to_string();
            Self {
                key_id: key_id.to_owned(),
                signing,
                public_jwks,
            }
        }

        /// Signs `claims` the way Kratos's tokenizer does (ES256 with `kid`).
        pub fn sign(&self, claims: &Value) -> String {
            let mut header = Header::new(Algorithm::ES256);
            header.kid = Some(self.key_id.clone());
            jsonwebtoken::encode(&header, claims, &self.signing).expect("signing a test token")
        }

        /// A verifier that trusts this key pair and [`TEST_ISSUER`].
        pub fn verifier(&self) -> JwtVerifier {
            JwtVerifier::try_new(&self.public_jwks, TEST_ISSUER).expect("valid test key set")
        }

        /// A valid session JWT for `subject`, as Kratos would issue it.
        pub fn token_for(&self, subject: &str) -> String {
            self.sign(&kratos_claims(subject))
        }
    }

    /// Claims as Kratos issues them for `subject` who logged in just now, valid for one minute.
    pub fn kratos_claims(subject: &str) -> Value {
        let now = jsonwebtoken::get_current_timestamp();
        let logged_in = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .expect("RFC 3339 formatting");
        json!({
            "authenticated_at": logged_in,
            "sub": subject,
            "iss": TEST_ISSUER,
            "aud": AUDIENCE,
            "iat": now,
            "nbf": now,
            "exp": now + 60,
            "jti": "token-id",
            "sid": "session-id",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use serde_json::json;

    use crate::testing::{TEST_ISSUER, TestKeys, kratos_claims};

    const SUBJECT: &str = "4c3b9a1e-identity-id";

    #[test]
    fn accepts_a_valid_kratos_token() {
        let keys = TestKeys::generate("dev-1");
        let user = keys
            .verifier()
            .verify(&keys.sign(&kratos_claims(SUBJECT)))
            .unwrap();
        assert_eq!(user.id, "4c3b9a1e-identity-id");
        assert!(user.has_recent_login(Duration::minutes(1)));
    }

    #[test]
    fn reads_the_login_time() {
        let keys = TestKeys::generate("dev-1");
        let mut claims = kratos_claims(SUBJECT);
        // Kratos's format: RFC 3339 with nanoseconds
        claims["authenticated_at"] = json!("2026-11-26T15:42:03.123456789Z");
        let user = keys.verifier().verify(&keys.sign(&claims)).unwrap();
        assert_eq!(
            user.authenticated_at,
            Some(time::macros::datetime!(2026-11-26 15:42:03.123456789 UTC))
        );
        assert!(!user.has_recent_login(Duration::minutes(15)));
    }

    #[test]
    fn a_token_without_login_time_is_never_recent() {
        let keys = TestKeys::generate("dev-1");
        let mut claims = kratos_claims(SUBJECT);
        claims.as_object_mut().unwrap().remove("authenticated_at");
        let user = keys.verifier().verify(&keys.sign(&claims)).unwrap();
        assert_eq!(user.authenticated_at, None);
        assert!(!user.has_recent_login(Duration::minutes(15)));
    }

    #[test]
    fn rejects_an_expired_token() {
        let keys = TestKeys::generate("dev-1");
        let mut claims = kratos_claims(SUBJECT);
        claims["exp"] = json!(jsonwebtoken::get_current_timestamp() - 120);
        let err = keys.verifier().verify(&keys.sign(&claims)).unwrap_err();
        assert_eq!(err.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn rejects_another_issuer() {
        let keys = TestKeys::generate("dev-1");
        let mut claims = kratos_claims(SUBJECT);
        claims["iss"] = json!("https://evil.example/");
        assert!(keys.verifier().verify(&keys.sign(&claims)).is_err());
    }

    #[test]
    fn rejects_another_audience() {
        let keys = TestKeys::generate("dev-1");
        let mut claims = kratos_claims(SUBJECT);
        claims["aud"] = json!("some-other-api");
        assert!(keys.verifier().verify(&keys.sign(&claims)).is_err());
    }

    #[test]
    fn rejects_a_token_signed_with_another_key() {
        let trusted = TestKeys::generate("dev-1");
        let attacker = TestKeys::generate("dev-1"); // same kid, different key
        let token = attacker.sign(&kratos_claims(SUBJECT));
        assert!(trusted.verifier().verify(&token).is_err());
    }

    #[test]
    fn rejects_garbage() {
        let keys = TestKeys::generate("dev-1");
        assert!(keys.verifier().verify("not-a-jwt").is_err());
    }

    #[test]
    fn config_rejects_a_key_set_without_usable_keys() {
        let result = JwtVerifier::try_new(r#"{"keys":[]}"#, TEST_ISSUER);
        assert!(matches!(result, Err(AuthConfigError::NoUsableKey)));
        let result = JwtVerifier::try_new("not json", TEST_ISSUER);
        assert!(matches!(result, Err(AuthConfigError::ParseJwks(_))));
    }

    async fn me(user: AuthenticatedUser) -> String {
        user.id
    }

    async fn call_me(keys: &TestKeys, authorization: Option<&str>) -> (StatusCode, String) {
        use tower::ServiceExt;

        let router = Router::new()
            .route("/me", get(me))
            .with_state(Arc::new(keys.verifier()));
        let mut request = Request::get("/me");
        if let Some(value) = authorization {
            request = request.header(AUTHORIZATION, value);
        }
        let response = router
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn handler_gets_the_user_from_a_valid_token() {
        let keys = TestKeys::generate("dev-1");
        let bearer = format!("Bearer {}", keys.sign(&kratos_claims(SUBJECT)));
        let (status, body) = call_me(&keys, Some(&bearer)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "4c3b9a1e-identity-id");
    }

    #[tokio::test]
    async fn handler_rejects_requests_without_a_token() {
        let keys = TestKeys::generate("dev-1");
        let (status, _) = call_me(&keys, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn reads_the_bearer_token() {
        let request = Request::get("/")
            .header(AUTHORIZATION, "Bearer abc")
            .body(())
            .unwrap();
        let (parts, ()) = request.into_parts();
        assert_eq!(bearer_token(&parts), Some("abc"));
    }
}
