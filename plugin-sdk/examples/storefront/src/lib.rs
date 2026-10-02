//! The reference commerce plugin: the WooCommerce test, small but honest.
//!
//! Everything here goes through the public contract — post types, meta,
//! designer sections, routes, settings, a schedule, mail — and nothing
//! here is known to the host by name. What it proves:
//!
//! * products are a **post type** (`product`), edited like any entry
//! * price and stock are **post meta**, set through this plugin's route
//! * the store page is **designed in the studio** from this plugin's
//!   sections (`store/product-grid`, `store/product-hero`,
//!   `store/buy-button`), with bindings resolved by the host
//! * the **cart is signed-in only**: plugin routes never see or set
//!   cookies — that boundary keeps session material away from third-party
//!   code, so a guest cart would need the visitor's browser to carry
//!   state this plugin is deliberately not allowed to give it
//! * **checkout is an order request**, not a payment: the visitor asks,
//!   the owner gets a mail. Payments are a future provider-plugin slot.

wit_bindgen::generate!({
    path: "../../../crates/plugins/wit",
    world: "vyasa-plugin-v2",
});

use vyasa::plugin::host;

struct Storefront;

const CURRENCY_KEY: &str = "settings/currency";
const ORDER_SEQ_KEY: &str = "orders/seq";

fn currency() -> String {
    host::kv_get(CURRENCY_KEY)
        .ok()
        .flatten()
        .unwrap_or_else(|| "₹".to_owned())
}

fn price_of(post_id: u64) -> Option<String> {
    host::get_post_meta(post_id, "price").ok().flatten()
}

fn html(status: u16, body: String) -> host::HttpResponse {
    host::HttpResponse {
        status,
        headers: vec![("content-type".to_owned(), "text/html; charset=utf-8".to_owned())],
        body,
    }
}

fn json(status: u16, body: String) -> host::HttpResponse {
    host::HttpResponse {
        status,
        headers: vec![("content-type".to_owned(), "application/json".to_owned())],
        body,
    }
}

/// One `key=value` out of a query string or form body. Enough for an
/// example; a real store would pull in a form crate.
fn field(encoded: &str, key: &str) -> Option<String> {
    encoded.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| v.replace('+', " ").replace("%40", "@"))
    })
}

impl Guest for Storefront {
    fn init(config_json: String) -> Result<PluginInfo, String> {
        let currency = serde_json::from_str::<serde_json::Value>(&config_json)
            .ok()
            .and_then(|v| v.get("currency").and_then(|c| c.as_str()).map(str::to_owned))
            .unwrap_or_else(|| "₹".to_owned());
        host::kv_set(CURRENCY_KEY, &currency)?;
        Ok(PluginInfo {
            name: "storefront".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        })
    }

    fn filter(_stage: FilterStage, content: String) -> String {
        content
    }

    fn action(_kind: EventKind, _payload_json: String) -> Result<(), String> {
        Ok(())
    }

    fn register_blocks() -> String {
        "[]".to_owned()
    }

    fn register_routes() -> String {
        serde_json::json!([
            {"method": "GET", "path": "order"},
            {"method": "POST", "path": "order"},
            {"method": "GET", "path": "cart"},
            {"method": "POST", "path": "cart/add"},
            {"method": "POST", "path": "price"}
        ])
        .to_string()
    }

