//! Writes the Kotlin and Python bindings from a built library:
//! `cargo run -p ankiquest-rules --features bindgen --bin uniffi-bindgen -- generate --library <lib> --language kotlin --out-dir <dir>`.

fn main() {
    uniffi::uniffi_bindgen_main()
}
