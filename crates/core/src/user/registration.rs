//! Public registration: a visitor makes their own account, which can do
//! nothing until the address it was made with is confirmed.
//!
//! This service decides and stores; it sends nothing. Each outcome names
//! the mail the caller owes the address, and the caller answers the client
//! the same way whichever outcome it got, so a stranger cannot learn which
//! addresses have accounts.
//!
//! ## One address, two registrations before it is confirmed
//!
//! Anyone can type anyone's address into the form, so until the address is
//! confirmed nobody knows which registrant (if either) owns the mailbox.
//! The rule here is that **a password the mailbox's owner may not have
//! chosen never becomes a working credential**:
//!
//! - A repeat with the *same* password is the same person pressing the
//!   button again: nothing changes and the confirmation is sent again.
//! - A repeat with a *different* password means two parties (or one who
//!   forgot). Keeping the first password would let someone who registered
//!   a victim's address in advance walk into the account the victim later
//!   confirms; taking the second would let someone who registers just
//!   after the victim do the same. So neither is kept: the unconfirmed
//!   account's password is removed, and once the address is confirmed its
//!   owner sets one through the mailbox (the ordinary set-password link).
//!   [`VerifyOutcome::Confirmed`]'s `needs_password` tells the caller when
//!   that is so.
//!
//! ## One registration, and the link opened
//!
//! A single registration is enough to put a stranger's password on an
//! address, and opening the link proves only that someone (or something:
//! mail scanners follow links) reached the mailbox. So confirming keeps the
//! stored password only when the person opening the link types it again
//! ([`RegistrationService::verify`] with the password): then it is theirs.
//! Opened without it, the address is confirmed and the password removed,
//! and the caller mails a set-password link. A wrong password changes
//! nothing and does not spend the link. A password reset through the
//! mailbox confirms the address too (`UsersRepo::reset_password_with_token`),
//! and an administrator's confirmation removes the password as an
//! unattended link does ([`RegistrationService::confirm`]).
//!
//! The account's username and display name stay as first registered; a
//! confirmed account is never touched at all.
//!
//! ## No chosen usernames
//!
//! A registration does not pick its username: one is generated from the
//! display name plus a random suffix. A username is public (author pages)
//! and unique, so one the visitor chose would be reserved by a new address
//! and not by an address that already has an account -- and trying it
//! again, or looking it up, would tell which addresses have accounts.

use chrono::Duration;
use rand::RngCore as _;
use sha2::{Digest as _, Sha256};
use vyasa_common::AppError;
use vyasa_db::models::{Role, TokenPurpose, UserRow};
use vyasa_db::repo::{
    ConfirmPassword, NewUser, RolesRepo, TokensRepo, UnconfirmedInsertError, UsersRepo,
};

use super::password::{self, DUMMY_HASH, MIN_PASSWORD_LENGTH};
use super::rbac::{cap_name, role_caps, Capability};
use super::service::derived_username;
use crate::options::OptionsService;

/// How long a confirmation link works.
pub const VERIFY_TOKEN_TTL: Duration = Duration::hours(24);

/// How long an account may stay unconfirmed before it is deleted.
pub const UNCONFIRMED_ACCOUNT_LIFETIME: Duration = Duration::days(7);

/// Capabilities the role given to self-registered accounts may not hold.
/// A stranger gets at most an author's power over their own work: not the
/// running of the site (users, options, plugins, themes), and not power
/// over other people's work or words (editing others' content, moderating
/// comments, managing the categories everyone's posts are filed under) --
/// so of the built-in roles, administrator and editor are never offered.
pub const FORBIDDEN_DEFAULT_ROLE_CAPS: [Capability; 7] = [
    Capability::ManageUsers,
    Capability::ManageOptions,
    Capability::ManagePlugins,
    Capability::ManageThemes,
    Capability::EditOthers,
    Capability::ModerateComments,
    Capability::ManageCategories,
];

/// The longest address accepted (the limit SMTP itself puts on a path).
const MAX_EMAIL_LENGTH: usize = 254;

/// The longest display name accepted from a stranger.
const MAX_DISPLAY_NAME_CHARS: usize = 100;

/// The longest part of a generated username taken from the display name.
const USERNAME_BASE_MAX: usize = 40;

/// Characters of the random username suffix (lowercase base32).
const USERNAME_SUFFIX_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// Length of the random username suffix: 30 bits.
const USERNAME_SUFFIX_LEN: usize = 6;

