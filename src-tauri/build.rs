fn main() {
    // Icon edits must also refresh the Windows PE resources on incremental builds.
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
