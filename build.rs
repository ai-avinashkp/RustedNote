//! Build script: on Windows, embed the app icon and version/product metadata
//! into the executable so RustedNote.exe has a proper icon and file details.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "RustedNote");
        res.set("FileDescription", "RustedNote — plain-text editor");
        res.set("CompanyName", "RustedNote");
        res.set("LegalCopyright", "Copyright (c) 2026 Avinash Prajapati. Personal use only");
        if let Err(e) = res.compile() {
            // Don't hard-fail the build if the resource compiler is unavailable;
            // the app still runs, just without the embedded icon/metadata.
            println!("cargo:warning=winresource: {e}");
        }
    }
    println!("cargo:rerun-if-changed=assets/icon.ico");
}
