# Darkshot

A privacy-first screenshot and annotation tool for Windows. Captures leave Darkshot
only through the clipboard or a file you choose to save. There is no upload, no
installer and no automatic startup.

## Build

Darkshot comes in two versions, built from the same source:

- **Office**: the default. The capture shortcut is an ordinary Windows hotkey, so
  combinations Windows keeps for itself (such as Win+Shift+S) cannot be used.
  Settings are stored only beside the exe, and there are no diagnostic log files.
- **Personal**: also takes over shortcuts from Windows with a keyboard hook, falls
  back to `%APPDATA%\Darkshot` for settings when the exe's folder is read-only, can
  write a diagnostic log (`--trace`), and has a "Start with Windows" setting.

### Prerequisites

- Windows 10 or 11.
- [Rust](https://rustup.rs) (stable, MSVC toolchain).
- The Microsoft C++ build tools: Visual Studio Build Tools with the
  "Desktop development with C++" workload.

### Office version

From the repository root:

```
cargo build --release --locked
```

The exe is `target\release\darkshot.exe`.

### Personal version

From the repository root:

```
cargo build --release --locked --features personal --target-dir target/personal
```

The exe is `target\personal\release\darkshot.exe`. The separate target folder keeps
both versions side by side. Without `--target-dir`, both builds write
`target\release\darkshot.exe` and the second one replaces the first.

### Using it portably

Copy `darkshot.exe` on its own into a folder you can write to, and run it from there.
It needs no other files. Settings are saved as `darkshot.toml` in the same folder the
first time you save them, so the folder can be moved or copied as a whole.

Running `darkshot.exe` puts an icon in the notification area. Press the capture
shortcut (Win+Shift+S by default) or click the icon to capture. Windows keeps
Win+Shift+S for its own Snipping Tool: the personal build takes it over, while the office
build can only use it once it is freed in Windows; otherwise pick another shortcut in
Settings. Right-click the icon for
Settings and Quit.

### Security

- No network code: nothing in Darkshot or its dependencies uploads or downloads.
- Runs as the signed-in user. Its manifest never asks for administrator rights.
- The exe carries version information naming its build (office or personal), shown
  under Properties → Details.
- Exploit mitigations: ASLR, DEP, Control Flow Guard and CET shadow-stack
  compatibility. DLLs are loaded only from System32, so a DLL placed next to the exe
  is ignored.
- Only the PNG, JPEG and ICO image formats are compiled in.
- Check the dependencies against the RustSec advisory database with
  `cargo install cargo-audit` and then `cargo audit`.

### Starting with Windows

Neither way needs administrator rights, and both affect only your own account.

- **Either version:** press Win+R, type `shell:startup` and press Enter. In the
  folder that opens, right-click, choose New → Shortcut and browse to `darkshot.exe`.
  Delete the shortcut to stop. Darkshot itself changes nothing in Windows.
- **Personal version:** tick "Start Darkshot when I sign in to Windows" in Settings.
  This adds Darkshot to `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`
  and takes effect at once; untick to remove it. If you move the exe, Settings shows
  that the entry points to the old location, and ticking the box again fixes it.
