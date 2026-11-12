//! Embeds the app icon (assets/darkshot.ico) into the Windows executable.

fn main() {
    println!("cargo:rerun-if-changed=assets/darkshot.rc");
    println!("cargo:rerun-if-changed=assets/darkshot.ico");
    embed_resource::compile("assets/darkshot.rc", embed_resource::NONE)
        .manifest_optional()
        .expect("could not embed the app icon");
}
