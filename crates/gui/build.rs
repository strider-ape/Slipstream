fn main() {
    // Compile the UI description into Rust at build time.
    slint_build::compile("ui/app.slint").expect("ui/app.slint compiles");
    // Windows: give the .exe the Slipstream icon (taskbar, Start menu, tray) and file details.
    println!("cargo:rerun-if-changed=../../assets/slipstream.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("../../assets/slipstream.ico")
            .compile()
            .expect("Windows resources compile");
    }
}
