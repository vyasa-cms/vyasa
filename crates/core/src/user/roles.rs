//! Administrator-defined roles: validation, the escalation guard, and
//! assignment.
//!
//! A custom role is a name and a set of the existing capabilities. The
//! guard that keeps this from becoming a way to grant oneself more: a
//! caller may only create, edit, delete or assign a role whose
//! capabilities they all hold themselves.

use vyasa_common::AppError;
use vyasa_db::models::{Role, RoleRow, UserRow};
use vyasa_db::repo::{NewRole, RoleUpdate, RolesRepo, UsersRepo};

use super::rbac::{cap_name, effective_caps, parse_cap, Capability};

/// Longest role name, in characters.
const NAME_MAX: usize = 60;
/// Slug length bounds, in bytes (slugs are ASCII).
const SLUG_MIN: usize = 2;
const SLUG_MAX: usize = 40;

/// A new custom role.
#[derive(Clone, Debug)]
pub struct RoleInput {
    /// Identifier: `^[a-z0-9][a-z0-9-]{1,39}$`, not a built-in role name.
    pub slug: String,
    /// Display name, 1-60 characters.
    pub name: String,
    /// What the role is for.
    pub description: String,
    /// Capability names (snake_case). Duplicates collapse; may be empty.
    pub capabilities: Vec<String>,
}

/// Changes to a custom role; `None` leaves a field as it is.
#[derive(Clone, Debug, Default)]
pub struct RolePatch {
    /// New slug; users holding the role keep it.
    pub slug: Option<String>,
    /// New display name.
    pub name: Option<String>,
    /// New description.
    pub description: Option<String>,
    /// New capability list (replaces the old one).
    pub capabilities: Option<Vec<String>>,
}

/// Custom role management.
#[derive(Clone, Debug)]
pub struct RolesService {
    roles: RolesRepo,
    users: UsersRepo,
}

impl RolesService {
    /// Creates a service over the roles and users repositories.
    #[must_use]
    pub fn new(roles: RolesRepo, users: UsersRepo) -> Self {
        Self { roles, users }
    }

    /// Every custom role with the number of users holding it, by slug.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list(&self) -> Result<Vec<(RoleRow, i64)>, AppError> {
        self.roles.list().await
    }

    /// Fetches a custom role by slug.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when there is no such role.
    pub async fn get(&self, slug: &str) -> Result<RoleRow, AppError> {
        self.roles.get(slug).await
    }

    /// Creates a custom role on behalf of `actor`.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] for a bad or reserved slug, a bad
    /// name or an unknown capability; [`AppError::Forbidden`] when the
    /// role holds a capability `actor` lacks; [`AppError::Conflict`] when
    /// the slug is taken; [`AppError::Db`] on database failure.
    pub async fn create(&self, actor: &UserRow, input: RoleInput) -> Result<RoleRow, AppError> {
        ensure_within(actor, &validate_caps(&input.capabilities)?)?;
        self.restore(input).await
    }

    /// Creates a custom role with no acting user, so with no escalation
    /// guard: for a site import, where the caller has decided who is
    /// importing (an operator at the command line is bounded by nobody).
    ///
    /// # Errors
    ///
    /// As [`Self::create`], without [`AppError::Forbidden`].
    pub async fn restore(&self, input: RoleInput) -> Result<RoleRow, AppError> {
        validate_slug(&input.slug)?;
        let name = validate_name(&input.name)?;
        let caps = validate_caps(&input.capabilities)?;
        self.roles
            .insert(&NewRole {
                slug: &input.slug,
                name,
                description: input.description.trim(),
                capabilities: &cap_names(&caps),
            })
            .await
    }

    /// Updates the custom role `slug` on behalf of `actor`.
    ///
    /// The guard applies to the role as it is and as it will be: a caller
    /// edits only a role they could have granted, and only into one they
    /// could grant. Without the first half, a caller could strip a role
    /// held by accounts beyond their reach and so bring those accounts
    /// within it.
    ///
    /// # Errors
    ///
    /// As [`Self::create`], plus [`AppError::NotFound`] when there is no
    /// such role. [`AppError::Forbidden`] also when the role as stored
    /// holds a capability `actor` lacks.
    pub async fn update(
        &self,
        actor: &UserRow,
        slug: &str,
        patch: RolePatch,
    ) -> Result<RoleRow, AppError> {
        if let Some(new_slug) = &patch.slug {
            validate_slug(new_slug)?;
        }
        let name = patch.name.as_deref().map(validate_name).transpose()?;
        let new_caps = patch
            .capabilities
            .as_deref()
            .map(validate_caps)
            .transpose()?;
        ensure_within(
            actor,
            &known_caps(&self.roles.get(slug).await?.capabilities),
        )?;
        if let Some(caps) = &new_caps {
            ensure_within(actor, caps)?;
        }
        let names = new_caps.as_deref().map(cap_names);
        self.roles
            .update(
                slug,
                &RoleUpdate {
                    slug: patch.slug.as_deref(),
                    name,
                    description: patch.description.as_deref().map(str::trim),
                    capabilities: names.as_deref(),
                },
            )
            .await
    }

    /// Deletes a custom role no user holds, on behalf of `actor`, who
    /// must hold every capability in it (as for editing it).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when there is no such role,
    /// [`AppError::Forbidden`] when it holds a capability `actor` lacks,
    /// [`AppError::Conflict`] naming the number of users when it is in
    /// use.
    pub async fn delete(&self, actor: &UserRow, slug: &str) -> Result<(), AppError> {
        ensure_within(
            actor,
            &known_caps(&self.roles.get(slug).await?.capabilities),
        )?;
        self.roles.delete(slug).await
    }

    /// Gives `user_id` the custom role `slug` on behalf of `actor`, or
    /// with `None` takes their custom role away (leaving a subscriber).
    ///
    /// The user's built-in role becomes `subscriber`, so this is refused
    /// for the last active admin.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] for a missing role or user,
    /// [`AppError::Forbidden`] when the role holds a capability `actor`
    /// lacks, [`AppError::Validation`] for the last active admin.
    pub async fn assign(
        &self,
        actor: &UserRow,
        user_id: i64,
        slug: Option<&str>,
    ) -> Result<(), AppError> {
        if let Some(slug) = slug {
            let role = self.roles.get(slug).await?;
            ensure_within(actor, &known_caps(&role.capabilities))?;
        }
        self.users.set_custom_role(user_id, slug).await
    }
}

