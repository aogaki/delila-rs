// build.rs - Generate CAEN FELib bindings using bindgen

use std::env;
use std::path::PathBuf;

fn main() {
    // Tell cargo to look for shared libraries in /usr/local/lib
    println!("cargo:rustc-link-search=/usr/local/lib");

    // Tell cargo to link the CAEN_FELib library
    println!("cargo:rustc-link-lib=CAEN_FELib");

    // macOS: Set rpath for runtime library loading
    #[cfg(target_os = "macos")]
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/local/lib");

    // Tell cargo to invalidate the built crate whenever the wrapper changes
    println!("cargo:rerun-if-changed=src/reader/caen/wrapper.h");

    // Generate bindings
    let bindings = bindgen::Builder::default()
        // The input header
        .header("src/reader/caen/wrapper.h")
        // Include path for CAEN headers
        .clang_arg("-I/usr/local/include")
        // CAEN uses two naming conventions:
        // - Functions/Types: CAEN_FELib_* (CamelCase)
        // - Macros/Constants: CAEN_FELIB_* (ALL_CAPS)
        .allowlist_function("CAEN_FELib_.*")
        .allowlist_type("CAEN_FELib_.*")
        .allowlist_var("CAEN_FELIB_.*")
        // Use Rust enums for C enums
        .rustified_enum("CAEN_FELib_ErrorCode")
        // Generate comments from C docs
        .generate_comments(true)
        // Derive common traits
        .derive_debug(true)
        .derive_default(true)
        .derive_eq(true)
        .derive_hash(true)
        // Generate bindings
        .generate()
        .expect("Unable to generate bindings");

    // Write the bindings to the $OUT_DIR/bindings.rs file
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("caen_felib_bindings.rs"))
        .expect("Couldn't write bindings!");
}
