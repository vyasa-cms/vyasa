# vyasa.site

The project website and documentation, built with [Fumadocs](https://fumadocs.dev) as a static export.

- Docs pages under `content/docs/` are hand-written (`index.mdx`, `getting-started.mdx`) or generated: `pnpm sync:docs` copies the repository's `docs/*.md` and `CONTRIBUTING.md` in, and `pnpm gen:api` builds the REST reference from `admin/openapi.json`. Edit the originals, not the generated copies.
- `pnpm dev` runs both and starts a dev server; `pnpm build` writes the static site to `out/`.

Deployed to Cloudflare Pages from `main`: root directory `website`, build command `pnpm build`, output directory `out`, environment `NODE_VERSION=22` (pnpm comes from `packageManager` in `package.json`; also `.node-version`).
