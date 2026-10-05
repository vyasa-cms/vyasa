# Security

What is enforced, where, and what is deliberately not covered. Verified
against the tree on 2026-08-28; the Authorization section was added and
verified on 2026-09-30; the Roles section was added on 2026-09-30 and
revised the same day after the phase 97 final review (full administrator,
administrator-level capabilities, `view_admin`); the Registration section
and the "Registration limits" paragraph under Brute force were added
2026-09-30 for phase 98 and revised 2026-10-01 after its final review
(the confirm-time password rule, reset and administrator confirmation,
`/auth/forgot` limits, the default-role line, IPv6 /64 keying).

## Reporting

Security reports go to the maintainer privately rather than through a public
issue. This is a single-maintainer project: there is no LTS branch and fixes
land on the current release line.

## Authentication

- **Sessions** — `vy_session` cookie, HttpOnly, `SameSite=Lax`, 14-day
  expiry. (Renamed from `rp_session` on 2026-08-28; the change logs out
  existing sessions.)
- **Passwords** — Argon2id, via `vyasa_core::user::password`.
- **API keys** — `vy_` plus 32 bytes of hex, stored only as a SHA-256 hash;
  lookup is by the full hash. The raw key is returned once at creation and
  cannot be recovered. A key can never grant a capability its creator does
  not hold.
- **Reset tokens** — single-use, one hour, SHA-256 hashed at rest, consumed
  atomically. `POST /auth/forgot` always returns 200, including for
  addresses with no account and when the email fails to queue, so the
  response cannot be used to enumerate accounts.

## Authorization

