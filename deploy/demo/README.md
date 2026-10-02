# The public demo

A Vyasa install in demo mode that anyone can sign in to, reset every hour.
It runs from the published image with its own Postgres, capped at about
1.5 GB of memory in total, and listens on `127.0.0.1:38400` only.

## What demo mode changes

`VYASA_DEMO__ENABLED=true` makes the install a sandbox:

- The sign-in page shows the shared account (`DEMO_EMAIL` /
  `DEMO_PASSWORD`, default `demo@vyasa.site` / `vyasademo`), and every
  page carries a banner saying it is a demo that resets.
- Visitors are administrators and can write, edit, upload (2 MiB per file),
  build content types, menus and themes.
- Refused with "… is disabled in the demo": installing or uploading plugins
  and theme packages, marketplace installs, updates, AI provider keys, the
  mail relay, webhooks, link checks, the site-address check, imports and
  exports, API keys, two-factor sign-in, the security-sensitive options,
  re-running setup, granting account management (directly or through a
  custom role), and anything that would lock the next visitor out of the
  shared account.
- Background work that reaches other servers on its own — webhook delivery,
  the hourly link-check sweep, IndexNow pings — does not start.
- `X-Robots-Tag: noindex, nofollow` on every response and a `robots.txt`
  that disallows everything, so nothing in the sandbox reaches search
  results.

The decisions live in `crates/api/src/policy.rs` (`DEMO_REFUSED_ROUTES` and
the `demo_*` functions); `crates/api/tests/demo_mode.rs` checks them.

## Running it

```bash
cd deploy/demo
./seed.sh              # once: database + sample content -> seed/
./reset.sh             # starts the demo from the seed
```

`seed.sh` needs the image to be pullable (`docker login ghcr.io` while the
repository is private). Re-run it after moving the demo to a new version.

## Limits

- The app container is read-only apart from in-memory volumes: uploads
  (256 MiB), the search index (128 MiB) and scratch space. A reboot clears
  uploads until the next reset restores them.
- Memory is capped at 1 GiB for the app and 512 MiB for Postgres; the
  database itself is not size-capped, so watch the host's disk.
- The network has a fixed subnet (`172.31.240.0/24`) and only its gateway
  is trusted to report visitors' addresses. If that subnet clashes with
  another network on the host, change both the subnet and
  `VYASA_TRUSTED_PROXIES`.

## Accepted risks

Anyone can publish text and images on the demo for up to an hour. Search
engines are told to stay away, but direct links work. Put Cloudflare's bot
protection and rate limits in front of it, and keep the reset hourly or
shorter.

## Hourly reset

Install a timer that runs `reset.sh` every hour, for example a cron entry:

```cron
0 * * * * /path/to/vyasa/deploy/demo/reset.sh >> /var/log/vyasa-demo-reset.log 2>&1
```

or a systemd timer. Both containers restart on boot (`restart:
unless-stopped`).

## Publishing it

The demo listens on loopback; publish it through a tunnel or proxy.
With Cloudflare Tunnel, add an ingress rule before the catch-all:

```yaml
- hostname: demo.vyasa.site
  service: http://127.0.0.1:38400
```

then `cloudflared tunnel route dns <tunnel> demo.vyasa.site` and restart
cloudflared. Recommended on Cloudflare: a rate-limiting rule for
`demo.vyasa.site/api/v1/auth/*` and for non-GET requests, and an uptime
check on `https://demo.vyasa.site/admin/login`.

## Moving it to another machine

Copy this directory (with `seed/`) and run `./reset.sh` there; point the
hostname at the new machine.
