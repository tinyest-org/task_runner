// build.rs
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}