fn main() {
    // Compile the UI description into Rust at build time.
    slint_build::compile("ui/app.slint").expect("ui/app.slint compiles");
}