/// How many generated usernames are tried before giving up.
const USERNAME_ATTEMPTS: usize = 8;

/// A username for a new registration: the display name slugified (at most
/// [`USERNAME_BASE_MAX`] bytes; `member` when nothing is left), a dash,
/// and [`USERNAME_SUFFIX_LEN`] random lowercase base32 characters. Already
/// a slug, so it passes the username rule unchanged.
#[must_use]
pub fn generated_username(display_name: &str) -> String {
    let mut base = vyasa_common::slugify(display_name);
    if base.len() > USERNAME_BASE_MAX {
        let mut end = USERNAME_BASE_MAX;
        while !base.is_char_boundary(end) {
            end -= 1;
        }
        base.truncate(end);
    }
    let base = base.trim_end_matches('-');
    let base = if base.is_empty() { "member" } else { base };
    let mut bytes = [0u8; USERNAME_SUFFIX_LEN];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let suffix: String = bytes
        .iter()
        .map(|b| char::from(USERNAME_SUFFIX_ALPHABET[usize::from(b % 32)]))
        .collect();
    format!("{base}-{suffix}")
}

/// Whether a built-in role may be the default for registrations.
#[must_use]
pub fn builtin_role_may_be_default(role: Role) -> bool {
    !role_caps(role)
        .iter()
        .any(|cap| FORBIDDEN_DEFAULT_ROLE_CAPS.contains(cap))
}

/// Whether a custom role holding these capability names may be the default
/// for registrations.
#[must_use]
pub fn capabilities_may_be_default(names: &[String]) -> bool {
    !names.iter().any(|name| {
        FORBIDDEN_DEFAULT_ROLE_CAPS
            .iter()
            .any(|cap| cap_name(*cap) == name)
    })
}

/// A confirmation (or reset) token nobody can guess: 256 bits from the
/// operating system's generator.
#[must_use]
pub fn fresh_token() -> String {
    let mut buf = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    format!("vy-{}", hex::encode(buf))
}

/// What is stored for a token: its SHA-256, hex-encoded (the same form
/// password-reset tokens are stored in).
#[must_use]
pub fn hash_token(raw: &str) -> String {
    hex::encode(Sha256::digest(raw))
}

/// What the registration form needs to know before it is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegistrationStatus {
    /// Whether visitors may register at all.
    pub enabled: bool,
    /// The shortest password that will be accepted.
    pub password_min_length: usize,
}

/// A visitor's registration request.
#[derive(Clone, Debug)]
pub struct NewRegistration {
    /// Address; surrounding whitespace is dropped, case does not matter.
    pub email: String,
    /// Display name; also the start of the generated username. When absent
    /// or blank the generated username stands in.
    pub display_name: Option<String>,
    /// Password, under the rule every set-password path applies.
    pub password: String,
}

/// What a well-formed registration came to. The caller answers the client
/// identically for all three and sends the mail the variant asks for.
#[derive(Clone, Debug)]
pub enum RegisterOutcome {
    /// A new, unconfirmed account was made. Mail `user.email` a
    /// confirmation link carrying `token`.
    Created {
        /// The new account.
        user: UserRow,
        /// The raw confirmation token (only its hash is stored).
        token: String,
    },
    /// The address already has an unconfirmed account. Nothing about it
    /// was replaced; mail `user.email` a confirmation link carrying the
    /// fresh `token` (within the caller's mail limits).
    ConfirmationResent {
        /// The existing unconfirmed account.
        user: UserRow,
        /// A fresh raw confirmation token.
        token: String,
        /// The account has no password: this or an earlier repeat came
        /// with a different one (see the module documentation). The mail
        /// should say a password will have to be set after confirming.
        password_cleared: bool,
    },
    /// The address belongs to a confirmed account, or a suspended one.
    /// Nothing was changed and no token was made; mail `user.email` a
    /// "you already have an account" note.
    AlreadyRegistered {
        /// The existing account, untouched.
        user: UserRow,
    },
}

