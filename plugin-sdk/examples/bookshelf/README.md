# bookshelf — every extension point, once

A small plugin that uses the whole `vyasa-plugin-v2` contract, so each
feature has a reference implementation you can read in one sitting:

| What it does | How |
|---|---|
| Adds a **Books** content type at `/book/…` with an archive | `register-post-types` |
| Adds a **Book rating** block to the editor | `register-blocks` + `render-block` |
| Serves `GET /api/v1/plugin/bookshelf/stats` | `register-routes` + `handle-request` |
| Counts published books every five minutes | `register-schedule` + `run-task` |
| Offers a **Star glyph** setting in the admin | `register-admin`, read back at `init` |
| Styles its block on every page | `register-assets` |
| Notes the count in an HTML comment | `filter` at `page-html` |
| Remembers the count and the chosen glyph | `kv-set` / `kv-get` |
| Adds a **Shelves** taxonomy at `/shelf/…` | `register-taxonomies` |
| Greets a signed-in reader in post titles | `filter-at` + `current-viewer` |
| Rewrites "novel" to "book" in searches | `filter-at` at `search-query` |
| Stamps every saved post with a note | `on-event` + `set-post-meta` |
| Emails the site owner about new accounts | `on-event` + `send-mail` |

Its capabilities are exactly what those need and nothing more:
`log:write`, `db:read:posts`, `db:read:options`, `db:write:meta`,
`kv:read`, `kv:write`, `viewer:read`, `mail:send`, and `html:page` for
the book-count comment its `page-html` filter appends. It never writes a post,
never reaches the network and never reads a comment, so it never asks for
those.

## Three things worth copying

**A plugin has no memory between calls.** Every call runs in a fresh
instance with a fresh store — that is what makes plugin calls concurrent
and keeps one visitor's request out of the next. A `static mut` would be
reset before you could read it. Anything that has to outlive a call goes
in `kv`, which is why `init` writes the star glyph there and
`render_block` reads it back.

**Escaping is still yours.** The host sanitizes what `render-block`
returns, but sanitizing removes script — it does not fix markup. Values
that came out of a document go through `escape` before they go into a
tag.

**A filter point you do not handle must return its payload unchanged.**
`filter_at` here matches two points and returns `payload` for everything
else. Every plugin sees every point, so anything else would mean the last
plugin installed silently wins.

## Build and install

```bash
cargo build --release --target wasm32-wasip2

mkdir -p pkg && cp target/wasm32-wasip2/release/vyasa_plugin_bookshelf.wasm pkg/plugin.wasm
cp manifest.toml pkg/
VYASA_SIGNING_KEY=<secret hex> vyasa plugin pack pkg --out bookshelf.vyplugin
vyasa plugin install bookshelf.vyplugin --enable
# then restart the server: declarations are collected at boot
```

`bookshelf-component.wasm` is the built artifact, committed so
`crates/plugins/tests/v2_contract_tests.rs` and
`crates/api/tests/plugin_integration.rs` run without a wasm toolchain.
Rebuild it whenever you change `src/lib.rs`:

```bash
cargo build --release --target wasm32-wasip2 \
  && cp target/wasm32-wasip2/release/vyasa_plugin_bookshelf.wasm bookshelf-component.wasm
```
