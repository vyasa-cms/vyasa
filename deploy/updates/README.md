# The update channel

`https://updates.vyasa.site/stable.json` is the release manifest every
Vyasa install checks: which stable versions exist, where their tarballs
are, and the ed25519 signature (by the release key compiled into the
binary, `crates/api/src/official.rs`) that makes each tarball trusted.
Release candidates never appear here.

## Publishing

`scripts/release-manifest.py` adds a release to `dist/stable.json`;
`npx wrangler deploy` in this directory publishes it. The release
workflow does both for every stable tag (secrets `RELEASE_KEY`,
`CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_ACCOUNT_ID`); until then it is run
from a maintainer's machine.