/// What a request to send the confirmation again came to. The caller
/// answers the client identically for both.
// Made once per request and matched at once; boxing the row would only
// make every caller unbox it.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum ResendOutcome {
    /// An unconfirmed account has this address: mail it a confirmation
    /// link carrying `token` (within the caller's mail limits).
    Send {
        /// The unconfirmed account.
        user: UserRow,
        /// A fresh raw confirmation token.
        token: String,
        /// The account has no password and will need one after confirming.
        needs_password: bool,
    },
    /// No account, a confirmed one, or a suspended one: send nothing.
    Nothing,
}

/// What opening a confirmation link came to.
// Made once per request and matched at once, as `ResendOutcome`.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum VerifyOutcome {
    /// The address is confirmed.
    Confirmed {
        /// The account, now confirmed.
        user: UserRow,
        /// The account has no password (it was opened without one, or a
        /// contest removed it; see the module documentation): mail its
        /// owner a set-password link.
        needs_password: bool,
    },
    /// A password was given and it is not the account's (or the account
    /// has none). Nothing changed and the link still works: whoever holds
    /// it may try again, or confirm without a password.
    PasswordMismatch,
}

/// Registration, confirmation and re-sending.
#[derive(Clone, Debug)]
pub struct RegistrationService {
    users: UsersRepo,
    tokens: TokensRepo,
    roles: RolesRepo,
    options: OptionsService,
}

/// The role a new registration gets.
enum DefaultRole {
    Builtin(Role),
    Custom(String),
}

impl RegistrationService {
    /// Creates the service.
    #[must_use]
    pub fn new(
        users: UsersRepo,
        tokens: TokensRepo,
        roles: RolesRepo,
        options: OptionsService,
    ) -> Self {
        Self {
            users,
            tokens,
            roles,
            options,
        }
    }

    /// Whether registration is open, and the password rule.
    ///
    /// # Errors
    ///
    /// Propagates repository errors.
    pub async fn status(&self) -> Result<RegistrationStatus, AppError> {
        Ok(RegistrationStatus {
            enabled: self.options.registration_enabled().await?,
            password_min_length: MIN_PASSWORD_LENGTH,
        })
    }

    /// Registers an account, or works out what the address is owed
    /// instead. See [`RegisterOutcome`].
    ///
    /// A password is hashed or verified exactly once on every path that
    /// gets past validation, whether or not the address has an account, so
    /// timing does not tell them apart. The username is generated (see the
    /// module documentation); nothing the visitor sends is reserved unless
    /// an account is made.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Forbidden`] when registration is off,
    /// [`AppError::Validation`] for a malformed address, display name or
    /// password, [`AppError::Db`] on database failure (including, in
    /// practice never, running out of generated usernames).
    pub async fn register(&self, input: NewRegistration) -> Result<RegisterOutcome, AppError> {
        if !self.options.registration_enabled().await? {
            return Err(AppError::forbidden("registration is closed"));
        }
        let (email, display_name) = checked(&input)?;

        let existing = self.find(&email).await?;
        // The one argon2 computation every request pays for.
        let (new_hash, same_password) = match &existing {
            None => (Some(password::hash_password(&input.password)?), false),
            Some(user) => (None, password_matches(user, &input.password)),
        };

        let Some(user) = existing else {
            let hash = new_hash.unwrap_or_default();
            return self
                .create(&email, display_name.as_deref(), &hash, &input.password)
                .await;
        };
        self.existing(user, same_password).await
    }

    /// A registration whose mail the caller will not send (its limits are
    /// spent): no account is made, no token issued, but the contest step
    /// still runs. An unconfirmed account whose password this request
    /// does not match loses it, exactly as in [`Self::register`];
    /// otherwise someone who spent an address's mail budget first could
    /// keep their password through the owner's own registration. One
    /// argon2 operation on every path, as in [`Self::register`].
    ///
    /// # Errors
    ///
    /// As [`Self::register`].
    pub async fn register_without_mail(&self, input: NewRegistration) -> Result<(), AppError> {
        if !self.options.registration_enabled().await? {
            return Err(AppError::forbidden("registration is closed"));
        }
        let (email, _) = checked(&input)?;
        match self.find(&email).await? {
            None => {
                drop(password::hash_password(&input.password)?);
            }
            Some(user) => {
                let same_password = password_matches(&user, &input.password);
                if awaiting_confirmation(&user) && has_password(&user) && !same_password {
                    // Two registrations, two passwords: neither stands.
                    self.users.clear_unconfirmed_password(user.id).await?;
                }
            }
        }
        Ok(())
    }

