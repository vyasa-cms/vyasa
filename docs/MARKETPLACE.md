# Running a marketplace

A Vyasa marketplace is a **static signed document**, not a service. One
`index.json` lists every plugin and theme; the packages live wherever you
like. Every install verifies the package by checksum and (when you sign)
by ed25519 signature, so **the host does not have to be trusted** — only
reachable. That is why hosting a marketplace costs a static file, and why
a mirror of it is exactly as safe as the original.

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

- the **author** signature inside a `.vyplugin`, checked against
  `plugin_trusted_keys`;
- the **registry** signature over the downloaded bytes, checked against
  the `registry_trusted_keys` option.

Silence is never trust in either direction. Once an install lists trusted
keys, an unsigned artifact is refused; and a signed artifact is refused
when no key is configured to check it.

And code is never installed unsigned
(`signing::verify_registry_download`): every plugin, and any theme with
`assets/theme.js` or a template that writes script, needs a registry
signature from a trusted key — on an install with no keys it is refused.
Only a theme that is pure data installs unsigned, and only over https with
a matching sha256.

## Configuring a site

Settings → *Marketplace and updates*:

- `registry_url` — https URL of the index. Empty turns the marketplace
  off entirely.
- `registry_trusted_keys` — hex ed25519 public keys allowed to sign
  packages. Options rather than config, so adding a marketplace never
  requires editing `vyasa.toml` and restarting.

Themes install straight away and stay inert until activated. Plugins
install disabled and stay inert until enabled — seeing the capability
list and then choosing is the point.

## Multiple registries and tiers

The index is a static document, so a per-tier catalogue is just a
different URL, and a private catalogue is one behind whatever auth your
CDN offers. Nothing in the format assumes a single registry.

## A reference registry

`vyasa-cms/directory` is a working one, and the shortest path to
running your own is to fork it. It carries the listing format, a
standard-library-only `tools/registry.py` that builds `index.json` from
one file per listing and cross-checks every package against its listing,
`tools/sign.py` for keys and signatures, and two workflows: validate on
every pull request, publish on merge.

Its validation deliberately mirrors what this server refuses at install
time, so a contributor learns about a mistyped capability in CI rather
than from an operator's error log.

## Hosting one from a Vyasa install

A site can serve a marketplace itself. This is the answer when the
registry's source repository is private: the index and packages must be
readable by every install, but the listings, review history and CI that
produce them need not be. GitHub release assets and Pages both require
credentials for a private repo, and an install fetching an index sends
none.

Put the files in `registry_dir` (config, default `./registry`):

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

The trade-off is that the marketplace is reachable only while that site
is. Nothing in the format or the verification depends on who serves the
bytes, so moving to a bucket later changes the URLs in the listings and
nothing else.
