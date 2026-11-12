fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winres::WindowsResource::new();
        resource.set_icon("src/images/windows_app_icon.ico");
        resource
            .compile()
            .expect("failed to embed Windows application icon");
    }
}
