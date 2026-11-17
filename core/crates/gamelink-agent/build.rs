fn main() {
    let target = std::env::var("TARGET").unwrap();
    let root =
        std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../..");
    let archive = root.join(".tools/agent-shim").join(&target);
    assert!(
        archive.join("libgamelink_agent.a").is_file(),
        "prebuild with scripts/build-agent-shim.sh {target}"
    );
    println!("cargo:rustc-link-search=native={}", archive.display());
    println!(
        "cargo:rustc-link-search=native={}",
        root.join("core/target/cgo").join(&target).display()
    );
    println!("cargo:rustc-link-lib=static=gamelink_agent");
    println!("cargo:rustc-link-lib=static=gamelink_ffi");
    println!(
        "cargo:rerun-if-changed={}",
        archive.join("libgamelink_agent.a").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join("core/target/cgo")
            .join(&target)
            .join("libgamelink_ffi.a")
            .display()
    );
    if !target.contains("musl") {
        for lib in ["pthread", "dl", "m", "resolv"] {
            println!("cargo:rustc-link-lib={lib}");
        }
    }
}
