//! Users, tenants, sessions and role-based permissions.

mod account;
mod invitations;
mod members;
mod password;
mod rbac;
mod session;
mod throttle;

pub use account::{
    Profile, SignupError, SignupInput, TenantSummary, authenticate, default_tenant, load_profile, signup,
};
pub use invitations::{
    Acceptor, GrantSpec, INVITATION_TTL, Invitation, InvitationStatus, Preview, StaffError, accept, invite,
    list_invitations, preview, resend, revoke_invitation, staff_key,
};
pub use members::{MatrixRow, Member, members, permission_matrix, revoke_member, set_grants};
pub use password::{hash_password, verify_password};
pub use rbac::{
    ALL_PERMISSIONS, ALL_ROLES, Grant, JUNIOR_ROLES, Permission, Role, allows, check_may_act_on, check_may_give,
    describe, load_grants,
};
pub use session::{
    SESSION_TTL, SessionInfo, TenantAccess, create_session, delete_session, resolve_session, resolve_tenant_access,
    switch_tenant,
};
pub use throttle::{LOGIN_WINDOW_SECS, MAX_LOGIN_FAILURES, clear_login_failures, reserve_login_attempt};

/// Whether `err` violated the named constraint or unique index.
fn violates(err: &sqlx::Error, constraint: &str) -> bool {
    err.as_database_error().and_then(|db_err| db_err.constraint()).is_some_and(|name| name == constraint)
}
