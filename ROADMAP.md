# Duskshot roadmap

Duskshot is a privacy-first screenshot tool: captures leave the app only through the
clipboard or a file on disk. There is no upload and no network code.

## Next

Roughly smallest first. Everything stays offline, and opt-in where it changes what a
capture contains or how long it is kept.

- Try HDR capture on a real HDR display: colours and brightness should match the screen,
  not look washed out. Not yet possible here.

Considered and left out: scrolling capture (it would mean sending scroll input to other
apps, which Duskshot avoids), GIF and video recording, uploads of any kind, and checking
for or downloading updates (that needs network code, which Duskshot has none of).

If GIF recording is reconsidered, this is what it would take (looked into in November
2026; roughly two weeks for recording and saving, three to four with trimming and
annotations):

- Encoding (the small part): the `image` crate's `gif` feature (pure Rust `gif` and
  `color_quant`; not gifski or imagequant, which are GPL). Small files come from one
  palette shared by all frames, then for each frame: pixels identical to what is
  already shown (or within a tolerance) become transparent, the frame is cropped to
  what changed and drawn over the previous one, and identical frames only lengthen the
  previous frame's delay. Compare against the shown image rather than the previous
  source frame, so tolerances do not add up into drift, and avoid random dithering,
  whose noise changes every frame and defeats the transparency.
- Capture: keep a DXGI desktop duplication open (as `hdr` already does for one HDR
  frame), reading 8-bit frames at a fixed rate and cropping to the region. One monitor
  only; `xcap`'s whole-monitor capture is too slow per frame.
- Recording flow (the largest part and the main risk): select the region on the frozen
  overlay, then the overlay closes and a click-through border window, kept out of the
  recording with `WDA_EXCLUDEFROMCAPTURE`, shows the region with a stop button and a
  timer; the shortcut or the tray also stops it. The capture process lives on while
  recording, with a length limit.
- Memory, not disk: raw frames are large (30 s of 800×600 at 15 fps is about 860 MB),
  and a temp file would break "only the clipboard or a file you choose", so frames are
  kept in memory as changed areas and encoded after stopping, which also allows the
  shared palette and trimming.
- Save only: most apps take a pasted GIF only as a file, which would need one on disk.
  Optional extras are trimming and drawing static annotations onto every frame. The
  restrictive build might leave recording out.

## Done

- Renamed from Darkshot to Duskshot, because another screenshot app already used the
  name. The exe, settings file (`duskshot.toml`), `%APPDATA%\Duskshot` folder, registry
  entries and repository all follow. There is no migration: to carry settings over,
  rename the old `darkshot.toml` and turn Start with Windows and Open with off in the
  old version first.
- Version 1.4.0:
  - The emoji stamp has a full emoji picker: recent emoji, a tab per group, and a
    search by name, tag or shortcode (Unicode's list and GitHub's gemoji). Kaomoji
    and symbol pickers stamp in the current colour.
  - Right-click backs out one step at a time (a panel, the selection, the tool)
    instead of drawing; only the left button draws.
  - Shortcut and tool keys follow the keyboard layout, shown as printed on the key.
  - Settings rearranged: shortcuts and tool keys on the left, the rest on the right.
    Tool help is off by default.
  - A click on the tray or the shortcut while a capture is open brings it to the front.
  - Graphics through Direct3D 12 alone (other backends only without a Direct3D 12
    GPU) with a memory-saving allocator: about a fifth of the GPU memory of before.
  - Reopen last capture: a closing capture hands its state, compressed, to the tray
    and exits, instead of staying open hidden (a few MB instead of hundreds). Turning
    the option off lets go of a kept capture at once.
- Version 1.3.0:
  - The hotkey strip wraps onto more rows instead of running off the screen past 95%
    of its width; groups stay together on one row when they fit on one.
  - Compact tool help shows everything but the keys and modifiers, which the strip
    already shows.
  - An opened image that fits gets a window a little larger than itself instead of
    covering the screen. Opened images zoom (Ctrl+wheel, around the pointer) and pan
    (middle-drag or Space+drag); Ctrl+0 fits them again.
  - A shortcut for Edit clipboard image, set in Settings.
  - Open with in Explorer for PNG and JPEG (personal build): registered for the
    current user only, never as the default app.
  - Reopen last capture (opt-in): an overlay started from the tray hides when done
    and keeps everything in its own memory, never on disk, for 1-60 minutes. The tray
    menu or a shortcut brings it back; the next capture or the time ends it.
- Version 1.2.0: emoji stamps, keys chosen per tool, Ctrl+drag moves a drawing with any
  tool, sizes remembered per tool, Enter finishes a text (Shift+Enter for a new line),
  and help on screen: the hotkey strip, the tool box beside the region, the F1 sheet
  and the Tool help setting.
- Edit an existing image: a PNG or JPEG (dragged onto the exe, `duskshot.exe
  picture.png`, or Open image… in the tray menu) or the clipboard image (Edit clipboard
  image) opens in the overlay with the whole image selected. No window snapping; Save
  suggests the original name plus `-edited`, in its folder and format. Large images are
  shown scaled to fit, with annotations, copies and saves in image pixels. Photos are
  turned upright from their EXIF orientation.