    /// Redeems a confirmation token. With `password`, the account's
    /// stored password is kept if `password` is it, and nothing happens if
    /// not ([`VerifyOutcome::PasswordMismatch`]: the token is not spent).
    /// Without, the address is confirmed and any stored password removed:
    /// a password the mailbox's owner did not show they know never becomes
    /// a working credential (see the module documentation). On
    /// confirmation the token is spent and the account's other
    /// confirmation links stop working.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] when the token is unknown, already
    /// used, expired, not a confirmation token, or its account is gone;
    /// [`AppError::Db`] on database failure.
    pub async fn verify(
        &self,
        token: &str,
        password: Option<&str>,
    ) -> Result<VerifyOutcome, AppError> {
        let token_hash = hash_token(token);
        let invalid = || AppError::validation("this confirmation link is invalid or has expired");
        let Some(password) = password else {
            let user = self
                .users
                .confirm_email_with_token(&token_hash, ConfirmPassword::Clear)
                .await?
                .ok_or_else(invalid)?;
            return Ok(VerifyOutcome::Confirmed {
                needs_password: !has_password(&user),
                user,
            });
        };
        let holder = self
            .users
            .confirmation_token_holder(&token_hash)
            .await?
            .ok_or_else(invalid)?;
        if !password_matches(&holder, password) {
            return Ok(VerifyOutcome::PasswordMismatch);
        }
        let checked = holder.password_hash.as_deref().unwrap_or_default();
        if let Some(user) = self
            .users
            .confirm_email_with_token(&token_hash, ConfirmPassword::KeepIf(checked))
            .await?
        {
            return Ok(VerifyOutcome::Confirmed {
                needs_password: !has_password(&user),
                user,
            });
        }
        // Between the look and the write, either the link was spent or the
        // password changed (a contest removed it).
        match self.users.confirmation_token_holder(&token_hash).await? {
            Some(_) => Ok(VerifyOutcome::PasswordMismatch),
            None => Err(invalid()),
        }
    }

    /// Issues a fresh confirmation token for the unconfirmed account with
    /// this address, if there is one. See [`ResendOutcome`].
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure. A malformed or
    /// unknown address is [`ResendOutcome::Nothing`], not an error.
    pub async fn resend(&self, email: &str) -> Result<ResendOutcome, AppError> {
        let Ok(email) = normalized_email(email) else {
            return Ok(ResendOutcome::Nothing);
        };
        match self.find(&email).await? {
            Some(user) if awaiting_confirmation(&user) => {
                let token = self.issue(&user).await?;
                Ok(ResendOutcome::Send {
                    needs_password: !has_password(&user),
                    user,
                    token,
                })
            }
            _ => Ok(ResendOutcome::Nothing),
        }
    }

    /// Issues a fresh confirmation token for `user`: an administrator's
    /// "send it again". Whether the caller may act on this account is the
    /// caller's to decide first.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] when the account is already
    /// confirmed, [`AppError::Db`] on database failure.
    pub async fn issue_confirmation(&self, user: &UserRow) -> Result<String, AppError> {
        if user.email_verified_at.is_some() {
            return Err(AppError::validation(
                "this account's email address is already confirmed",
            ));
        }
        self.issue(user).await
    }

    /// Confirms an account's address on an administrator's word, ending
    /// its outstanding confirmation links. An administrator vouches for
    /// the mailbox, not for the password a registrant typed, so an account
    /// confirmed by this loses that password and the caller mails its
    /// owner a set-password link; an account confirmed already is left as
    /// it is. Whether the caller may act on this account is the caller's
    /// to decide first.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when missing, [`AppError::Db`] on
    /// database failure.
    pub async fn confirm(&self, user_id: i64) -> Result<UserRow, AppError> {
        self.users.confirm_email_clearing_password(user_id).await
    }

    async fn find(&self, email: &str) -> Result<Option<UserRow>, AppError> {
        match self.users.get_by_email(email).await {
            Ok(user) => Ok(Some(user)),
            Err(AppError::NotFound { .. }) => Ok(None),
            Err(err) => Err(err),
        }
    }

    async fn issue(&self, user: &UserRow) -> Result<String, AppError> {
        let token = fresh_token();
        self.tokens
            .issue(
                user.id,
                TokenPurpose::Verify,
                &hash_token(&token),
                VERIFY_TOKEN_TTL,
            )
            .await?;
        Ok(token)
    }

