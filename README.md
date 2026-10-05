<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/banner.png">
  <source media="(prefers-color-scheme: light)" srcset="assets/banner-light.png">
  <img src="assets/banner.png" alt="Vyasa — a content management system, written in Rust" width="100%">
</picture>

<p align="center"><strong>Vyasa is an AI-native content management system — a fast WordPress alternative written in Rust.</strong></p>

<p align="center">
  <a href="https://github.com/vyasa-cms/vyasa/actions/workflows/ci.yml"><img src="https://github.com/vyasa-cms/vyasa/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="#licence"><img src="https://img.shields.io/badge/licence-MIT%20OR%20Apache--2.0-blue" alt="Licence: MIT OR Apache-2.0"></a>
  <a href="https://github.com/vyasa-cms/vyasa/releases"><img src="https://img.shields.io/github/v/release/vyasa-cms/vyasa?include_prereleases" alt="Latest release"></a>
</p>

<p align="center">
  <a href="https://vyasa.site">Website</a> ·
  <a href="https://vyasa.site/docs">Docs</a> ·
  <a href="https://demo.vyasa.site">Live demo</a> ·
  <a href="https://github.com/vyasa-cms/vyasa/discussions">Discussions</a>
</p>

---

Content is stored as structured blocks rather than markup, themes are data
rather than code, and plugins run in a WebAssembly sandbox with only the
capabilities they declare. One server binary and PostgreSQL are all it needs.

> **व्यास** — *vyāsa*, from *vi* + √*as*, "to divide, to arrange". Veda Vyāsa
> earned the name by dividing the one Veda into four collections; the word is a
> title, not a personal name. In Sanskrit grammar *vyāsa* is also the term for
> analysis — decomposing a compound into its parts — against *samāsa*, which
> composes them back. That is what an editor does in each direction.

## Why Vyasa

- **AI built in, not bolted on** — writing assistance in the editor, an AI
  theme studio that designs a site from a description, semantic search and
  related posts, and an [MCP server](docs/MCP.md) so AI agents can manage
  content under the same permissions as a person. Bring your own Anthropic
  or OpenAI-compatible provider.
- **Block editor** — a writing canvas built on ProseMirror, plus a classic
  field editor. Both read and write the same block document, so a post opens
  in either.
- **Your content model** — create content types and typed fields from the
  admin (text, number, date, choice, URL, media, links to other entries),
  with per-type templates and archives.
- **Themes as data** — a validated set of design tokens compiled to CSS, with
  sandboxed templates. A theme can never run code on your server.
- **Sandboxed plugins** — WebAssembly components with declared capabilities,
  checked by a broker on every call. A plugin cannot read what it was not
  granted.
- **Users and roles** — built-in and custom roles, optional public
  registration with email confirmation, two-factor sign-in, API keys.
- **APIs first** — an OpenAPI-documented REST API and a GraphQL API with
  subscriptions, both behind the same permission checks.

## Quick start

With Docker:

```bash
mkdir vyasa && cd vyasa
curl -fsSLO https://raw.githubusercontent.com/vyasa-cms/vyasa/main/docker-compose.yml
docker compose run --rm app migrate          # create the database schema
docker compose up -d
docker compose exec app cat .run/setup-token # the one-time setup token
```

Open <http://localhost:3000/admin/setup>, enter the token, and create the
first administrator. The site is on <http://localhost:3000>, the admin on
`/admin`. The compose file follows the latest release; pin a version with
`VYASA_VERSION=0.1.0` in front of the commands (or in an `.env` file next
to `docker-compose.yml`).

Without Docker, the installer fetches the release for your platform,
verifies it and puts `vyasa` on your `PATH`; it needs a PostgreSQL
database to run against:

```bash
curl -fsSL https://vyasa.site/install.sh | sh
cd ~/vyasa && VYASA_DATABASE_URL=postgres://vyasa:PASSWORD@localhost:5432/vyasa vyasa migrate && vyasa serve
```

Building from source and production deployment are covered in
[CONTRIBUTING.md](CONTRIBUTING.md) and [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md).
Configuration comes from `vyasa.toml` if present, overridden by `VYASA_*`
environment variables — `__` separates nesting, so `VYASA_LOG__LEVEL=debug`.

## Documentation

| Topic | Where |
|---|---|
| How it fits together | [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) |
| Building themes | [docs/THEMES.md](docs/THEMES.md) |
| Writing plugins | [docs/plugin-api.md](docs/plugin-api.md), [plugin-sdk/](plugin-sdk) |
| Publishing to the marketplace | [docs/PUBLISHING.md](docs/PUBLISHING.md) |
| AI agents over MCP | [docs/MCP.md](docs/MCP.md) |
| Deploying and running | [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md), [docs/OPERATIONS.md](docs/OPERATIONS.md) |
| Security model | [docs/SECURITY.md](docs/SECURITY.md) |
| Who may call which API route | [docs/ROUTE-ACCESS.md](docs/ROUTE-ACCESS.md) |
| Versions and upgrades | [docs/VERSIONING.md](docs/VERSIONING.md) |

## Project status

Pre-1.0: usable and running in production, but APIs may still change between
minor versions — see [docs/VERSIONING.md](docs/VERSIONING.md) and the
[changelog](CHANGELOG.md). Back up your database before upgrading;
migrations are forward-only.

## Plugins and themes

Every install browses the official marketplace out of the box — Plugins
→ Browse plugins, Appearance → Browse themes. To publish yours, start
from the [theme-starter](https://github.com/vyasa-cms/theme-starter) or
[plugin-starter](https://github.com/vyasa-cms/plugin-starter) template
and follow [docs/PUBLISHING.md](docs/PUBLISHING.md).

## Contributing

Bug reports, docs, plugins, themes and code are all welcome. Start with
[CONTRIBUTING.md](CONTRIBUTING.md) and the
[`good first issue`](https://github.com/vyasa-cms/vyasa/labels/good%20first%20issue)
label, and please follow the [Code of Conduct](CODE_OF_CONDUCT.md). Report
security issues privately as described in [SECURITY.md](SECURITY.md).

## Licence

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
