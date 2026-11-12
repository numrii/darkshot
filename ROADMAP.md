# Darkshot roadmap

Darkshot is a privacy-first screenshot tool: captures leave the app only through the
clipboard or a file on disk. There is no upload and no network code.

## Next

- **Snap to window edges.** Hold Ctrl while drawing or resizing the capture region to
  snap it to the edges of the window under the cursor. Needs the list of window
  rectangles captured together with the screenshot.

## Planned

- **Take over shortcuts Windows reserves.** An optional setting to claim a combination
  Windows holds for itself, such as Win+Shift+S (Snipping Tool), without the user
  editing the registry. Use a low-level keyboard hook (`WH_KEYBOARD_LL`, as AutoHotkey
  and PowerToys do) that swallows the combination before Windows sees it. Nothing in
  Windows is changed, and the takeover ends when Darkshot quits. The hook callback must
  return quickly, or Windows removes the hook.
- **Release build.** A proper app icon, a size-optimised release profile, and turning
  off eframe's default features so no code that can open a URL (the `webbrowser`
  crate) is compiled in.
- **Numbered step markers** (1, 2, 3…) for step-by-step instructions.
- **Eyedropper** to pick a colour from the screenshot.

## Done

- Tray icon and global capture shortcut; one overlay spanning all monitors.
- Capture region with resize handles, move, arrow-key nudge, Ctrl+A for the current
  monitor; Shift constrains, Alt works from the centre.
- Editable annotations: rectangle, ellipse, line, arrow, pen, highlighter, text and
  blur, with undo and redo.
- Stroke styles (solid, dashed, long dash, dash dot, dotted) and arrowheads (triangle,
  open, stealth, dot; optionally on both ends).
- Highlighter: freehand, or straight with Shift; translucent, yellow by default.
- Text: plain, outlined or boxed; multi-line; editable after placing.
- Blur tool: pixelate, Gaussian or a solid redaction.
- Context-based toolbar options that keep the toolbar's width fixed.
- Copy to clipboard; save as PNG or JPEG (with quality) through a save panel.
- Settings file next to the exe and a settings window: shortcut (including the Windows
  key), colours, file name prefix and suffix with `{date}` and `{time}`, format,
  quality and default folder.
