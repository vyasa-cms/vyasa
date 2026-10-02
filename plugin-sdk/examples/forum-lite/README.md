# forum-lite — the reference community plugin

A forum in ~250 lines, because the CMS does most of the work. Copy this
when you build a community plugin — the point is what it does *not*
rebuild.

## What it registers

| Surface | What |
|---|---|
| `register-post-types` | `topic` — public at `/topic/{slug}`, archive at `/topic` |
| `register-sections` (filter point) | `forum/topic-list` (binds `topic`) with reply counts and a "start a topic" button |
| `register-routes` | `GET new` (the form), `POST topics` (create) |
| `register-assets` | list styling against the theme's `--vy-*` tokens |

## The composition that matters

- **Replies are the site's own comments.** A topic is an entry, so the
  existing comment form, threading, moderation queue, spam-guard filter
  and email notifications all apply without one line here. The section's
  reply counts come from `list-comments`.
- **Reading needs no account; writing does.** `POST topics` checks
  `current-viewer` — the one boundary where a plugin may honestly see the
  signed-in visitor — and creates the entry through `create-post` under
  its own `db:write:posts` / `db:publish:posts` grants.
- **The forum index is just a page.** Design it in the studio with
  `forum/topic-list`, or use the type's archive at `/topic`. Search finds
  topics because published entries of public types are indexed like
  everything else.

## Build

```sh
cargo build --release --target wasm32-wasip2 \
  && cp target/wasm32-wasip2/release/vyasa_plugin_forum_lite.wasm forum-lite-component.wasm
```
