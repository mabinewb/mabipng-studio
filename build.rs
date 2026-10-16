fn main() {
    println!("cargo:rerun-if-changed=assets/app_icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/app_icon.ico")
            .set("ProductName", "MabiPNG Studio")
            .set("FileDescription", "Mabinogi picture chat PNG editor")
            .compile()
            .expect("Windows icon resource compilation failed");
    }
}