    fn handle_request(req: host::HttpRequest) -> Result<host::HttpResponse, String> {
        match (req.method.as_str(), req.path.as_str()) {
            // The order form: reachable by anyone, no account needed. An
            // order request is a message, not a transaction.
            ("GET", "order") => {
                let product = field(&req.query, "product").unwrap_or_default();
                let title = product
                    .parse::<u64>()
                    .ok()
                    .and_then(|id| host::get_post(id).ok())
                    .map_or_else(|| "this product".to_owned(), |p| p.title);
                Ok(html(200, format!(
                    "<h1>Request to order {title}</h1>\
                     <form method=\"post\" action=\"/api/v1/plugin/storefront/order\">\
                     <input type=\"hidden\" name=\"product\" value=\"{product}\">\
                     <label>Your name <input name=\"name\" required></label>\
                     <label>Your email <input name=\"email\" type=\"email\" required></label>\
                     <button type=\"submit\">Send the request</button></form>"
                )))
            }
            ("POST", "order") => {
                let product = field(&req.body, "product").unwrap_or_default();
                let name = field(&req.body, "name").unwrap_or_default();
                let email = field(&req.body, "email").unwrap_or_default();
                if name.is_empty() || email.is_empty() {
                    return Ok(html(422, "<p>Name and email are required.</p>".to_owned()));
                }
                let seq: u64 = host::kv_get(ORDER_SEQ_KEY)
                    .ok()
                    .flatten()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0)
                    + 1;
                host::kv_set(ORDER_SEQ_KEY, &seq.to_string())?;
                host::kv_set(
                    &format!("orders/{seq:08}"),
                    &serde_json::json!({"product": product, "name": name, "email": email})
                        .to_string(),
                )?;
                // The owner hears about it; the plugin never learns their
                // address — `site-admin` resolves host-side.
                host::send_mail(
                    "site-admin",
                    "New order request",
                    &format!("{name} <{email}> asked to order product #{product}."),
                )?;
                Ok(html(200, "<h1>Thank you</h1><p>The owner will be in touch.</p>".to_owned()))
            }
            // The cart is per-account: plugin routes never see cookies, so
            // there is nothing to hang a guest cart on — by design, and
            // this example says so instead of working around it.
            ("POST", "cart/add") => {
                let Ok(Some(viewer)) = host::current_viewer() else {
                    return Ok(json(401, r#"{"error":"sign in to keep a cart"}"#.to_owned()));
                };
                let product = field(&req.query, "product")
                    .or_else(|| field(&req.body, "product"))
                    .unwrap_or_default();
                if product.parse::<u64>().is_err() {
                    return Ok(json(422, r#"{"error":"product must be an id"}"#.to_owned()));
                }
                let key = format!("cart/{}/{}", viewer.id, product);
                let count: u64 = host::kv_get(&key)
                    .ok()
                    .flatten()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0)
                    + 1;
                host::kv_set(&key, &count.to_string())?;
                Ok(json(200, format!("{{\"product\":{product},\"count\":{count}}}")))
            }
            ("GET", "cart") => {
                let Ok(Some(viewer)) = host::current_viewer() else {
                    return Ok(json(401, r#"{"error":"sign in to see your cart"}"#.to_owned()));
                };
                let prefix = format!("cart/{}/", viewer.id);
                let mut items = Vec::new();
                for key in host::kv_list(&prefix, 100)? {
                    let count = host::kv_get(&key).ok().flatten().unwrap_or_default();
                    let id = key.trim_start_matches(&prefix).to_owned();
                    items.push(serde_json::json!({"product": id, "count": count}));
                }
                Ok(json(200, serde_json::json!({"items": items}).to_string()))
            }
            // Prices are meta, and meta writes go through the plugin that
            // owns them. Admins only: a shop where any signed-in reader
            // reprices the stock is a different kind of shop.
            ("POST", "price") => {
                match host::current_viewer() {
                    Ok(Some(viewer)) if viewer.role == "admin" => {}
                    _ => return Ok(json(403, r#"{"error":"admins set prices"}"#.to_owned())),
                }
                let product = field(&req.body, "product")
                    .and_then(|p| p.parse::<u64>().ok())
                    .ok_or("product must be an id")?;
                let value = field(&req.body, "value").unwrap_or_default();
                host::set_post_meta(product, "price", &value)?;
                if let Some(stock) = field(&req.body, "stock") {
                    host::set_post_meta(product, "stock", &stock)?;
                }
                Ok(json(200, r#"{"ok":true}"#.to_owned()))
            }
            _ => Ok(host::HttpResponse {
                status: 404,
                headers: vec![],
                body: "no such endpoint".to_owned(),
            }),
        }
    }

    fn render_block(_kind: String, _attrs_json: String) -> Result<String, String> {
        Err("storefront declares no editor blocks".to_owned())
    }

    fn run_task(name: String) -> Result<(), String> {
        if name != "stock-digest" {
            return Ok(());
        }
        let mut low = Vec::new();
        for product in host::query_posts(Some("product"), 100)? {
            let stock = host::get_post_meta(product.id, "stock")
                .ok()
                .flatten()
                .and_then(|v| v.parse::<i64>().ok());
            if let Some(n) = stock {
                if n < 3 {
                    low.push(format!("{} ({n} left)", product.title));
                }
            }
        }
        if !low.is_empty() {
            host::send_mail(
                "site-admin",
                "Low stock",
                &format!("Running low: {}", low.join(", ")),
            )?;
        }
        Ok(())
    }

    fn register_schedule() -> String {
        serde_json::json!([{"name": "stock-digest", "every-seconds": 86_400}]).to_string()
    }

    fn register_admin() -> String {
        serde_json::json!({
            "title": "Storefront",
            "description": "Products are ordinary entries of the product type; price and stock are meta this plugin owns.",
            "fields": [
                {"key": "currency", "label": "Currency symbol", "type": "text", "default": "₹"}
            ]
        })
        .to_string()
    }

    fn register_assets() -> String {
        serde_json::json!({
            "css": ".store-grid{display:grid;grid-template-columns:repeat(var(--vy-cols,3),1fr);gap:1.2rem}\
                    .store-card{background:var(--vy-color-surface);border:1px solid var(--vy-color-border);\
                    border-radius:var(--vy-radius);padding:1rem;display:flex;flex-direction:column;gap:.3rem}\
                    .store-price{color:var(--vy-color-primary);font-weight:700}\
                    .store-buy{display:inline-block;background:var(--vy-color-primary);color:var(--vy-color-on-primary);\
                    padding:.55em 1.1em;border-radius:var(--vy-radius);text-decoration:none;font-weight:600}"
        })
        .to_string()
    }

    fn register_post_types() -> String {
        serde_json::json!([
            {"slug": "product", "singular": "Product", "plural": "Products",
             "public": true, "has-archive": true}
        ])
        .to_string()
    }

    fn register_taxonomies() -> String {
        serde_json::json!([
            {"slug": "product-category", "singular": "Product category",
             "plural": "Product categories", "hierarchical": true, "public": true}
        ])
        .to_string()
    }

    fn filter_at(point: String, payload: String) -> String {
        match point.as_str() {
            "register-sections" => sections_json(),
            "render-section" => render_section(&payload),
            _ => payload,
        }
    }

    fn on_event(_name: String, _payload_json: String) -> Result<(), String> {
        Ok(())
    }
}

fn sections_json() -> String {
    serde_json::json!([
        {
            "kind": "store/product-grid",
            "title": "Product grid",
            "category": "commerce",
            "description": "Bound products as cards, with prices",
            "settings-schema": {
                "type": "object",
                "properties": {
                    "bind": {"type": "object", "properties": {}},
                    "heading": {"type": "string", "maxLength": 120}
                },
                "additionalProperties": false
            },
            "sample": {"bind": {"source": "product", "sort": "newest", "limit": 6},
                        "heading": "New arrivals"},
            "binds": true,
            "inline": ["heading"]
        },
        {
            "kind": "store/product-hero",
            "title": "Product hero",
            "category": "commerce",
            "description": "One product, large: title, price, order button",
            "settings-schema": {
                "type": "object",
                "properties": {"product": {"type": "integer", "minimum": 1}},
                "required": ["product"],
                "additionalProperties": false
            },
            "sample": {},
            "binds": false,
            "inline": []
        },
        {
            "kind": "store/buy-button",
            "title": "Buy button",
            "category": "commerce",
            "description": "A request-to-order button for one product",
            "settings-schema": {
                "type": "object",
                "properties": {
                    "product": {"type": "integer", "minimum": 1},
                    "label": {"type": "string", "maxLength": 60}
                },
                "required": ["product"],
                "additionalProperties": false
            },
            "sample": {"label": "Request to order"},
            "binds": false,
            "inline": ["label"]
        }
    ])
    .to_string()
}

fn render_section(payload: &str) -> String {
    let Ok(req) = serde_json::from_str::<serde_json::Value>(payload) else {
        return payload.to_owned();
    };
    let settings = req.get("settings").cloned().unwrap_or_default();
    let sym = currency();
    match req.get("kind").and_then(|k| k.as_str()) {
        Some("store/product-grid") => {
            let heading = settings
                .get("heading")
                .and_then(|h| h.as_str())
                .unwrap_or("Products");
            let mut out = format!("<h2 class=\"store-heading\">{}</h2>", escape(heading));
            out.push_str("<div class=\"store-grid\">");
            let empty = Vec::new();
            for entry in req.get("bound").and_then(|b| b.as_array()).unwrap_or(&empty) {
                let title = entry.get("title").and_then(|t| t.as_str()).unwrap_or("");
                let url = entry.get("url").and_then(|u| u.as_str()).unwrap_or("#");
                let id = entry.get("id").and_then(serde_json::Value::as_u64).unwrap_or(0);
                let price = price_of(id)
                    .map(|p| format!("<span class=\"store-price\">{sym}{}</span>", escape(&p)))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "<article class=\"store-card\"><a href=\"{}\">{}</a>{price}</article>",
                    escape(url),
                    escape(title)
                ));
            }
            out.push_str("</div>");
            out
        }
        Some("store/product-hero") => {
            let Some(id) = settings.get("product").and_then(serde_json::Value::as_u64) else {
                return String::new();
            };
            let Ok(product) = host::get_post(id) else {
                return String::new();
            };
            let price = price_of(id)
                .map(|p| format!("<p class=\"store-price\">{sym}{}</p>", escape(&p)))
                .unwrap_or_default();
            format!(
                "<div class=\"store-hero\"><h2>{}</h2>{price}\
                 <a class=\"store-buy\" href=\"/api/v1/plugin/storefront/order?product={id}\">\
                 Request to order</a></div>",
                escape(&product.title)
            )
        }
        Some("store/buy-button") => {
            let Some(id) = settings.get("product").and_then(serde_json::Value::as_u64) else {
                return String::new();
            };
            let label = settings
                .get("label")
                .and_then(|l| l.as_str())
                .unwrap_or("Request to order");
            format!(
                "<a class=\"store-buy\" href=\"/api/v1/plugin/storefront/order?product={id}\">{}</a>",
                escape(label)
            )
        }
        _ => payload.to_owned(),
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

export!(Storefront);
