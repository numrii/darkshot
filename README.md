# Darkshot

A privacy-first screenshot and annotation tool for Windows. Captures leave Darkshot
only through the clipboard or a file you choose to save. There is no upload and no
installer, and nothing starts with Windows unless you set it up.

## Using Darkshot

Run `darkshot.exe` and it sits in the notification area. Press the capture shortcut
(Win+Shift+S by default) or click the icon to capture; right-click the icon for
Settings and Quit.

Windows keeps Win+Shift+S for its own Snipping Tool. The personal build takes it over;
the office build can use it only once it is freed in Windows, so pick another shortcut
in Settings if it does not register.

### Capturing

- Drag to select a region, then move or resize it with the handles. Shift keeps the
  proportions, Alt resizes from the centre, and the arrow keys nudge it.
- Ctrl+click selects a window. Hold Ctrl while drawing, moving or resizing to snap to
  window and monitor edges.
- Ctrl+A selects the whole monitor under the cursor.
- Ctrl+C or Enter copies, Ctrl+S saves, Esc cancels.

### Annotating

| Key | Tool |
| --- | --- |
| V | Select and move annotations |
| R | Rectangle |
| E | Ellipse |
| L | Line |
| A | Arrow |
| P | Pen |
| M | Highlighter (Shift for a straight line) |
| T | Text |
| B | Blur: pixelate, Gaussian or solid redaction |
| N | Numbered steps |
| I | Eyedropper (Shift magnifies to single pixels) |

The mouse wheel changes thickness, font size, blur strength or step size. Ctrl+Z
undoes, Ctrl+Y or Ctrl+Shift+Z redoes, and Delete removes the selected annotation.

### Settings

Settings has the capture shortcut, the default colours, a light, dark or system theme,
the save format and quality, the default folder and the file name. The file name is a
template that can use `{date}`, `{time}`, `{year}`, `{month}`, `{day}`, `{hour}`,
`{minute}`, `{second}` and `{counter}` (the next number not yet used in the save
folder); hover the `?` beside the field for examples.

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

## Licence

MIT. See [LICENSE](LICENSE).
