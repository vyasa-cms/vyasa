# Deploy to Fly.io

One machine running the official image, a volume for uploads and the
search index, and Fly Postgres attached. Verified live for 0.2.0; the maintainer re-runs the checklist at the
end before each release that touches it.

```bash
cd deploy/fly
fly launch --no-deploy --copy-config --name <your-app>   # keeps this fly.toml
fly postgres create --name <your-app>-db                 # or attach an existing one
fly postgres attach <your-app>-db                        # sets DATABASE_URL
fly volumes create vyasa_data --size 1 --region <region>
fly secrets set VYASA_SECRET_KEY="$(openssl rand -hex 32)" \
    VYASA_ADMIN_EMAIL=you@example.com VYASA_ADMIN_PASSWORD='a-long-password'
fly deploy
fly scale count 1                                        # one instance per site
```

The first boot applies migrations and creates the administrator from the
two `VYASA_ADMIN_*` secrets. Sign in at `https://<your-app>.fly.dev/admin`,
then `fly secrets unset VYASA_ADMIN_PASSWORD` (once users exist the
variables are ignored anyway).

Without the `ADMIN` secrets the boot prints a setup token to the log
(`fly logs | grep stp_`) for the browser wizard at `/admin/setup`.

## Notes

- `Dockerfile` is the official image plus `entrypoint.sh`: Fly mounts the
  volume owned by root while the image serves as uid 10001, so the
  entrypoint starts as root, makes `/data/index` and `/data/media` the
  image user's, and drops to that user before `vyasa serve`.

- `fly postgres attach` sets `DATABASE_URL`; Vyasa honours it when
  `VYASA_DATABASE_URL` is unset.
- The health check is `/readyz`: Fly routes traffic only once the database
  answers and migrations are applied.
- `auto_stop_machines = "off"` and `min_machines_running = 1`: a stopped
  machine would answer the first visitor with a cold boot and an index
  rebuild. Keep one machine (`fly scale count 1`): the index is local.
- The volume holds `/data/index` and `/data/media`. For media in Tigris or
  another S3 bucket instead, set the `VYASA_STORAGE__*` secrets
  (`PROVIDER=s3`, `BUCKET`, `ENDPOINT`, `REGION`, `ACCESS_KEY_ID`,
  `SECRET_ACCESS_KEY`); the volume then holds only the index.
- Upgrade by changing the image tag in `Dockerfile` and `fly deploy`. Back
  up first (`fly postgres connect` + `pg_dump`): migrations are forward-only.

## Verification checklist (run for 0.2.0)

Launch with the steps above, sign in with the environment administrator,
upload a file, `fly machine restart <id>`, confirm `/readyz` returns 200
and search still answers.
