# Publishing to the marketplace

How a plugin or theme you built gets into the official marketplace, so
every Vyasa install can browse and install it. The short version: sign
it with your own key, open a pull request to
[vyasa-cms/marketplace](https://github.com/vyasa-cms/marketplace) adding
a listing, and the maintainers publish it on merge.

Start from a template if you have not built one yet:
[theme-starter](https://github.com/vyasa-cms/theme-starter) or
[plugin-starter](https://github.com/vyasa-cms/plugin-starter). Each is a
working project with a tutorial that ends here.

## Before you start

- A Vyasa install to test against: the [quick start](../README.md#quick-start)
  runs one in Docker in a minute. The `vyasa` binary from a release
  archive gives you the `plugin` and `theme` commands below.
- For a plugin: Rust with the `wasm32-wasip2` target
  (`rustup target add wasm32-wasip2`).

## Your author key

Every published plugin, and every theme that carries a script, is signed
by its author. Mint a key once and keep it:

```bash
vyasa plugin keygen
```

It prints a **secret** (64 hex characters) and a **public** key. Put the
secret in your password manager and never in a repository; the public
half goes in your listing as `author_key`. A leaked secret lets someone
publish under your name, so treat it like a deploy credential.

## Build and sign

**A plugin:**

```bash
cargo build --release --target wasm32-wasip2
mkdir -p pkg && cp manifest.toml pkg/ && cp target/wasm32-wasip2/release/*.wasm pkg/plugin.wasm
VYASA_SIGNING_KEY=<secret hex> vyasa plugin pack pkg --out my-plugin-0.1.0.vyplugin
```

`manifest.toml` must declare every capability the plugin uses
(`capabilities = ["log:write", "db:read:posts"]`); the host enforces that
list at runtime, and the marketplace shows it to operators before they
install.

**A theme:**

```bash
vyasa theme pack my-theme                     # a theme that is only data
VYASA_SIGNING_KEY=<secret hex> vyasa theme pack my-theme   # one with a script
```

The second form writes `my-theme-1.vytheme.sig` beside the package.

## Test it locally

Tell your test install to trust your key, then upload the package:

```bash
VYASA_PACKAGE_TRUSTED_KEYS=<public hex> vyasa serve
```

Plugins → Upload, or Appearance → Upload package (for a theme that carries
a script, select the `.sig` together with the `.vytheme` in the file
picker). Enable or activate it and use it the way
an operator would. A package that installs here installs everywhere: the
marketplace's checks are the server's own.

## Open a pull request

1. Fork `vyasa-cms/marketplace`.
2. Add one file, `listings/plugins/<name>.json` or
   `listings/themes/<name>.json`, named after the package:

   ```json
   {
     "kind": "plugin",
     "name": "my-plugin",
     "title": "My Plugin",
     "summary": "One sentence an operator reads in the list.",
     "author": "Your Name",
     "homepage": "https://github.com/you/my-plugin",
     "author_key": "<your public key, 64 hex>",
     "versions": [
       {
         "version": "0.1.0",
         "url": "https://marketplace.vyasa.site/packages/my-plugin-0.1.0.vyplugin",
         "sha256": "<filled in by tools/sign.py stamp>",
         "min_host_api": 0,
         "capabilities": ["log:write", "db:read:posts"],
         "released_at": "2026-10-04"
       }
     ]
   }
   ```

   Run `python3 tools/sign.py stamp --package my-plugin-0.1.0.vyplugin
   listings/plugins/my-plugin.json 0.1.0` to fill in `sha256` (the
   marketplace signature is added by the maintainers on merge).
3. Attach the package file to the pull request. The maintainers host it;
   you never need a server of your own.
4. Run `python3 tools/registry.py validate` before pushing. CI runs the
   same thing.

## What CI checks

The marketplace refuses exactly what a Vyasa server would refuse at
install time, so you learn about a mistake from CI rather than from an
operator's error log:

- the listing's shape: `kind` matches the directory, `name` is
  `[a-z0-9-]{1,60}` and matches the file name, `homepage` is https,
  at least one version;
- every version's `url` is on `marketplace.vyasa.site/packages/` and its
  `sha256` matches the attached package;
- a plugin's `author_key` is present and its `signature.txt` verifies
  under it; the package's `name`, `version`, `min_host_api` and
  `capabilities` match the listing exactly, and every capability parses
  in the broker's grammar (`db:read:posts`, `net:fetch:api.example.com`);
- a theme's `required_api` and `version` match its manifest;
- no stray files in the package, and size within limits.

## After the merge

The maintainers sign the package bytes with the marketplace key, publish
the package and the regenerated `index.json` to `marketplace.vyasa.site`,
and every install sees the new listing on its next browse. Nothing
auto-installs: an operator reads the capability list and chooses.

On an update, capabilities the installed version did not already hold
are marked **(new)** in the admin and named in the confirmation, so widen
them only when the plugin needs to.

## Updating and withdrawing

- **A new version:** add an entry to `versions` (newest first), attach
  the new package, open a pull request. Old versions keep their URLs —
  the bytes behind a published address never change.
- **Withdrawing a version:** open a pull request removing its entry.
  Installs that already have it keep running; it is no longer offered.
- **A new key:** add the new public key as `author_key` and sign the next
  version with it; say so in the pull request.
