# Deploy to Cloudflare Containers

A Worker that fronts one Container running the official image, with media
in an R2 bucket and a hosted Postgres. Walkthrough for 0.2.0; the
maintainer runs the checklist at the end on a real account before each
release that touches it.

**You bring:** a Cloudflare account on the Workers paid plan (Containers
require it), a Postgres reachable from the internet with TLS
([Neon](https://neon.tech)'s free tier is used in the walkthrough), and an
R2 bucket with an S3 API token.

## Steps

```bash
cd deploy/cloudflare
npm install
wrangler login
wrangler r2 bucket create vyasa-media
# Dashboard → R2 → Manage API tokens → Object Read & Write on this bucket;
# note the access key id, secret access key and the S3 endpoint
# https://<account-id>.r2.cloudflarestorage.com — put the endpoint in
# wrangler.jsonc under vars.R2_ENDPOINT.
wrangler secret put DATABASE_URL          # postgres://…?sslmode=require (Neon's pooled URL)
wrangler secret put VYASA_SECRET_KEY      # openssl rand -hex 32
wrangler secret put R2_ACCESS_KEY_ID
wrangler secret put R2_SECRET_ACCESS_KEY
wrangler secret put VYASA_ADMIN_EMAIL
wrangler secret put VYASA_ADMIN_PASSWORD  # delete after the first boot
wrangler deploy                           # builds the image, pushes it, deploys
```

The first request starts the container; the boot applies migrations and
creates the administrator from the two `VYASA_ADMIN_*` secrets. Sign in at
`https://vyasa.<your-subdomain>.workers.dev/admin` (or the custom domain
you route to the Worker), then `wrangler secret delete VYASA_ADMIN_PASSWORD`.
Without those secrets the setup token is in `wrangler tail` for the
browser wizard at `/admin/setup`.

## What the files do

- `Dockerfile` is one line: the official image. Containers build from a
  Dockerfile and push the result to your account's registry.
- `src/index.ts` defines the container class — port 3000, never sleeps,
  the environment built from the secrets — and forwards every request to
  the single instance named `site`. `max_instances: 1` in `wrangler.jsonc`
  is deliberate: the search index is local to the container.
- The container's disk is ephemeral: `/tmp` holds the run directory, the
  index and media scratch, and media itself lives in R2 through the S3
  API. On every restart the index rebuilds itself from Postgres in the
  background; small and medium sites notice nothing.
- `DATABASE_URL` is a secret with the database's own TLS URL. Hyperdrive
  bindings are reachable from Workers, not from inside a container, so
  the container connects to Postgres directly; Neon's pooled endpoint is
  the right one.

## Cost and limits

A `standard` instance (1/2 vCPU, 4 GiB) running continuously is the main
line on the bill; see Cloudflare's Containers pricing. Neon's free tier
and R2's free allowance cover a small site.

## Verification checklist

Deploy with the steps above, sign in with the environment administrator,
upload a file (visible in the R2 bucket), redeploy to force a restart,
confirm `/readyz` returns 200 and search still answers.
