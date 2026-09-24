//! Users, tenants, sessions and role-based permissions.

mod account;
mod password;
mod rbac;
mod session;
mod throttle;

pub use account::{
    Profile, SignupError, SignupInput, TenantSummary, authenticate, default_tenant, load_profile, signup,
};
pub use password::{hash_password, verify_password};
pub use rbac::{Grant, Permission, Role, allows, load_grants};
pub use session::{SESSION_TTL, SessionInfo, create_session, delete_session, resolve_session, switch_tenant};
pub use throttle::{LOGIN_WINDOW_SECS, MAX_LOGIN_FAILURES, clear_login_failures, reserve_login_attempt};
