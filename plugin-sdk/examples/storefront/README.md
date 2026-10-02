# storefront — the reference commerce plugin

A small, honest store: products, a designed store page, a signed-in cart,
and order requests by mail. Copy this when you build a commerce plugin —
every mechanism it uses is public contract, and the host knows nothing
about shops.

## What it registers

| Surface | What |
|---|---|
| `register-post-types` | `product` — edited like any entry, public at `/product/{slug}` with an archive at `/product` |
| `register-taxonomies` | `product-category` (hierarchical) |
| `register-sections` (filter point) | `store/product-grid` (binds `product`), `store/product-hero`, `store/buy-button` — all appear in the studio's insert library, inspector and AI vocabulary automatically |
| `register-routes` | `GET/POST order`, `GET cart`, `POST cart/add`, `POST price` |
| `register-admin` | the currency symbol |
| `register-schedule` | a daily low-stock digest, mailed to the owner |
| `register-assets` | card/button CSS, styled against the theme's `--vy-*` tokens |

## Design decisions worth copying

- **Price and stock are post meta**, written through this plugin's own
  admin-gated route (`POST price`). Products stay ordinary entries; the
  plugin owns only what is commerce-shaped about them.
- **Bindings are resolved by the host.** `store/product-grid` declares
  `"binds": true` and receives entries in the render payload; it holds no
  database capability for querying and needs none.
- **The cart is signed-in only.** Plugin routes never see or set cookies —
  that boundary keeps visitors' sessions away from third-party code. A
  guest cart would need browser state this plugin is deliberately not
  allowed to create, so it does not pretend otherwise.
- **Checkout is an order request, not a payment.** The visitor asks; the
  owner gets a mail (`send-mail` to `site-admin` — the plugin never
  learns the address). Payment processing belongs to a future payment
  provider plugin slot, with its own security review.

## Build

```sh
cargo build --release --target wasm32-wasip2 \
  && cp target/wasm32-wasip2/release/vyasa_plugin_storefront.wasm storefront-component.wasm
```
