# Deploy to Railway

A template following the container contract in
[docs/DEPLOYMENT.md](../../docs/DEPLOYMENT.md#container-platforms).
Documented, not account-verified in 0.2.

1. New project → **Deploy from GitHub repo**, root directory `deploy/railway`
   (or copy this directory into your own repository). Railway builds the
   `Dockerfile`: the official image plus `entrypoint.sh`, which takes
   ownership of the volume (Railway mounts it as root) and drops to the
   image's user before serving.
2. Add a **PostgreSQL** service. Railway sets `DATABASE_URL` on the app
   service when you reference it: in the app's Variables, add
   `DATABASE_URL=${{Postgres.DATABASE_URL}}`.
3. Add a **Volume** mounted at `/data`.
4. Variables on the app service:

   | Name | Value |
   |---|---|
   | `VYASA_RUN_DIR` | `/tmp/vyasa-run` |
   | `VYASA_INDEX_DIR` | `/data/index` |
   | `VYASA_MEDIA_DIR` | `/data/media` |
   | `VYASA_LOG__FORMAT` | `json` |
   | `VYASA_SECRET_KEY` | `openssl rand -hex 32` |
   | `VYASA_ADMIN_EMAIL` | your address |
   | `VYASA_ADMIN_PASSWORD` | a long password; remove after the first boot |

   Railway sets `PORT` itself; Vyasa binds to it.
5. Deploy. `railway.json` sets the health check to `/readyz` and one
   replica; keep it at one — the search index is local to the instance.

Sign in at `/admin`; without the `ADMIN` variables the setup token is in
the deploy log (search for `stp_`). Upgrade by changing the tag in
`Dockerfile`; back up the database first.
