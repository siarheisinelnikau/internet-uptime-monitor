fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/app.ico")
            .set("ProductName", "Internet Uptime Monitor")
            .set("FileDescription", "Local internet outage monitor")
            .set(
                "LegalCopyright",
                "Copyright (c) 2026 siarheisinelnikau. MIT License.",
            )
            .compile()
            .expect("compile Windows icon and version resource");
    }
}
