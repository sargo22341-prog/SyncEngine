//! Windows: embeds the application icon in `syncengine-desktop.exe`
//! (Explorer, taskbar, shortcuts). Other targets need no resource.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=assets/syncengine.rc");
    println!("cargo:rerun-if-changed=assets/syncengine.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/syncengine.rc", embed_resource::NONE)
            .manifest_optional()?;
    }
    Ok(())
}
