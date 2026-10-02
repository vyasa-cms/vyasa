# footer-note

A complete, installable Vyasa plugin. It appends a note before `</body>`
on every rendered page and asks the host how many posts the site has, so
building and installing it exercises the whole path: the component
contract, the capability broker, the `page-html` filter and the host
data calls.

```bash
cargo build --release --target wasm32-wasip2
wasm-tools validate target/wasm32-wasip2/release/vyasa_plugin_footer_note.wasm
```

Then package it with `vyasa plugin keygen` / `vyasa plugin pack` as
described in `../../README.md`.

Capabilities requested: `db:read:posts` (for the count), `log:write`, and
`html:page` — without it the host never calls the `page-html` filter.
