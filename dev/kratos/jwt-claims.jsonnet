// Extra claims for the session JWT Aret services verify (crates/aret-auth, aret-backend#3).
// Kratos merges this `claims` object into its default claims (sub, iss, exp, nbf, iat, jti, sid).
// The audience makes services accept only tokens minted for them. `authenticated_at` (RFC 3339)
// is when the user last entered their password: sensitive actions such as deleting the account
// require a recent one (aret-backend#13).
local session = std.extVar('session');

{
  claims: {
    aud: 'aret-api',
    authenticated_at: session.authenticated_at,
  },
}
