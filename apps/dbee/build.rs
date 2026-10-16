//! Windows only: embed the app icon and version info (VERSIONINFO) into `dbee.exe`, so it
//! shows in Explorer, the taskbar, the Start menu and Alt-Tab.
//!
//! On every other target this does nothing. A missing resource compiler is a warning, so a
//! cross-compile from macOS or Linux still links, unless `DBEE_REQUIRE_WINRES=1` turns it
//! into an error (for release builds).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/app-icon/dbee.ico");
    println!("cargo:rerun-if-env-changed=DBEE_REQUIRE_WINRES");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/app-icon/dbee.ico")
        .set("ProductName", "dbee")
        .set("FileDescription", "dbee database client")
        .set("CompanyName", "Kaspar Rosin")
        .set("LegalCopyright", "Copyright (c) 2026 Kaspar Rosin and the dbee contributors. MIT OR Apache-2.0.")
        .set("OriginalFilename", "dbee.exe")
        .set("InternalName", "dbee");
    if let Err(e) = res.compile() {
        if std::env::var_os("DBEE_REQUIRE_WINRES").is_some() {
            eprintln!("embedding Windows resources failed: {e}");
            std::process::exit(1);
        }
        println!("cargo:warning=dbee.exe built without icon/version resources: {e}");
    }
}
