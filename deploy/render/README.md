# Deploy to Render

A Blueprint following the container contract in
[docs/DEPLOYMENT.md](../../docs/DEPLOYMENT.md#container-platforms).
Documented, not account-verified in 0.2.

1. Copy this directory (`render.yaml`, `Dockerfile`, `entrypoint.sh`) to
   the root of a repository you own and create a **Blueprint** from it.
   The Dockerfile is the official image plus an entrypoint that takes
   ownership of the disk (Render mounts it as root) and drops to the
   image's user before serving.
2. Render asks for the two `sync: false` values: `VYASA_ADMIN_EMAIL` and
   `VYASA_ADMIN_PASSWORD`. They create the first administrator on the
   first boot; clear the password variable afterwards.
3. Deploy. Render sets `PORT` and, from the `databases` entry,
   `DATABASE_URL` (internal connection string). The health check is
   `/readyz`; traffic arrives once migrations are applied.

`numInstances: 1` is deliberate: the search index is on the disk at
`/data`, local to the instance. Media goes on the same disk; for an S3
bucket instead, add the `VYASA_STORAGE__*` variables (`PROVIDER=s3`,
`BUCKET`, `ENDPOINT`, `REGION`, `ACCESS_KEY_ID`, `SECRET_ACCESS_KEY`).

Without the `ADMIN` variables the setup token is in the service log
(search for `stp_`). Upgrade by changing the image tag in `Dockerfile`; back up the
database first — migrations are forward-only.
