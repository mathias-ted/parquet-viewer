//! Embeds assets/icon.ico into the Windows .exe (the icon shown in Explorer and on the taskbar).
//! If the Windows resource compiler is not available, the build continues without the file icon.

#[cfg(windows)]
fn embed_icon() {
    let mut res = winres::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    if let Err(e) = res.compile() {
        println!("cargo:warning=could not embed the exe icon ({e}); the program still works");
    }
}

#[cfg(not(windows))]
fn embed_icon() {}

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    embed_icon();
}