    /// The address has an account already.
    async fn existing(
        &self,
        user: UserRow,
        same_password: bool,
    ) -> Result<RegisterOutcome, AppError> {
        if !awaiting_confirmation(&user) {
            return Ok(RegisterOutcome::AlreadyRegistered { user });
        }
        let user = if same_password || !has_password(&user) {
            user
        } else {
            // Two registrations, two passwords: neither stands.
            self.users.clear_unconfirmed_password(user.id).await?;
            self.users.get(user.id).await?
        };
        // Confirmed in the meantime: it is simply an account now.
        if user.email_verified_at.is_some() {
            return Ok(RegisterOutcome::AlreadyRegistered { user });
        }
        let token = self.issue(&user).await?;
        Ok(RegisterOutcome::ConfirmationResent {
            password_cleared: !has_password(&user),
            user,
            token,
        })
    }

    /// Nobody has the address: make the account, under a generated
    /// username (a fresh one while the last was taken).
    async fn create(
        &self,
        email: &str,
        display_name: Option<&str>,
        password_hash: &str,
        password: &str,
    ) -> Result<RegisterOutcome, AppError> {
        let mut default_role = self.default_role().await?;
        for _ in 0..USERNAME_ATTEMPTS {
            let username = derived_username(
                Some(&generated_username(display_name.unwrap_or_default())),
                email,
            )?;
            let (role, custom) = match &default_role {
                DefaultRole::Builtin(role) => (*role, None),
                DefaultRole::Custom(slug) => (Role::Subscriber, Some(slug.as_str())),
            };
            let new = NewUser {
                id: vyasa_common::next_id_i64(),
                email,
                username: &username,
                display_name: display_name.unwrap_or(&username),
                password_hash: Some(password_hash),
                role,
                bio: "",
            };
            match self.users.insert_unconfirmed(&new, custom).await {
                Ok(user) => {
                    let token = self.issue(&user).await?;
                    return Ok(RegisterOutcome::Created { user, token });
                }
                Err(UnconfirmedInsertError::UsernameTaken) => {}
                // Deleted between the look and the write.
                Err(UnconfirmedInsertError::RoleMissing) => {
                    default_role = DefaultRole::Builtin(Role::Subscriber);
                }
                // Lost a race to the same address: it is the existing
                // account it now is.
                Err(UnconfirmedInsertError::EmailTaken) => {
                    let Some(user) = self.find(email).await? else {
                        return Err(AppError::db("a registration raced a deletion"));
                    };
                    let same_password = password_matches(&user, password);
                    return self.existing(user, same_password).await;
                }
                Err(UnconfirmedInsertError::Other(err)) => return Err(err),
            }
        }
        Err(AppError::db("no free username could be generated"))
    }

    /// The role a registration made now would get: the configured one's
    /// name or slug, or `subscriber` when that one can no longer be given
    /// (see [`Self::default_role`]). Compared with
    /// [`OptionsService::registration_default_role`], it says whether the
    /// setting is silently falling back.
    ///
    /// # Errors
    ///
    /// Propagates repository errors.
    pub async fn effective_default_role(&self) -> Result<String, AppError> {
        Ok(match self.default_role().await? {
            DefaultRole::Builtin(role) => role.as_str().to_owned(),
            DefaultRole::Custom(slug) => slug,
        })
    }

    /// The configured default role if it is still one a stranger may be
    /// given, `subscriber` otherwise: a role that is missing, was widened
    /// past [`FORBIDDEN_DEFAULT_ROLE_CAPS`] since it was chosen, or was
    /// written past the option's validation.
    async fn default_role(&self) -> Result<DefaultRole, AppError> {
        let configured = self.options.registration_default_role().await?;
        if let Ok(role) = Role::parse(&configured) {
            return Ok(DefaultRole::Builtin(if builtin_role_may_be_default(role) {
                role
            } else {
                Role::Subscriber
            }));
        }
        match self.roles.get(&configured).await {
            Ok(role) if capabilities_may_be_default(&role.capabilities) => {
                Ok(DefaultRole::Custom(role.slug))
            }
            Ok(_) | Err(AppError::NotFound { .. }) => Ok(DefaultRole::Builtin(Role::Subscriber)),
            Err(err) => Err(err),
        }
    }
}

