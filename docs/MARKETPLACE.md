# The marketplace

Every Vyasa install uses the official marketplace at
<https://marketplace.vyasa.site> and trusts only the keys compiled into
its binary (`crates/api/src/official.rs`). There is nothing to configure:
Appearance → Browse themes and Plugins → Browse plugins work on a fresh
install, and so does the update check against
<https://updates.vyasa.site>.

A marketplace is a **static signed document**, not a service. One
`index.json` lists every plugin and theme with a sha256 and an ed25519
signature per version. Every install verifies the package it downloads
against both, so **the host does not have to be trusted** — only
reachable. That is why serving the marketplace costs a static file, and
why a mirror of it is exactly as safe as the original.

To publish your own plugin or theme to it, read
[PUBLISHING.md](PUBLISHING.md).

## The index

```json
{
  "schema": 1,
  "generated_at": "2026-09-04T12:00:00Z",
  "listings": [
    {
      "kind": "plugin",
      "name": "storefront",
      "title": "Storefront",
      "summary": "Products, a cart and checkout.",
      "author": "Your Team",
      "homepage": "https://example.com/storefront",
      "versions": [
        {
          "version": "1.2.0",
          "url": "https://cdn.example.com/storefront-1.2.0.vyplugin",
          "sha256": "…",
          "signature": "…",
          "min_host_api": 2,
          "capabilities": ["db:read:posts", "kv:read", "kv:write"],
          "released_at": "2026-09-01"
        }
      ]
    },
    {
      "kind": "theme",
      "name": "aurora",
      "title": "Aurora",
      "versions": [
        {
          "version": "3",
          "url": "https://cdn.example.com/aurora-3.vytheme",
          "sha256": "…",
          "required_api": 1
        }
      ]
    }
  ]
}
```

`capabilities` must list exactly what the plugin's own manifest
requests, in the colon-separated grammar the broker parses —
`db:read:posts`, `kv:write`, `net:fetch:*.stripe.com`. A capability that
does not parse is refused at install.
The admin shows that list before installing and sends it back as the
accepted set; if the package asks for anything else, the install is
refused. A catalogue edited between reading and clicking therefore cannot
grant powers nobody agreed to.

## Hosting it

Keep the source repository private if you like — listings, review and CI
history — and publish only the built artifacts:

1. A contributor opens a pull request adding or updating a listing.
2. CI validates the package: signature valid, manifest parses, declared
   capabilities match what it actually requests, size within limits,
   `min_host_api` / `required_api` present.
3. Merging publishes: CI signs the artifacts and uploads them plus the
   regenerated `index.json` to a bucket behind your CDN.

Every install pointed at that URL offers the new version on its next
check. Publishing is a pull request; there is no service to run or
secure.

Do not gate the index behind a private repository's credentials unless
you mean to: every instance would need a token, tokens shared with other
people's installs are effectively public, and the signature — not the
transport — is what makes a package trustworthy.

## Signing

Reuse the plugin signing tooling:

```bash
vyasa plugin keygen                 # prints a keypair
vyasa plugin pack ./my-plugin --key <hex>   # signs the package itself
```

Two independent signatures end up protecting a marketplace install:

- the **marketplace** signature over the downloaded bytes, checked
  against the keys compiled into the binary — the trust anchor;
- the **author** signature inside a `.vyplugin`, checked against the
  `author_key` the listing names (or the marketplace keys when it names
  none), so a listing cannot be re-pointed at someone else's package.

Silence is never trust: an unsigned package that carries code is refused
whatever the index says.

And code is never installed unsigned
(`signing::verify_registry_download`): every plugin, and any theme with
`assets/theme.js` or a template that writes script, needs a registry
signature from a trusted key — on an install with no keys it is refused.
Only a theme that is pure data installs unsigned, and only over https with
a matching sha256.

## Operators: mirrors and switches

Where packages and releases come from is not an administrator's setting
— whoever could change it could hand the site any binary or plugin they
signed. Only the server's own configuration can, and only in two ways:

```toml
[marketplace]
enabled = true          # false: no browsing, no marketplace installs
mirror_url = ""         # another https address for the same signed index

[updates]
enabled = true          # false: no update checks
mirror_url = ""         # another https address for the same signed manifest

# Keys trusted for packages uploaded by hand (plugins, and themes that
# carry a script). `plugin_trusted_keys` still works as an alias.
package_trusted_keys = []
```

or, as environment variables, `VYASA_MARKETPLACE__MIRROR_URL`,
`VYASA_MARKETPLACE__ENABLED`, `VYASA_UPDATES__MIRROR_URL`,
`VYASA_UPDATES__ENABLED` and `VYASA_PACKAGE_TRUSTED_KEYS` (comma-separated).

A mirror changes only where bytes come from. The keys stay the official
ones, so a mirror can serve nothing those keys did not sign; at worst it
can hide versions or serve an older copy. Settings → *Marketplace and
updates* shows which of the three states each source is in.

Themes install straight away and stay inert until activated. Plugins
install disabled and stay inert until enabled — seeing the capability
list and then choosing is the point.

## The marketplace repository

`vyasa-cms/marketplace` holds the listings, one file per plugin or
theme, and the tools that build and check them: `tools/registry.py`
builds `index.json` and cross-checks every package against its listing
(including the author signature against the listing's `author_key`),
`tools/sign.py` handles keys and signatures, `tools/build-site.sh`
assembles the static site that `marketplace.vyasa.site` serves.

Its validation deliberately mirrors what this server refuses at install
time, so a contributor learns about a mistyped capability in CI rather
than from an operator's error log.

## Running a mirror from a Vyasa install

A site can serve a copy of the marketplace itself — for an air-gapped
network, or to keep installs working when the internet does not. Copy
`site/dist` from the marketplace repository (or sync
`marketplace.vyasa.site`) into `registry_dir` (config, default
`./registry`):

```text
registry/index.json              the catalogue
registry/packages/*.vyplugin     the packages it points at
registry/packages/*.vytheme
```

and the site serves exactly two paths:

```text
GET /registry/index.json
GET /registry/packages/{file}
```

Both 404 until those files exist, so an install that hosts no
marketplace gains no public surface. Package filenames are checked
against an allowlist — lowercase ASCII, digits, dot, dash, underscore,
ending in a known extension — rather than scanned for traversal, which
excludes path separators by construction. Symlinks are refused.

The index is cached for a minute; packages are immutable, because the
bytes behind a published URL must never change. Publish new bytes as a
new version with a new filename.

Then point the other installs at it with
`[marketplace] mirror_url = "https://that-site/registry/index.json"`.
The package URLs inside the index still name `marketplace.vyasa.site`;
rewrite them to the mirror when the mirror must be the only host reached.
