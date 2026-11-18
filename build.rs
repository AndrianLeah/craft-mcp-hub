//! On Windows, puts the icon and the version details into the programs, so Explorer, the Start
//! menu and the taskbar show them. Other systems take the icon when the window opens.

const ICON: &str = "assets/icons/windows/craft-mcp-hub.ico";

fn main() {
    println!("cargo:rerun-if-changed={ICON}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") || !std::path::Path::new(ICON).is_file() {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon(ICON).set("ProductName", "Craft MCP Hub").set("FileDescription", "Craft MCP Hub");
    // A build machine without the Windows resource compiler still builds, just without the icon.
    if let Err(why) = resource.compile() {
        println!("cargo:warning=the icon was not put into the programs: {why}");
    }
}