- Copy text reads again at a few other sizes when Windows finds nothing: its
  recognition misses some short codes at particular sizes and reads them at others.
- Copy text (O) and Read QR code (Q): read the region's text with Windows' built-in
  offline OCR (`Windows.Media.Ocr`), or decode its QR codes locally (`rqrr`). Results
  show in an editable panel; each line or code is outlined on the capture and can be
  clicked to copy only it (Shift+click for single words). Blurs and redactions apply
  before reading. Text goes to the clipboard kept out of history and cloud sync; links
  are never opened. Both can be switched off under Enabled tools.
- Copied images go on the clipboard twice: as PNG with transparency (a drop shadow
  keeps its soft edge in programs that read PNG) and as a bitmap on white for the rest.
  Both are kept out of clipboard history and cloud sync; the clipboard library used
  before did that for text but not for images.
- The colour picker shows the same hue ring and saturation/value square in every tab
  (the RGB and CMYK wheels went dark and grey with the colour). The border colour can
  be see-through.
- Turn the capture: a round handle above the region turns the captured screen under it,
  so a photo at an angle (a receipt) can be straightened before annotating. Copy, save,
  blur, Copy text and the eyedropper all see the straightened image. Turns freely,
  Shift for 15° steps. Annotations stay put while the capture turns under them,
  so it can be corrected after marking up; blur follows what is under it, live.
  Right-click the handle to reset. Turned by hand, Copy text reads as it is, without
  trying other ways up.
- Read QR code reads styled codes (dots, colour, ornaments): when the plain read fails,
  the region is tried as hard black and white and enlarged and softened. Moving,
  resizing or turning with the reading panel open reads the region again.
- Copy text reads text any way up: Windows straightens tilts itself (up to about 40°);
  when the region reads as nothing or gibberish, it is read again turned a quarter, half
  and three-quarter turn, and the reading that looks most like words is kept.
- Version and a link to the GitHub repository in Settings. The link is opened by the
  browser, only when clicked; Duskshot itself still connects to nothing.
- Include the mouse cursor (a setting, off by default): the cursor is drawn into the
  capture where it was when the capture was asked for.
- Ask before discarding (a setting, off by default): Esc or Close asks first when a
  capture has annotations. Enter discards, Esc goes back.
- Decoration: a toolbar button for a border and a drop shadow around the image, set
  per capture. The border has a colour, a thickness and a line style like any shape
  (switching it on takes the toolbar's colour and line style) and is previewed around
  the region. Both are added outside the image on copy and save. What they leave
  uncovered is transparent in a PNG file and white on the clipboard and in a JPEG.
  Settings choose what each capture starts with; both are off by default.
- HDR monitors: a monitor with HDR switched on is read in its own format (linear
  16-bit floats, through DXGI desktop duplication) and converted to sRGB using
  Windows' SDR brightness setting, instead of the ordinary capture, which looks washed
  out. Other monitors are captured as before. Not yet tried on an HDR display.
- Tray icon and global capture shortcut; one overlay spanning all monitors.
- Capture region with resize handles, move, arrow-key nudge, Ctrl+A for the current
  monitor; Shift constrains, Alt works from the centre.
- Release build: the app icon embedded in the exe and used for the tray and settings
  window; whole-program optimisation; eframe trimmed to the features Duskshot uses, so
  no URL-opening code (and no networking crate at all) is compiled in.
- Numbered step markers (N): click to place; each new one takes the next number.
- Eyedropper (I): picks a colour from the capture itself. A round spotlight shows the
  undimmed capture around the cursor (hold Shift to magnify it to single pixels), with
  the colour's hex, RGB and HSL values beside it.
- Restrictive and personal builds from one source (see the README). The default
  restrictive build stores settings only beside the exe and writes no diagnostic logs;
  `--features personal` adds the rest.
- No keyboard hook in either build, and no synthesized key presses: the capture
  shortcut is always a registered hotkey, and shortcuts are recorded from the focused
  settings window. (The personal build used to take shortcuts over with a low-level
  hook, which had to send a masking key press to keep the Start menu shut; anti-cheat
  software can read that as automated input.)
- Free a Windows shortcut (personal build): a setting that adds the shortcut's letter
  to `DisabledHotkeys`, so Win+Shift+S can be registered normally, with a button to
  restart Explorer and a way to give letters back.
- Enabled tools: Settings lists every tool with its icon; unticked ones leave the
  toolbar and their keys do nothing. All are on by default.
- Start with Windows (personal build): a Settings checkbox that adds Duskshot to the
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
- Settings file next to the exe (the personal build falls back to `%APPDATA%\Duskshot`)
  and a settings window in two columns: shortcut (including the Windows key), default
  colours, light, dark or system theme, file name, format, quality and default folder,
  with the enabled tools on the right. The window can be resized and scrolls; Save and
  Cancel stay at the bottom.
- Win+Shift+S is the default shortcut (once freed from Windows).
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
- Version 1.0.0. Releases are built and published by GitHub Actions from a version tag,
  with SHA-256 checksums and build provenance attestations.
