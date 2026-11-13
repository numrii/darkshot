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
- Eyedropper (I): picks a colour from the capture itself. A round spotlight shows the
  undimmed capture around the cursor (hold Shift to magnify it to single pixels), with
  the colour's hex, RGB and HSL values beside it.
- Office and personal builds from one source (see the README). The default office
  build has no keyboard hook, stores settings only beside the exe and writes no
  diagnostic logs; `--features personal` adds the rest.
- Shortcut takeover (personal build): an optional setting that catches the shortcut
  with a low-level keyboard hook, so combinations Windows keeps for itself
  (Win+Shift+S) work without registry changes. Recording a shortcut uses the same hook,
  so any combination can be recorded. A lone Windows-key release is masked so the Start
  menu does not open. The office build records shortcuts only from the focused
  settings window.
- Start with Windows (personal build): a Settings checkbox that adds Darkshot to the
  current user's Run key, no administrator rights needed. Either build can instead be
  started from a shortcut in the Startup folder (see the README).
- Privacy hardening: solid redactions are applied after pixelate and blur, so an
  overlapping blur cannot bring hidden pixels back; copies stay out of Windows
  clipboard history and cloud sync; Save asks before replacing a file when it has to
  add the extension; the Copy and Save test overrides exist only in debug builds.
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
- Settings file next to the exe (the personal build falls back to `%APPDATA%\Darkshot`)
  and a settings window: shortcut (including the Windows key), colours, light, dark or
  system theme, file name, format, quality and default folder. The window can be resized
  and scrolls; Save and Cancel stay at the bottom.
- Win+Shift+S is the default shortcut (taken over from Windows in the personal build).
- Colour picker: one picker for every colour, with a wheel per tab (an RGB hexagon, an
  HSV ring and square, a CMYK ink wheel), gradient sliders, a hex field with copy, and
  a separate alpha slider. Colour and opacity are kept apart, so lowering alpha or
  passing through black never loses the colour.
- File names from one template with `{date}`, `{time}`, `{year}`, `{month}`, `{day}`,
  `{hour}`, `{minute}`, `{second}` and `{counter}` (the next free number in the save
  folder). Characters Windows forbids are refused, and reserved device names avoided.
- Popups and tooltips stay on the monitor they open on; tooltips wrap.
- The overlay draws at most one frame per display refresh.
- Sharp at any display scaling: per-monitor DPI awareness declared in the manifest.
