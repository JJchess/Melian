pub fn embed(description: &str, filename: &str) {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let icon =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../desktop/assets/melian.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    winresource::WindowsResource::new()
        .set_icon(icon.to_str().expect("icon path must be UTF-8"))
        .set("ProductName", "Melian")
        .set("FileDescription", description)
        .set("OriginalFilename", filename)
        .compile()
        .expect("compile Melian Windows icon and version resources");
}
