//! Re-run the build when the embedded migrations change (`sqlx::migrate!`
//! cannot register the directory itself).
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
