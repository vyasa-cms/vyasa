# The update channel

`https://updates.vyasa.site/stable.json` is the release manifest every
Vyasa install checks: which stable versions exist, where their tarballs
are, and the ed25519 signature (by the release key compiled into the
binary, `crates/api/src/official.rs`) that makes each tarball trusted.
Release candidates never appear here.

## Publishing

`scripts/release-manifest.py` adds a release to `dist/stable.json`;
`npx wrangler deploy` in this directory publishes it. The `channel` job
in `.github/workflows/release.yml` does both for every stable tag; it
needs the repository secrets `RELEASE_KEY` (the signing key),
`CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` (the same values the
website repository uses). While Actions is unavailable, run from a
maintainer's machine:

```bash
python3 scripts/release-manifest.py --version 0.1.0 --dist dist \
  --key-file ~/.config/vyasa/release-key \
  --current deploy/updates/dist/stable.json --out deploy/updates/dist/stable.json
(cd deploy/updates && npx wrangler deploy)
```
