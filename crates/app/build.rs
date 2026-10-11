//! Embeds the app icon and version info into the Windows executable.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("../../assets/icon.ico");
    resource.set("ProductName", "GodCode");
    resource.set("FileDescription", "GodCode");
    if let Err(err) = resource.compile() {
        // A missing Windows SDK must not block the build; the exe just keeps the default icon.
        println!("cargo:warning=could not embed the Windows icon: {err}");
    }
}
