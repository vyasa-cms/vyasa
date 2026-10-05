# Deployment

Vyasa is a single binary plus a Postgres database. It serves the public
site, the REST and GraphQL APIs, and the admin SPA from one process.

## What the process needs

| | |
|---|---|
| Postgres | 15 or newer, with the `citext` extension available |
| Disk | A writable directory for uploads and one for the search index |
| Files | The built admin bundle at `admin/dist`, and `themes-starter/` for first-boot themes |
| Network | One inbound port; outbound only if AI, webhooks or SMTP are used |

The binary resolves `admin/dist` and `themes-starter/` **relative to its
working directory**, so run it from the directory that contains them.

## Configuration

Everything is set through `vyasa.toml` or environment variables; the
environment wins. `.env.example` lists every key with its default.

The one required value is `VYASA_DATABASE_URL`. Set `VYASA_SECRET_KEY`
(any long random string, e.g. `openssl rand -hex 32`) in production: it
keeps preview links valid across restarts and seals the AI provider keys
saved on the Models page. Two more matter in
production:

- **`site_url`** — set it on the Settings page. Feeds and sitemaps fall
  back to the request's `Host` header without it. Password-reset and invite
  emails do **not**: they refuse to send until `site_url` is set, because a
  link built from a client-supplied `Host` hands the reset token to whoever
  forged the header.
- **`VYASA_SMTP__*`** — with no relay configured, password-reset links and
  comment notifications are written to the log instead of being sent. That
  is the intended development default and a broken production install.

Public registration (phase 98, the `registration_enabled` option) needs
**both a working mail relay and `site_url`**: with either missing, every
`POST /auth/register` answers `503 registration_unavailable` and creates
nothing. Site health warns when registration is on but either is not
configured.

## First run

```bash
vyasa migrate                                   # idempotent
vyasa admin create --email you@example.com   # prompts for a password
vyasa serve
```

`migrate` is deliberately a separate command rather than something `serve`
does at boot: two instances starting together would otherwise race to
migrate the same database.

## Behind a reverse proxy

By default the client address used for rate limiting and login lockout is
the TCP peer, and forwarded headers are ignored. Behind a proxy that makes
every visitor look like the proxy, so tell the server which peers to trust:

- `VYASA_TRUSTED_PROXIES` — comma-separated addresses or CIDR blocks. Only
  when the peer is one of these is `X-Forwarded-For` read, and then the
  rightmost entry that is not itself a trusted proxy is the client (the
  leftmost entries are whatever the client sent).
- `VYASA_TRUST_CF_CONNECTING_IP=true` — additionally honour
  `CF-Connecting-IP` from a trusted peer. Only for Cloudflare, which
  overwrites that header.

`X-Forwarded-Proto` still decides the scheme in generated absolute URLs;
without it, a TLS-terminating proxy produces `http://` links.

**IPv6 clients are counted by their /64.** Every rate-limit bucket and the
sign-in lockout key an IPv6 client by the /64 it is in (`2001:db8:1:2::/64`),
not by the full address: one subscriber is normally given a whole /64 and
can rotate through it at will. IPv4 is counted by the address, and an
IPv4-mapped IPv6 address (`::ffff:203.0.113.9`) as the IPv4 address. Two
visitors who share a /64 share its buckets. This applies to whatever address
the rules above settle on, so behind a proxy it matters that the proxy is
trusted: otherwise every visitor is the proxy's own address.

Registration's own rate limits (per client on `/auth/register` and
`/auth/register/resend`, per email address on registration mail, and per
email address on `/auth/forgot`) are in-process buckets like the rest of
rate limiting, so they are **per node** and not shared across instances;
only the per-account count of mailed links (confirmation and password-reset
alike) is read from Postgres and so holds across nodes and restarts. See
"Registration limits" in `docs/SECURITY.md`.

### Cloudflare Tunnel

cloudflared connects from localhost, so a tunnel deployment needs
`VYASA_TRUSTED_PROXIES=127.0.0.1,::1` and `VYASA_TRUST_CF_CONNECTING_IP=true`.
Without them every visitor shares one rate-limit bucket.

## Docker

```bash
docker compose run --rm app migrate
docker compose up -d
```

Migrations are not run from the entrypoint, for the reason above. Run them
once before the first `up`, and again after upgrading the image.

Uploads and the index are on named volumes: an image upgrade that did not
preserve them would silently discard every uploaded file.

### Image tags and verification

`docker-compose.yml` runs `ghcr.io/vyasa-cms/vyasa`. Tags:

