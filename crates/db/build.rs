//! A new file under `migrations/` must rebuild this crate: `sqlx::migrate!`
//! reads the directory when the crate compiles, and without this cargo
//! saw no Rust source change, kept the old object, and shipped a binary
//! whose migrator did not know the newest migration.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
