fn main() {
    println!("cargo:rerun-if-env-changed=CLAUDE_GUARD_PAYLOAD");
    let destination =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("payload.zip");
    if let Some(path) = std::env::var_os("CLAUDE_GUARD_PAYLOAD") {
        println!(
            "cargo:rerun-if-changed={}",
            std::path::Path::new(&path).display()
        );
        std::fs::copy(path, destination).expect("installer payload");
    } else {
        std::fs::write(destination, []).unwrap();
    }
}
