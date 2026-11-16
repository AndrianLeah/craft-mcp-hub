//! On Windows, puts the icon and the version details into the programs, so Explorer, the Start
//! menu and the taskbar show them. Other systems take the icon when the window opens.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") || !std::path::Path::new("assets/icon.ico").is_file() {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("assets/icon.ico").set("ProductName", "Craft MCP Hub").set("FileDescription", "Craft MCP Hub");
    // A build machine without the Windows resource compiler still builds, just without the icon.
    if let Err(why) = resource.compile() {
        println!("cargo:warning=the icon was not put into the programs: {why}");
    }
}