/// Checks a registration's shape without touching storage, and returns the
/// address as it is stored and looked up. [`RegistrationService::register`]
/// applies the same checks; a caller runs this first to answer a malformed
/// request before spending a rate limit on it or deciding anything else.
///
/// # Errors
///
/// [`AppError::Validation`] for a malformed address, display name or
/// password.
pub fn validate_registration(input: &NewRegistration) -> Result<String, AppError> {
    checked(input).map(|(email, _)| email)
}

/// The address and the display name of a well-formed registration.
fn checked(input: &NewRegistration) -> Result<(String, Option<String>), AppError> {
    let email = normalized_email(&input.email)?;
    let display_name = input
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned);
    if display_name.as_deref().is_some_and(|name| {
        name.chars().count() > MAX_DISPLAY_NAME_CHARS || name.chars().any(char::is_control)
    }) {
        return Err(AppError::validation(
            "display name must be at most 100 characters",
        ));
    }
    password::validate_password(&input.password)?;
    Ok((email, display_name))
}

/// An account registration may still act on: unconfirmed, not suspended.
fn awaiting_confirmation(user: &UserRow) -> bool {
    user.email_verified_at.is_none() && user.suspended_at.is_none()
}

fn has_password(user: &UserRow) -> bool {
    user.password_hash
        .as_deref()
        .is_some_and(|hash| !hash.is_empty())
}

/// Whether `attempt` is the account's password. Costs one argon2
/// verification whether or not the account has a password.
fn password_matches(user: &UserRow, attempt: &str) -> bool {
    match user.password_hash.as_deref() {
        Some(hash) if !hash.is_empty() => password::verify_password(attempt, hash).is_ok(),
        _ => {
            drop(password::verify_password(attempt, DUMMY_HASH));
            false
        }
    }
}