| Tag | Moves when |
|---|---|
| `latest` | every stable release |
| `0.1` | every `0.1.x` release |
| `0.1.0` | never (one release) |
| `0.1.0-rc.1` | never (a release candidate; candidates never move `latest`) |

Pin a version with `VYASA_VERSION=0.1.0 docker compose up -d`. Back up the
database before moving to a newer version: migrations are forward-only.

Every image and release archive published from the public repository carries a
build provenance attestation:

```bash
gh attestation verify oci://ghcr.io/vyasa-cms/vyasa:0.1.0 --owner vyasa-cms
gh attestation verify vyasa-0.1.0-x86_64-unknown-linux-gnu.tar.gz --owner vyasa-cms
```

## Release archives

Each release has an archive per platform (Linux x86_64 and arm64, macOS
arm64) with a `.sha256` next to it. The Linux binaries need glibc 2.34 or
newer and the system CA certificates (`ca-certificates` on Debian and
Ubuntu — minimal container images may not have it). Unpack it and run the server from that
directory — it serves the admin from `admin/dist` beside the binary:

```bash
sha256sum -c vyasa-0.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256
tar -xzf vyasa-0.1.0-x86_64-unknown-linux-gnu.tar.gz
cd vyasa-0.1.0-x86_64-unknown-linux-gnu
export VYASA_DATABASE_URL=postgres://vyasa:…@localhost:5432/vyasa
./vyasa migrate && ./vyasa serve
```

The image is debian-slim rather than scratch/musl. wasmtime and Tantivy both
expect a real libc, and a static build of them costs more maintenance than
the image size saves.

## Upgrading

Vyasa knows when it is behind, checks whether *this* install can take the
upgrade, and performs it where doing so is safe. What "safe" means
depends on how it is deployed.

| Deployment | Detects | Applies |
|---|---|---|
| Binary (standalone or systemd) | yes | yes — `vyasa update apply`, or the button on Site health |
| Docker / Compose | yes | no — shows the commands; the image is the unit of upgrade |
| Kubernetes | yes | no — your rollout owns it; `GET /api/v1/updates` drives your tooling |

A container cannot replace its own image, so the admin never offers a
button there. Mounting the Docker socket into the CMS to work around
that would hand a web-facing process root on the host; use your CD
pipeline or Watchtower instead.

### What is and is not at risk

Themes and plugins are **rows in Postgres** — tokens, layouts and
templates as JSONB, plugin wasm as bytes with its own version history.
An upgrade cannot clobber them. Starter themes are compiled into the
binary, so an edited theme is never overwritten either.

The state that *can* be lost is on disk:

- `media/` — uploads and their derivatives. In Docker this must be a
  volume; the preflight warns when it is on the container filesystem.
- `index/` — the search index, which is derived and can be rebuilt with
  `vyasa search reindex`.

Neither should live inside the directory a release is extracted over.
The preflight checks that too.

### The channel

Update checks read <https://updates.vyasa.site/stable.json>, and every
tarball is verified against the release keys compiled into the binary;
nothing to set up. Container installs are told which image to pull rather
than swapping the binary. To mirror the manifest or turn checks off, see
`[updates]` in [MARKETPLACE.md](MARKETPLACE.md#operators-mirrors-and-switches).

### Check, rehearse, apply

```bash
vyasa update check                 # version, availability, full preflight
                                   # exit 10 when an upgrade is available
vyasa migrate --plan               # exactly which migrations would run
scripts/upgrade-test.sh            # rehearse them against a copy of real data
vyasa update verify --to 1.2.0     # preflight one specific release
vyasa update apply --to 1.2.0      # download, verify, back up, swap, migrate, restart
```

`apply` does, in this order: download and checksum, verify the signature,
unpack, **dump the database**, swap the binary (keeping the old one) and
the admin bundle together, migrate, restart, and wait for the new build
to answer. Everything before the swap leaves the install untouched on
failure.

### Rolling back

```bash
vyasa update rollback              # restores the kept binary
```

This is safe **only while the new build has not served anything** — which
is exactly when the updater does it automatically, if the new build fails
its health check. After that, the pre-upgrade dump is the way back:
migrations are forward-only, and an older binary cannot read theme
documents a newer one has written.

For Docker:

```bash
docker compose pull
docker compose run --rm app migrate
docker compose up -d
```

## Operating

`docs/OPERATIONS.md` covers metrics, logs and the health checks.
`docs/PERFORMANCE.md` has the benchmark baseline and the load-test harness.