/// `^[a-z0-9][a-z0-9-]{1,39}$`, and not a built-in role name.
fn validate_slug(slug: &str) -> Result<(), AppError> {
    let well_formed = (SLUG_MIN..=SLUG_MAX).contains(&slug.len())
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !slug.starts_with('-');
    if !well_formed {
        return Err(AppError::validation(
            "a role slug is 2 to 40 lowercase letters, digits and hyphens, \
             and does not start with a hyphen",
        ));
    }
    if Role::ALL.iter().any(|role| role.as_str() == slug) {
        return Err(AppError::validation(format!(
            "{slug:?} is a built-in role; choose another slug"
        )));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<&str, AppError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > NAME_MAX {
        return Err(AppError::validation(
            "a role name is 1 to 60 characters long",
        ));
    }
    Ok(name)
}

/// Parses capability names, collapsing duplicates and keeping order.
fn validate_caps(names: &[String]) -> Result<Vec<Capability>, AppError> {
    let mut caps = Vec::with_capacity(names.len());
    for name in names {
        let cap = parse_cap(name)
            .ok_or_else(|| AppError::validation(format!("unknown capability: {name:?}")))?;
        if !caps.contains(&cap) {
            caps.push(cap);
        }
    }
    Ok(caps)
}

/// The capabilities a stored role grants (unknown names grant nothing).
fn known_caps(names: &[String]) -> Vec<Capability> {
    names.iter().filter_map(|name| parse_cap(name)).collect()
}

fn cap_names(caps: &[Capability]) -> Vec<String> {
    caps.iter().map(|cap| cap_name(*cap).to_owned()).collect()
}

/// The escalation guard: every capability in `caps` is held by `actor`.
fn ensure_within(actor: &UserRow, caps: &[Capability]) -> Result<(), AppError> {
    let held = effective_caps(actor);
    let missing: Vec<&str> = caps
        .iter()
        .filter(|cap| !held.contains(cap))
        .map(|cap| cap_name(*cap))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(AppError::forbidden(format!(
            "this role holds capabilities you do not have: {}",
            missing.join(", ")
        )))
    }
}