/// An address as `citext` compares it: surrounding whitespace dropped and
/// each character lowercased as PostgreSQL's `lower()` does, one character
/// for one. Rust's full mapping turns 'İ' (U+0130) into two characters
/// ("i" and a combining dot); the database makes it 'i', so the first is
/// taken. The Kelvin sign (U+212A) is 'k' either way. Folding can change
/// the length in bytes ('Ⱥ' is two, its lowercase 'ⱥ' three).
#[must_use]
pub fn fold_email(raw: &str) -> String {
    raw.trim()
        .chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

/// The address as it is stored and looked up: surrounding whitespace
/// dropped (case is the column's business: it is `citext`). This is the
/// one rule for what is an address: at most 254 bytes (the limit SMTP
/// puts on a path) both as given and folded ([`fold_email`], the form
/// the per-address buckets count by — so an address accepted here can
/// always be counted and mailed), no whitespace or control characters,
/// and an `@` between a local part and a domain. Callers that count
/// requests per address check it first, so nothing else is ever counted.
///
/// # Errors
///
/// [`AppError::Validation`] for anything else.
pub fn normalized_email(raw: &str) -> Result<String, AppError> {
    let email = raw.trim();
    let well_formed = email.len() <= MAX_EMAIL_LENGTH
        && fold_email(email).len() <= MAX_EMAIL_LENGTH
        && !email.chars().any(|c| c.is_whitespace() || c.is_control())
        && email
            .split_once('@')
            .is_some_and(|(local, domain)| !local.is_empty() && !domain.is_empty());
    if !well_formed {
        return Err(AppError::validation("email must be a valid address"));
    }
    Ok(email.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        builtin_role_may_be_default, capabilities_may_be_default, fold_email, fresh_token,
        generated_username, hash_token, normalized_email, validate_registration, NewRegistration,
    };
    use vyasa_db::models::Role;

    /// A stranger gets at most what an author has: nothing over other
    /// people's work, nothing over the site.
    #[test]
    fn administrators_and_editors_are_refused_among_built_in_roles() {
        for role in Role::ALL {
            assert_eq!(
                builtin_role_may_be_default(role),
                !matches!(role, Role::Admin | Role::Editor),
                "{role:?}"
            );
        }
    }

    #[test]
    fn any_one_capability_over_others_rules_a_custom_role_out() {
        let caps = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
        assert!(capabilities_may_be_default(&caps(&[])));
        assert!(capabilities_may_be_default(&caps(&[
            "view_admin",
            "edit_posts",
            "publish_posts",
            "delete_posts",
            "upload_media",
            "not_a_capability",
        ])));
        for cap in [
            "manage_users",
            "manage_options",
            "manage_plugins",
            "manage_themes",
            "edit_others",
            "moderate_comments",
            // The site's shared categories: past an author's power.
            "manage_categories",
        ] {
            assert!(
                !capabilities_may_be_default(&caps(&["view_admin", cap])),
                "{cap}"
            );
        }
    }

    #[test]
    fn tokens_are_long_unique_and_stored_as_their_sha256() {
        let (a, b) = (fresh_token(), fresh_token());
        assert_ne!(a, b);
        assert_eq!(a.len(), 3 + 64);
        assert_eq!(
            hash_token("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn a_generated_username_is_the_display_name_and_a_random_suffix() {
        let name = generated_username("Ada Lovelace");
        let (base, suffix) = name.rsplit_once('-').unwrap();
        assert_eq!(base, "ada-lovelace");
        assert_eq!(suffix.len(), 6);
        assert!(suffix
            .bytes()
            .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b)));
        // Already a slug: the username rule leaves it as it is.
        assert_eq!(vyasa_common::slugify(&name), name);
        // Not derivable: two for the same name differ.
        let many: std::collections::HashSet<String> = (0..50)
            .map(|_| generated_username("Ada Lovelace"))
            .collect();
        assert_eq!(many.len(), 50);
        // Nothing usable in the name: a neutral base.
        for empty in ["", "   ", "!!!", "-- --"] {
            assert!(
                generated_username(empty).starts_with("member-"),
                "{empty:?}"
            );
        }
        // A long name is cut, never mid-character, never ending in a dash.
        let long = generated_username(&"é".repeat(80));
        let (base, _) = long.rsplit_once('-').unwrap();
        assert!(base.len() <= 40 && !base.ends_with('-'), "{long}");
    }

    #[test]
    fn addresses_are_trimmed_and_checked_for_shape() {
        assert_eq!(normalized_email("  A@x.com\n").unwrap(), "A@x.com");
        for bad in [
            "",
            "a",
            "@x.com",
            "a@",
            "a b@x.com",
            "a@x.com\r\nBcc: e@x.com",
        ] {
            assert!(normalized_email(bad).is_err(), "{bad:?}");
        }
        assert!(normalized_email(&format!("{}@x.com", "a".repeat(250))).is_err());
    }

    /// The length rule holds for the address as typed and as the database
    /// folds it, so an address accepted here can always be counted (and
    /// mailed) by the per-address buckets, which see the folded form.
    /// 'Ⱥ' (U+023A, two bytes) lowercases to 'ⱥ' (U+2C65, three).
    #[test]
    fn the_length_rule_holds_for_the_folded_address_too() {
        let domain = "@x.test";
        let fits = format!("{}{domain}", "a".repeat(254 - domain.len()));
        assert!(normalized_email(&fits).is_ok());
        // 123 of them: 246 + 7 = 253 bytes as typed, 369 + 7 folded.
        let grows = format!("{}{domain}", "\u{23A}".repeat(123));
        assert!(grows.len() <= 254);
        assert!(fold_email(&grows).len() > 254);
        assert!(normalized_email(&grows).is_err(), "folded past the limit");
        // A short one is fine either way, and folds as the database does.
        assert_eq!(fold_email(" \u{23A}b@X.test "), "\u{2C65}b@x.test");
        assert!(normalized_email("\u{23A}b@x.test").is_ok());
    }

    #[test]
    fn a_registration_is_checked_before_anything_is_looked_up() {
        let input = |email: &str, name: Option<&str>, password: &str| NewRegistration {
            email: email.to_owned(),
            display_name: name.map(str::to_owned),
            password: password.to_owned(),
        };
        assert_eq!(
            validate_registration(&input(" A@x.com ", Some("Ada"), "long-enough")).unwrap(),
            "A@x.com"
        );
        assert!(validate_registration(&input("a@x.com", None, "long-enough")).is_ok());
        assert!(validate_registration(&input("nope", None, "long-enough")).is_err());
        assert!(validate_registration(&input("a@x.com", None, "short")).is_err());
        assert!(
            validate_registration(&input("a@x.com", Some(&"n".repeat(101)), "long-enough"))
                .is_err()
        );
        assert!(validate_registration(&input("a@x.com", Some("a\u{7}b"), "long-enough")).is_err());
    }
}
