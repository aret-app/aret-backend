// Body of the after-registration web hook to identity-svc (compose.yaml, aret-backend#12).
// Only the account id: identity-svc never receives personal data (spec §10).
function(ctx) { identity_id: ctx.identity.id }