- **Route access** — every `/api/v1` route registers through
  `authz::Guarded` with an explicit `Access` (`Public`, `Authenticated`,
  `Cap`, `AnyOf`). A wrapper resolves the `Principal` once: 401 with none
  on a non-`Public` route, 403 lacking the capability (`Principal::ensure`,
  so an API key's own grants apply), else the handler runs, reusing that
  `Principal` for `CurrentUser`/`MaybePrincipal` — one session lookup.
- **The one exception** — `ANY /api/v1/plugin/{name}/{*rest}` is
  registered outside `Guarded` (`crates/api/src/public/mod.rs`). It is
  public by design: the request goes to the plugin that declared the path,
  and that plugin's own checks decide. A test fails if any other `/api/v1`
  route is registered outside `rest/`, or if the REST router is mounted
  anywhere but the single `nest("/api/v1", rest::router())`.
- **Resource ownership** — record-dependent decisions ("own entry or
  `EditOthers`") are named functions in `crates/api/src/policy.rs`.
- **Content types (phase 99)** — creating, relabelling and deleting types
  and managing fields (including clean-up of orphaned values) need
  `manage_options`; editors may read the type and field lists. Entries of a
  custom type follow the post capabilities and `policy.rs` rules. Entries
  of a non-public type (and `block`) are hidden from every caller without
  `edit_posts` on every surface: REST get answers `404` and lists leave them
  out, GraphQL `post` is `null`, and search (Tantivy and the SQL fallback),
  related posts, semantic rerank, link suggestions, translations and
  hreflang, sitemap, `llms.txt`, newsletter mail and IndexNow pings all
  exclude them. **Reusable blocks are private to those who edit
  content**: they are internal patterns other entries embed, so since
  phase 99 a REST read of a `block` entry by a caller without `edit_posts`
  (REST post reads need a sign-in, as before) answers `404`, GraphQL
  `post`/`postBySlug` give `null` to anonymous callers and subscribers
  alike, and listings leave them out (they were readable before, like any
  published entry). The admin's
  pattern library, used by those who edit content, is unaffected. The comment
  endpoints (REST `GET`/`POST /posts/{id}/comments`, GraphQL
  `comments`/`submitComment`) and the HTML `/comment` form decide through
  one rule, `policy::comment_target`: a missing entry, a draft the caller
  cannot see, an entry of a type the caller may not read, and a published
  password-protected entry the caller has not unlocked and may not edit
  are all answered as missing (`404 post_not_found`, GraphQL's not-found
  error, the themed 404 page), before the body is validated, so they say
  nothing about whether a hidden entry exists. A protected entry is
  unlocked by the signed token from `POST /posts/{id}/verify-password`
  (REST `?token=`) or the entry page's unlock cookie (REST and the HTML
  form); GraphQL has no unlock, so there only those who may edit the entry
  comment on it. Someone who can see a draft is told `400 Comments are
  closed on this post.` instead. `fields_missing` (the keys whose media or
  entry reference no longer resolves) is served only to a caller who may
  edit the entry, on single-entry responses; everyone else gets `null`.
  Field values are autoescaped in themes and never rendered as raw HTML;
  references that no longer resolve for the public render nothing, and URL
  values are re-checked at render time.
- **Tests** — a matrix sends every rule against every role at the real
  router (`EXCEPTIONS`/`PUBLIC_GATES` name the routes that legitimately
  differ); `docs/ROUTE-ACCESS.md`, generated from the route table, fails
  its test if it drifts — a permission change is a visible diff.

## Roles

- **Built-in roles are fixed** — `admin`, `editor`, `author`, `contributor`,
  `subscriber` and their capabilities never change; every "is this an
  administrator" check (last-admin protection, suspension, setup) still
  means the built-in role.
- **Custom roles** are a name plus a subset of the twelve capabilities,
  assignable like any built-in role; `vyasa_core::user::effective_caps`
  resolves through the custom role when one is set, then applies the
  existing per-user `meta` overrides.
- **The grant guard** — you can only grant, create or edit a role, built-in
  or custom, whose capabilities are a subset of your own; the same guard
  covers creating a user and assigning any role. Importing a site archive
  is not left to this guard: it needs a full administrator (below), who
  holds every capability and so could grant any role in the archive.
- **The reach guard** — you can only manage, erase or export an account
  whose effective capabilities are a subset of your own.
- Both guards live in `crates/api/src/policy.rs`. An API key can never
  exceed its owner's capabilities; last-administrator protection stays on
  the built-in `admin` role, never on a capability a custom role holds.
- **A role change ends outstanding reset links** — changing an account's
  role (built-in or custom), or replacing the capabilities of the custom
  role it holds, deletes its unredeemed password-reset and invitation
  tokens in the same statement or transaction. A link issued while the
  account held little cannot be redeemed after it is promoted.
- **Administrator-level capabilities** — three capabilities are
  administrator-level powers even on their own, and a role holding one
  should be given only to someone trusted as an administrator. The role
  editor says so when one is ticked.
  - `manage_options` — site-wide settings and custom CSS on every page;
    redirects from any address on the site; the whole-site export, which
    contains every account's email address and all unpublished content;
    the AI provider keys and spending cap; the audit log.
  - `manage_plugins` — installing and enabling plugins: code that runs on
    the server (sandboxed, within the capabilities the package declares)
    and scripts that run with full privileges on the site's pages; and
    webhooks, which send the site's events to any address.
  - `manage_themes` — what every visitor sees on every page: installing,
    activating and editing themes and menus, and installing theme
    packages from the marketplace.
- **Full administrator** — the operations through which `manage_options`
  alone could become every other capability need a caller who holds
  *every* capability (`policy::full_administrator`, decided through
  `Principal::ensure`, so an API key needs every grant as well). The
  routes still declare `manage_options`; anyone else is answered 403
  ("only a full administrator can …"). A built-in administrator always
  qualifies. The operations:
  - the mail relay: `PUT /mail/settings` and `POST /mail/test`. Whoever
    chooses the relay receives every password-reset link;
  - writing a security-sensitive option, through `PUT /options/{key}` or
    the bulk `PUT /options` (one such key refuses the whole request):
    `site_url` (the address in reset and invitation links), `smtp_host`,
    `smtp_port`, `smtp_username`, `smtp_password`, `smtp_from` (the relay,
    stored as options). The list is
    `vyasa_core::options::FULL_ADMINISTRATOR_OPTION_KEYS`. Where releases
    and packages come from, and whose signature makes them trusted, is
    not an option at all: it is compiled into the binary
    (`crates/api/src/official.rs`) and only the server's configuration
    can mirror or disable it;
  - `POST /updates/apply`;
  - `POST /import`: an archive brings in content, accounts and (when
    asked) site options wholesale;
  - the setup wizard's steps that write the same things (`/setup/site`,
    `/setup/mail` when it carries a relay, `/setup/mail/test`,
    `/setup/updates`), when a signed-in caller reaches them. The
    first-run path, authenticated by the setup token before any
    administrator exists, is unchanged.
- **`view_admin`** is enforced by the admin application, not the API: a
  signed-in account without it sees only its profile page and sign-out.
  Every API route is still guarded by its own declared capability, so
  this hides screens; it is not what protects data.
