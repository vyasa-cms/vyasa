//! User domain: authentication, password rules, RBAC, and management.

pub mod auth;
pub mod password;
pub mod rbac;
pub mod registration;
pub mod roles;
pub mod service;

pub use auth::{ensure_confirmed, AuthService, AuthSession, INVALID_CREDENTIALS, SESSION_TTL};
pub use rbac::{can, cap_name, effective_caps, ensure, parse_cap, role_caps, Capability};
pub use registration::{
    fold_email, normalized_email, validate_registration, NewRegistration, RegisterOutcome,
    RegistrationService, RegistrationStatus, ResendOutcome, VerifyOutcome,
    FORBIDDEN_DEFAULT_ROLE_CAPS, UNCONFIRMED_ACCOUNT_LIFETIME, VERIFY_TOKEN_TTL,
};
pub use roles::{RoleInput, RolePatch, RolesService};
pub use service::{CreateUser, UpdateProfile, UsersService};
