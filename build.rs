//! Embeds Windows resources into the executable: the app icon, an application
//! manifest (run as the user, never elevated) and version information naming the
//! build. The resource script is generated so the version comes from Cargo.toml.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=assets/darkshot.ico");
    println!("cargo:rerun-if-changed=assets/darkshot.manifest");

    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let asset = |name: &str| Path::new(&dir).join("assets").join(name).display().to_string().replace('\\', "\\\\");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut numbers: Vec<u16> = version.split(['.', '-', '+']).map_while(|part| part.parse().ok()).collect();
    numbers.resize(4, 0);
    let numeric = numbers.iter().map(u16::to_string).collect::<Vec<_>>().join(",");
    let edition = if std::env::var_os("CARGO_FEATURE_PERSONAL").is_some() { "personal" } else { "office" };
    let description = format!("Darkshot screenshot tool ({edition} build)");

    let rc = format!(
        r#"#pragma code_page(65001)
1 ICON "{icon}"
1 24 "{manifest}"

1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904B0"
        BEGIN
            VALUE "FileDescription", "{description}"
            VALUE "FileVersion", "{version}"
            VALUE "InternalName", "darkshot"
            VALUE "OriginalFilename", "darkshot.exe"
            VALUE "ProductName", "Darkshot"
            VALUE "ProductVersion", "{version}"
            VALUE "Comments", "Privacy-first screenshot and annotation tool. No network access."
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#,
        icon = asset("darkshot.ico"),
        manifest = asset("darkshot.manifest"),
    );
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("darkshot.rc");
    std::fs::write(&out, rc).expect("could not write the resource script");
    embed_resource::compile(&out, embed_resource::NONE)
        .manifest_required()
        .expect("could not embed the icon, manifest and version information");
}
