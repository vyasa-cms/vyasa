//! `sqlx::migrate!` embeds `crates/db/migrations` at compile time; a new
//! migration must rebuild this crate or the template would miss it.
fn main() {
    println!("cargo:rerun-if-changed=../db/migrations");
}
