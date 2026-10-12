fn main() {
    // The icon of the window (and of the Dock in development) is read into the binary: a new icon
    // must build it again.
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
