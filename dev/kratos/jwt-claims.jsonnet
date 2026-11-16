// Extra claims for the session JWT Aret services verify (crates/aret-auth, aret-backend#3).
// Kratos merges this `claims` object into its default claims (sub, iss, exp, nbf, iat, jti, sid).
// The audience makes services accept only tokens minted for them.
{
  claims: {
    aud: 'aret-api',
  },
}
