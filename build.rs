//! Embed Windows .ico + VERSIONINFO so Explorer Properties → Details is filled.
//!
//! Runs when *targeting* Windows (including cross-compile). On Linux hosts without
//! a Windows resource toolchain, compilation warns and continues.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/app_icon.ico");
        res.set("FileDescription", "Metis Mesh HQ");
        res.set("ProductName", "Metis Mesh HQ");
        res.set("ProductVersion", env!("CARGO_PKG_VERSION"));
        res.set("FileVersion", env!("CARGO_PKG_VERSION"));
        res.set(
            "LegalCopyright",
            "Copyright (c) DigitalExpl0it. MIT License.",
        );
        res.set("CompanyName", "Metis Mesh");
        res.set("OriginalFilename", "metis-mesh-hq.exe");
        res.set("InternalName", "metis-mesh-hq");
        if let Err(err) = res.compile() {
            println!("cargo:warning=winres: {err}");
        }
    }
    println!("cargo:rerun-if-changed=assets/app_icon.ico");
    println!("cargo:rerun-if-changed=assets/app_icon.png");
    println!("cargo:rerun-if-changed=Cargo.toml");
}
