// Ensure the embed target for `include_dir!` exists so fresh clones compile without a
// frontend build; an empty dir means `kansa ui` serves a "build the UI" page instead.
fn main() {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/dist");
    let _ = std::fs::create_dir_all(&dist);
    println!("cargo:rerun-if-changed={}", dist.display());
}