- **How a custom role is reported** — REST and GraphQL give the built-in
  base in `role` (always `subscriber` under a custom role) with
  `custom_role`/`customRole` and `role_name`/`roleName`; the account
  events sent to plugins carry `role` and `custom_role` the same way. A
  plugin's `current-viewer` and `GET /viewer` have one `role` field, which
  is the custom role's slug when there is one.

## Registration

Public self-registration (phase 98) is **off by default**
(`registration_enabled`).

- **Accounts start unconfirmed** and cannot sign in or mint API keys until
  the address is confirmed: by the confirmation link, by a password reset
  through the mailbox, or by an administrator.
- **A password the mailbox's owner did not choose or prove never becomes a
  working credential.** Anyone can register anyone's address, and opening
  a confirmation link proves only that something reached the mailbox (mail
  scanners follow links). So `POST /auth/verify` keeps the stored password
  only when the request carries it again (`{token, password}`); without
  it the address is confirmed, the stored password removed and a
  set-password link mailed, with the same `204`. A wrong password is `400
  password_mismatch` and leaves the link working (no attempt limit: the
  token is unguessable and only its holder can try). The keep is a
  compare-and-swap under a row lock, so a contest that clears the password
  in between cannot slip it through. The admin page never posts the token
  on load: the person confirms with their password, or chooses "I don't
  know the password — email me a link to set one"; someone who did not
  sign up is told to close the page (as the mail tells them to ignore it).
- **A password reset confirms.** A reset link is mailed to the address, so
  following it proves the mailbox, and the password set there is the
  owner's: `POST /auth/reset` on an unconfirmed account sets the password
  and confirms the address in one transaction. The token is looked up
  (without being spent) before the new password is checked or hashed, so
  a made-up token costs a lookup, never an argon2 hash.
- **Sign-in answers an unconfirmed account exactly like a wrong
  password** — the ordinary `401 unauthorized`, with no code or message
  that distinguishes it — so a correct password on an unfamiliar address
  reveals nothing about whether the address has an account; the refusal
  counts toward the lockout like any other failed attempt.
- **Usernames are generated**, not chosen: a slug of the display name (or
  `member`) plus a random suffix. A visitor-chosen name would be reserved
  the instant it was typed and so reveal which addresses already have
  accounts.
- **Contested registrations.** If an address is registered more than once
  with different passwords before any of them is confirmed, neither
  stands: the pending account's password is cleared. The mailbox owner
  sets their own password, through the existing set-password link, once
  they confirm — never a password someone else typed.
- **Unconfirmed accounts are never public.** No author page, no name on
  anything public — reassignment, the importers, the GraphQL author
  loader and plugin post attribution all skip them — and an account still
  unconfirmed after 7 days is purged by a scheduled job.
- **A stranger gets at most an author's power.** The default role may not
  hold `manage_users`, `manage_options`, `manage_plugins`,
  `manage_themes`, `edit_others`, `moderate_comments` or
  `manage_categories` — so of the built-in roles, never Administrator or
  Editor. Saving such a default is refused; one saved earlier, later
  widened, deleted or forced into storage falls back to `subscriber` at
  account-creation time (site health warns), and the Settings select
  offers only eligible roles.
- **Both options need a full administrator** —
  `registration_enabled` and `registration_default_role` are on
  `FULL_ADMINISTRATOR_OPTION_KEYS` (see Full administrator, above).
- **The admin "Confirm" action vouches for the mailbox, not the
  password** — it removes the password the account was registered with and
  mails a set-password link (refused, with nothing changed, when there is
  no mail relay or site address). If that link cannot be queued once the
  account is confirmed, the answer says so (`link_sent: false`) and the
  admin tells the administrator to send one from the row. An account
  confirmed already is left as it is.
- **No registrant-typed name in mail.** Every mail a stranger's
  registration can cause (`registration_confirm`, `registration_existing`,
  `password_reset`, `password_set`) leaves the display name out: on an
  account a registration created it is whatever the first registrant
  typed.

See "Registration limits" under Brute force, below, for the rate limits,
`/auth/forgot`'s limits and the mail-budget caveat.

## Brute force

Two independent mechanisms, because each covers the other's gap:

- **Rate limiting** — in-process per-client sliding window. Buckets: `auth`
  (10/60s, login and user creation), `write` (60/60s, any mutating method),
  `global` (300/60s), plus the registration and reset buckets below.
  Single-node; a multi-instance deployment needs a shared store.
