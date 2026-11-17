fn main() {
    let target = std::env::var("TARGET").unwrap();
    let root = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let archive = root.join(".tools/desktop-shim").join(&target);
    assert!(
        archive.join("libgamelink_desktop.a").is_file(),
        "prebuild the Go archive with scripts/build-desktop.sh --archives-only {target}"
    );
    println!("cargo:rustc-link-search=native={}", archive.display());
    println!(
        "cargo:rustc-link-search=native={}",
        root.join("core/target/cgo").join(&target).display()
    );
    println!("cargo:rustc-link-lib=static=gamelink_desktop");
    println!("cargo:rustc-link-lib=static=gamelink_ffi");
    println!(
        "cargo:rerun-if-changed={}",
        archive.join("libgamelink_desktop.a").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join("core/target/cgo")
            .join(&target)
            .join("libgamelink_ffi.a")
            .display()
    );
    if target.contains("linux") {
        for lib in ["pthread", "dl", "m", "resolv"] {
            println!("cargo:rustc-link-lib={lib}");
        }
    } else if target.contains("windows") {
        for lib in [
            "ws2_32", "ntdll", "userenv", "bcrypt", "crypt32", "secur32", "winmm",
        ] {
            println!("cargo:rustc-link-lib={lib}");
        }
    }
    tauri_build::build();
}
