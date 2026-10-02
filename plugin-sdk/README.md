# Vyasa Plugin SDK

- [`template/`](./template) — cargo-ready starter crate implementing the
  `vyasa-plugin-v2` world with every export stubbed out.
- [`examples/bookshelf/`](./examples/bookshelf) — a worked example using
  every extension point: a custom post type, a custom block, a public
  route, a scheduled task, a settings form and a stylesheet.
- [`examples/footer-note/`](./examples/footer-note) — the smallest useful
  plugin: one filter and one host call, on the base world.
- [`examples/hello/`](./examples/hello) — prebuilt component artifact.
- Contract reference: [`docs/plugin-api.md`](../docs/plugin-api.md).
- WIT definitions: [`crates/plugins/wit/host.wit`](../crates/plugins/wit/host.wit).

## Build a plugin

```bash
rustup target add wasm32-wasip2
cp -r template my-plugin && cd my-plugin
# edit src/lib.rs …
cargo build --release --target wasm32-wasip2
# artifact: target/wasm32-wasip2/release/vyasa_plugin_template.wasm
wasm-tools validate <that file>   # optional sanity check
```

## Binding-generation decision (recorded)

Guests use the **`wit-bindgen` macro directly against the checked-in WIT**
(`path = "../../crates/plugins/wit"`); no generated bindings are committed.
The host side will use `wasmtime::component::bindgen!` against the same file
(phase 31). Single source of truth: `host.wit`; zero generated-code drift.

## Signing and installing

A `.vyplugin` is a zip of `manifest.toml`, `plugin.wasm` and
`signature.txt`. The server only accepts packages signed by a key listed
in `plugin_trusted_keys`, so mint one first:

```bash
vyasa plugin keygen
# -> secret hex (sign with it) and public hex (trust it)
export VYASA_PLUGIN_TRUSTED_KEYS=<public hex>   # then restart the server
```

Then pack:

```bash
mkdir pkg
cp target/wasm32-wasip2/release/<crate>.wasm pkg/plugin.wasm
cat > pkg/manifest.toml <<'TOML'
name = "my-plugin"
version = "0.1.0"
min_host_api = 1
capabilities = ["db:read:posts"]
TOML
VYASA_SIGNING_KEY=<secret hex> vyasa plugin pack pkg --out my-plugin.vyplugin
```

`./sign.sh pkg my-plugin.vyplugin` does the same thing, using whichever
`vyasa` binary it can find.

Install through Admin → Plugins (or `POST /api/v1/plugins`) and enable
it. No restart: the server re-reads everything the plugin declares when
you enable, disable, upgrade, roll back or change a setting.

`vyasa plugin install` writes to the database from a separate process, so
a server that is already running will not notice until it restarts.

A complete worked example lives in `examples/bookshelf/`.

## Two worlds

`host.wit` defines two worlds, and a plugin implements exactly one:

| | `vyasa-plugin` | `vyasa-plugin-v2` |
|---|---|---|
| `init`, `filter`, `action` | yes | yes |
| `register-blocks`, `register-routes` | yes | yes |
| `handle-request` — serve a declared route | — | yes |
| `render-block` — render a declared block | — | yes |
| `run-task` + `register-schedule` — periodic work | — | yes |
| `register-admin` — a settings form | — | yes |
| `register-assets` — CSS/JS on every page | — | yes |
| `register-post-types` — custom post types | — | yes |
| `register-taxonomies` — custom taxonomies | — | yes |
| `filter-at` — the named filter points | — | yes |
| `on-event` — the named events | — | yes |

v2 is a strict superset, so target it unless you have a reason not to.
The host detects which world a component implements when it loads it: a
plugin built against the base world keeps working untouched, and every v2
entry point simply reports that it declares none of that.

The **host imports** in the `host` interface only ever grow, and a
component imports just the ones it calls, so adding host functions never
invalidates a plugin already in the field. That is why storage, post
writes and comment reads arrived in `vyasa:plugin@1.0.0` rather than in a
new interface version.

## Capabilities

Declared in `manifest.toml`, shown to the operator at install, and checked
by the broker on every call.

| Capability | Grants |
|---|---|
| `log:write` | `log` |
| `db:read:posts` | `get-post`, `query-posts` |
| `db:read:options` | `get-option` beyond the four public ones |
| `db:read:comments` | `list-comments` (approved only, no emails) |
| `db:write:posts` | `create-post`, `update-post` — **drafts, and only rows this plugin created** |
| `db:publish:posts` | on top of the above, may set `published` |
| `kv:read` / `kv:write` | `kv-get`/`kv-list`, `kv-set`/`kv-delete` in the plugin's own namespace (write implies read) |
| `net:fetch:<host>` | `fetch`, HTTPS GET to matching hosts (`*.example.com` matches one label) |
| `db:write:meta` | `set-post-meta`, in the plugin's own namespace on any post |
| `viewer:read` | `current-viewer` — id, name and role of whoever is signed in |
| `mail:send` | `send-mail`, to a registered user or the `site-admin` token |
| `event:emit` | `emit-event` for post events, by id |
| `ai:text` | `ai-complete` through the site's default text model (20 calls/minute; refused inside `page-html`) |
| `html:page` | being called at the `page-html` filter stage — rewriting every public page |
| `assets:script` | having the `js` from `register-assets` linked from every public page (CSS needs nothing) |

Reads are quota'd per minute, as are fetches, fetched bytes, writes and
`ai-complete` calls. `html:page` and `assets:script` are new: a plugin
that filters `page-html` or ships JS must declare them, and one installed
before they existed loses those powers until it is reinstalled from a
package that does.
Every write and every denial lands in `plugin_audit`.
