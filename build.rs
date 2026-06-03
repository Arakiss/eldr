// Eldr build script.
//
// Eldr links macOS system frameworks (CoreFoundation, IOKit, Foundation) and the
// PRIVATE IOReport framework. The actual linkage is declared with `#[link(...)]`
// attributes on the `extern "C"` blocks in `src/ffi/*` (same approach as macmon),
// so this script only makes the private-framework directory discoverable as a
// belt-and-suspenders fallback for the linker. On modern macOS the linker resolves
// these from the dyld shared cache even though the files are not present on disk.
fn main() {
    // Only meaningful on macOS / Apple Silicon. Eldr is macOS-only by design.
    println!("cargo:rustc-link-search=framework=/System/Library/PrivateFrameworks");
}
