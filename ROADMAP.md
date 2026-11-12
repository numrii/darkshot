# Darkshot roadmap

Darkshot is a privacy-first screenshot tool: captures leave the app only through the
clipboard or a file on disk. There is no upload and no network code.

## Next

Nothing scheduled. Ideas welcome.

## Done

- Tray icon and global capture shortcut; one overlay spanning all monitors.
- Capture region with resize handles, move, arrow-key nudge, Ctrl+A for the current
  monitor; Shift constrains, Alt works from the centre.
- Release build: the app icon embedded in the exe and used for the tray and settings
  window; whole-program optimisation; eframe trimmed to the features Darkshot uses, so
  no URL-opening code (and no networking crate at all) is compiled in.
- Numbered step markers (N): click to place; each new one takes the next number.
- Eyedropper (I): picks a colour from the capture itself, with dimming hidden while
  picking and a live preview of the colour under the cursor.
- Shortcut takeover: an optional setting that catches the shortcut with a low-level
  keyboard hook, so combinations Windows keeps for itself (Win+Shift+S) work without
  registry changes. Recording a shortcut uses the same hook, so any combination can be
  recorded. A lone Windows-key release is masked so the Start menu does not open.
- Window snapping: Ctrl+click selects a window; holding Ctrl while drawing, moving or
  resizing the region snaps its edges to window and monitor edges (ties go to the
  frontmost window).
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
