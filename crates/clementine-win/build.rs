fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/clementine.ico");
        res.set("ProductName", "Clementine");
        res.set("FileDescription", "Clementine - Zero-Click Offline File Converter");
        res.compile().expect("embed icon resource");
    }
}
