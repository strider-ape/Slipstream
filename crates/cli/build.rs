fn main() {
    // Windows: give the .exe the Slipstream icon and file details.
    println!("cargo:rerun-if-changed=../../assets/slipstream.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("../../assets/slipstream.ico")
            .compile()
            .expect("Windows resources compile");
    }
}
