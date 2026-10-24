//! Embeds the application icon and version information into the Windows
//! executable (shown by Explorer, the taskbar pin, Task Manager and the
//! installer's "Apps" entry).

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/notch.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let icon = manifest_dir.join("assets").join("notch.ico");
    // Forward slashes are accepted by both windres and RC.EXE.
    let icon = icon.display().to_string().replace('\\', "/");
    let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let part = |name: &str| env::var(name).ok().and_then(|v| v.parse::<u16>().ok()).unwrap_or(0);
    let (major, minor, patch) =
        (part("CARGO_PKG_VERSION_MAJOR"), part("CARGO_PKG_VERSION_MINOR"), part("CARGO_PKG_VERSION_PATCH"));

    let rc = format!(
        r#"1 ICON "{icon}"

1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEFLAGSMASK 0x3F
FILEFLAGS 0x0
FILEOS 0x40004
FILETYPE 0x1
FILESUBTYPE 0x0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "nabildzr"
      VALUE "FileDescription", "Windows Dynamic Notch"
      VALUE "LegalCopyright", "Copyright (c) 2026 nabildzr. MIT License."
      VALUE "Comments", "https://github.com/nabildzr/windows-dynamic-notch"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "windows-dynamic-notch"
      VALUE "OriginalFilename", "windows-dynamic-notch.exe"
      VALUE "ProductName", "Windows Dynamic Notch"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let rc_path = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("app.rc");
    fs::write(&rc_path, rc).expect("cannot write app.rc");
    embed_resource::compile(&rc_path, embed_resource::NONE).manifest_optional().expect("cannot embed resources");
}