- **Lockout** — progressive delay per `(account, IP)`: three free attempts,
  then doubling from one second to a five-minute cap, forgotten after
  fifteen minutes, cleared on success. The delay is returned as a rejection
  rather than by sleeping, since holding the connection open is the resource
  an attacker wants.

Rate limiting alone lets a distributed attempt grind one account, because it
resets every window. Both key on the client address, which is the TCP peer
unless the peer is listed in `VYASA_TRUSTED_PROXIES`; forwarded headers from
anyone else are ignored, so they cannot be spoofed to reset either one.
**An IPv6 client is counted by its /64** (`client_ip::rate_key`), for every
bucket and for the lockout: one subscriber is normally given a whole /64 and
can use any address in it, so counting full addresses handed one client
2^64 fresh buckets. IPv4 is counted by the address, and an IPv4-mapped IPv6
address is IPv4. The exact address (`client_ip::resolve`) is what logging
and audit should record.

**Registration limits** (phase 98). Five registrations and five re-send
requests per client (an IPv4 address, an IPv6 /64) an hour, and three mails
per email address and per account an hour. The per-account count is of
links issued, confirmation and password-reset alike, and is read from the
database; the per-client and per-address buckets are in-process like the
others (per node, forgotten on restart). `/auth/forgot` for an unconfirmed
account goes through the same per-address and per-account mail limits, so
it is not a way around them; and every address, whatever its account, may
ask `/auth/forgot` three times an hour (in-process). Every per-address
bucket, and the sign-in lockout's account, is keyed by the SHA-256 of the
address as `citext` compares it (trimmed, each character lowercased as
PostgreSQL's `lower()` does: 'İ' is `i`, the Kelvin sign `k`), so a key is
a fixed-size digest whatever a client sends and every spelling that finds an
account is one key. The folding matches PostgreSQL's only under a non-C
UTF-8 database locale (the default `LC_CTYPE` of a UTF-8 cluster); a `C`
locale or an ICU collation folds differently, and spellings the database
treats as one account could then fall into different buckets. A mail request (`/auth/forgot`, a re-send) for
something that is not an address by registration's rule, judged on that
folded form (over 254 bytes, no `@` between a local part and a domain,
whitespace), is answered exactly as any other (`200`, `202`) and goes in no
per-address bucket. Registration applies the 254-byte rule to the address
both as typed and folded, so an address it accepts always has a bucket and
can be sent its links ('Ⱥ', U+023A, is two bytes and folds to three). Every sign-in attempt counts in the lockout under its
digest, whatever its shape: malformed input is an unknown address (the
ordinary `401`, slowed down alike), and an administrator-made account whose
address registration would refuse is protected like any other. The sign-in
lookup trims the address as the keys do. Every one of these answers the
same way over the limit as under it (`202`, or `/auth/forgot`'s `200`),
except the per-client buckets, which answer `429` and say nothing about any
account; refusals and suppressions are logged at info without the email
address.
Anyone who knows an address can spend its mail budget (registration and
reset), and by asking again every hour can keep it spent indefinitely: the
owner's own requests then send nothing. It is not a lockout: every
request that spends the budget mails the owner the link it asked for (a
reset link for `/auth/forgot`, a confirmation link for a registration or
re-send), and the owner can use any of those; nor can it take the
account: a request over the mail limit still runs the contest step, and in
any case a stored password survives confirmation only when the person
holding the link types it. An unconfirmed account's correct password is
refused with the ordinary `401` and counts towards the lockout, so signing
in after registering says nothing about whether the address was new.

## CSRF

- Unsafe methods on cookie auth require an `Origin` matching `Host`.
- API-key requests are exempt: an explicit credential is not an ambient one,
  so there is no confused-deputy risk to prevent.
- `SameSite=Lax` blocks cross-site POSTs from ordinary navigation.

## Headers

Security headers are applied to every response: a CSP with `script-src
'self'`, `frame-ancestors 'self'`, `X-Content-Type-Options: nosniff` and a
`Referrer-Policy`.

## Input and output

- Every JSON body is capped by the body-limit middleware.
- Block content is validated against the registry schema before storage.
- All rendered HTML passes ammonia at the render boundary; plugin block
  output is sanitised again, as defence in depth.
- Search snippets are the one place a template renders unescaped text. They
  may only ever come from Tantivy's snippet generator, which escapes the
  source before inserting its own `<mark>`; the field's doc comment says so.
- Webhook payloads are signed with HMAC-SHA256 over `<timestamp>.<body>`;
  the verifier rejects timestamps outside a five-minute window and compares
  in constant time.

## Plugins

Plugins are WebAssembly components under wasmtime with fuel, memory and
epoch limits, and **every call gets its own store and instance**, so one
visitor's guest state can never reach the next. Host functions are gated
by the capability broker: a plugin reaching for something it did not
declare gets `capability-denied`, and the denial is audited.

A plugin whose hook traps or fails is marked degraded and skipped, rather
than failing the request.

What a plugin may do, and what it may not:

- **Options.** Only an allowlisted subset is readable, and a plugin cannot
  write one at all. Its own state goes in a private key/value namespace
  that no other plugin can see.
- **Posts.** `db:write:posts` creates and edits drafts; publishing needs
  `db:publish:posts` on top. Neither reaches a post the plugin did not
  create — ownership is checked against `plugin_posts` on every write, so
  a plugin can never rewrite what a person wrote.
- **Comments.** Approved rows only, without email addresses.
- **Events.** A plugin may announce a post event by id; it cannot forge a
  comment or a render request, which originate from a visitor.
- **Network.** HTTPS GET to granted hosts only, with redirects refused —
  a redirect is how a grant for one host becomes a request to another.
- **Requests it serves.** A plugin route never sees `cookie` or
  `authorization`, and its reply may set only `content-type` (from a fixed
  list), `cache-control`, and a `location` that stays on this site. A
  cross-origin redirect from a plugin would be a phishing primitive.
- **Blocks it renders.** Sanitised by the host before reaching a page, on
  top of whatever the plugin did. Authors cannot write the reserved
  attribute the renderer reads that output back out of.
- **Post types it registers.** Constrained to a slug grammar the
  `posts.type` column enforces independently, and refused for names the
  router already owns.

Front-end **assets** a plugin declares are the exception: JavaScript it
enqueues runs with the page's full privileges. That is not a new trust
boundary — installing the plugin already required a signature from a key
the operator trusts — but it is the one place the sandbox does not apply,
and it is worth knowing before granting trust to a signing key.

## Secrets

- API keys, session tokens and webhook secrets are never logged; `Secret`
  redacts in `Debug`.
- AI provider keys come from the environment and are never echoed.
- **Fixed 2026-08-28:** password-reset tokens were written to the log at
  `info` and never emailed, which put a working credential in the log file.
  They are now queued for delivery and never logged.

## Auditing

Two separate logs, for two separate questions:

- **`plugin_audit`** (migration 0017) — capability-broker decisions made by
  plugins: denials, quota hits, outbound fetches, and writes that leave the
  plugin's own storage (posts, post meta, mail). A denied read is a
  packaging mistake; a *successful* write that touches content or someone's
  inbox is the thing an operator needs to reconstruct afterwards. Writes to
  a plugin's private key/value namespace are not recorded — at the quota
  ceiling that would be 172 800 rows a day, burying the entries that
  matter.

  **Fixed 2026-08-30:** the `kind` column's constraint predated plugin
  writes and refused `'write'`, and the insert's result was discarded — so
  every write audit was silently thrown away. Migration 0028 widens the
  constraint, and a failed audit insert is now logged at `error` instead of
  ignored.
- **`audit_log`** (migration 0021) — actions taken by people that are hard
  to undo: user creation, deletion and role changes, theme activation, plugin
  install/enable/disable/delete, and option writes. Readable at
  `GET /api/v1/audit-log` with `manage_options`.

Audit writes are fire-and-forget: the action has already been authorised and
performed, so a logging failure must not turn a successful deletion into an
error the operator retries. `actor_id` is deliberately not a foreign key —
the log has to survive deletion of the account that acted, which is exactly
the case it exists to explain.

`cargo audit` runs in CI on every push and pull request
(`.github/workflows/ci.yml`), together with `cargo deny` for licence policy,
advisories and dependency sources (`deny.toml`).

## Not covered

- No external penetration test has been performed.
- No WAF integration.
- Forwarded client addresses are only as honest as the proxies listed in
  `VYASA_TRUSTED_PROXIES`; list only proxies you run.
- The rate limiter and lockout tracker are in-process: they do not survive a
  restart and are not shared between instances.
