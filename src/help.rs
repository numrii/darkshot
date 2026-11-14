//! On-screen help in the capture overlay:
//!
//! - **The hotkey strip** at the bottom of the screen: keys on top, what they do
//!   underneath (a tool's symbol, or a word). It always shows.
//! - **The tool box** beside the selection, explaining the current tool: what it does,
//!   what clicking and dragging do, its keys and modifiers, and its toolbar options. The
//!   help level setting shows all of it, only the keys, or no box at all.
//! - **The shortcut sheet** (F1, or ?), with everything in one place.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, RichText, Stroke, StrokeKind, Vec2, vec2};
use serde::{Deserialize, Serialize};

use crate::icons::{self, Icon};

/// How much the tool box shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HelpLevel {
    /// What the tool does, its clicks and drags, keys and modifiers, toolbar options.
    #[default]
    Full,
    /// Only the keys and modifiers.
    Compact,
    /// No tool box; the strip at the bottom stays.
    Off,
}

/// What the box says about one tool. Keys are written as they are pressed together,
/// such as `["Ctrl", "drag"]`.
pub struct ToolHelp {
    pub name: &'static str,
    pub does: &'static str,
    pub acts: &'static [(&'static str, &'static str)],
    pub mods: &'static [(&'static [&'static str], &'static str)],
    pub opts: Option<&'static str>,
}

const MOVE: (&[&str], &str) = (&["Ctrl", "drag"], "Move element");

/// The help for the tool with id `id` (as in [`crate::tools::TOOLS`]).
pub fn tool_help(id: &str) -> Option<ToolHelp> {
    let help = match id {
        "select" => ToolHelp {
            name: "Select and move",
            does: "Move, resize or change what you've drawn, or the region itself.",
            acts: &[
                ("Drag a shape", "Move it"),
                ("Drag its handles", "Resize it, or move a line's ends"),
                ("Drag empty space", "Move the region"),
                ("Drag the region's edges", "Resize the region"),
            ],
            mods: &[
                (&["Shift"], "Keep to one direction while moving, or the proportions while resizing"),
                (&["Alt"], "Resize from the centre"),
                (&["←", "→", "↑", "↓"], "Nudge 1 px, with Shift 10 px"),
                (&["Wheel"], "Thickness or size of the selected shape"),
                (&["Del"], "Delete the selected shape"),
                (&["Esc"], "Deselect"),
            ],
            opts: None,
        },
        "rect" => ToolHelp {
            name: "Rectangle",
            does: "Draws a rectangle outline.",
            acts: &[("Drag", "Draw it")],
            mods: &[(&["Shift"], "Square"), (&["Alt"], "Draw from the centre"), MOVE, (&["Wheel"], "Thickness")],
            opts: Some("Colour · line style: solid, dashed, dotted, dash-dot"),
        },
        "ellipse" => ToolHelp {
            name: "Ellipse",
            does: "Draws an ellipse outline.",
            acts: &[("Drag", "Draw it")],
            mods: &[(&["Shift"], "Circle"), (&["Alt"], "Draw from the centre"), MOVE, (&["Wheel"], "Thickness")],
            opts: Some("Colour · line style"),
        },
        "line" => ToolHelp {
            name: "Line",
            does: "Draws a straight line.",
            acts: &[("Drag", "Draw it")],
            mods: &[(&["Shift"], "Snap to 45° steps"), (&["Alt"], "Draw from the centre"), MOVE, (&["Wheel"], "Thickness")],
            opts: Some("Colour · line style"),
        },
        "arrow" => ToolHelp {
            name: "Arrow",
            does: "Draws an arrow, pointing where you let go.",
            acts: &[("Drag", "Draw it")],
            mods: &[(&["Shift"], "Snap to 45° steps"), (&["Alt"], "Draw from the centre"), MOVE, (&["Wheel"], "Thickness")],
            opts: Some("Colour · line style · arrowhead: triangle, open, stealth, dot · heads on both ends"),
        },
        "pen" => ToolHelp {
            name: "Pen",
            does: "Draws freehand.",
            acts: &[("Drag", "Draw")],
            mods: &[(&["Shift"], "Snap to 45° steps"), MOVE, (&["Wheel"], "Thickness")],
            opts: Some("Colour · line style"),
        },
        "highlighter" => ToolHelp {
            name: "Highlighter",
            does: "Draws a see-through stroke over text, with its own colour and width.",
            acts: &[("Drag", "Highlight")],
            mods: &[(&["Shift"], "Snap to 45° steps"), MOVE, (&["Wheel"], "Thickness")],
            opts: Some("Colour"),
        },
        "text" => ToolHelp {
            name: "Text",
            does: "Places text you type.",
            acts: &[("Click", "Place text and start typing"), ("Click a text", "Edit it")],
            mods: &[
                (&["Enter"], "Finish (or click outside)"),
                (&["Shift", "Enter"], "New line"),
                MOVE,
                (&["Wheel"], "Font size"),
            ],
            opts: Some("Colour · style: plain, outline, box"),
        },
        "blur" => ToolHelp {
            name: "Blur",
            does: "Hides what's under it. It always works on the capture itself, under everything you draw.",
            acts: &[("Drag", "Cover an area")],
            mods: &[(&["Shift"], "Square"), (&["Alt"], "Draw from the centre"), MOVE, (&["Wheel"], "Strength")],
            opts: Some("Pixelate · blur · redact: a solid fill nothing can be recovered from"),
        },
        "step" => ToolHelp {
            name: "Numbered steps",
            does: "Places numbered markers: 1, 2, 3…",
            acts: &[("Click", "Place the next number"), ("Drag", "Place it where you let go")],
            mods: &[MOVE, (&["Wheel"], "Size")],
            opts: Some("Colour: a placed step stays unselected, so a new colour is for the next one"),
        },
        "emoji" => ToolHelp {
            name: "Emoji stamp",
            does: "Stamps an emoji.",
            acts: &[("Click", "Place the emoji"), ("Drag", "Place it where you let go")],
            mods: &[MOVE, (&["Wheel"], "Size")],
            opts: Some(
                "Emoji: pick a favourite, or press Win+. in its field for any emoji. A placed stamp stays \
                 unselected, so a new emoji is for the next one",
            ),
        },
        "eyedropper" => ToolHelp {
            name: "Pick a colour",
            does: "Takes a colour from the capture for drawing.",
            acts: &[("Click", "Pick the colour under the cursor")],
            mods: &[(&["Shift"], "Magnify to single pixels"), (&["Esc"], "Cancel")],
            opts: Some("Shows hex, RGB and HSL as you move"),
        },
        "read-text" => ToolHelp {
            name: "Copy text",
            does: "Reads the text in the region into a panel. Blurred and redacted parts stay hidden.",
            acts: &[
                ("Click a line", "Copy only the lines you click"),
                ("Shift+click a word", "Copy only that word"),
                ("Move or resize the region", "Read again"),
            ],
            mods: &[(&["Ctrl", "Enter"], "Copy and close"), (&["Esc"], "Close the panel")],
            opts: Some("You can edit the text in the panel before copying"),
        },
        "read-qr" => ToolHelp {
            name: "Read QR code",
            does: "Reads every QR code in the region. Links are copied as text, never opened.",
            acts: &[("Click a code", "Copy only that one"), ("Move or resize the region", "Read again")],
            mods: &[(&["Ctrl", "Enter"], "Copy and close"), (&["Esc"], "Close the panel")],
            opts: None,
        },
        _ => return None,
    };
    Some(help)
}

// --- The tool box ---------------------------------------------------------------------

/// Width of the tool box, in points.
pub const BOX_WIDTH: f32 = 290.0;
const GAP: f32 = 14.0;
const MARGIN: f32 = 8.0;

/// Where the tool box goes, for a box of `size` beside the selection `sel`, on
/// `monitor`, keeping clear of `avoid` (the toolbar, the round handle, open panels) and
/// of everything below `floor` (the strip). In order: right of the selection, left of
/// it, below, above. With no room outside, it docks inside the selection, in the
/// bottom-left corner, or the top-right one while `pointer` is near the bottom-left.
/// Returns the top-left corner, and whether it docked inside.
pub fn place_box(size: Vec2, sel: Rect, monitor: Rect, avoid: &[Rect], floor: f32, pointer: Option<Pos2>) -> (Pos2, bool) {
    let bottom = floor.min(monitor.bottom()) - MARGIN;
    let clamp_y = |y: f32| y.min(bottom - size.y).max(monitor.top() + MARGIN);
    let clamp_x = |x: f32| x.min(monitor.right() - MARGIN - size.x).max(monitor.left() + MARGIN);
    let fits = |at: Pos2| {
        let r = Rect::from_min_size(at, size);
        r.left() >= monitor.left() + MARGIN - 0.5
            && r.right() <= monitor.right() - MARGIN + 0.5
            && r.top() >= monitor.top() + MARGIN - 0.5
            && r.bottom() <= bottom + 0.5
            && !avoid.iter().any(|a| a.intersects(r))
    };
    // Below the selection, or below whatever sits under it (the toolbar).
    let below = {
        let at = Pos2::new(clamp_x(sel.left()), sel.bottom() + GAP);
        let r = Rect::from_min_size(at, size);
        match avoid.iter().filter(|a| a.intersects(r)).map(|a| a.bottom()).reduce(f32::max) {
            Some(b) => Pos2::new(at.x, b + GAP),
            None => at,
        }
    };
    let above = {
        let at = Pos2::new(clamp_x(sel.left()), sel.top() - GAP - size.y);
        let r = Rect::from_min_size(at, size);
        match avoid.iter().filter(|a| a.intersects(r)).map(|a| a.top()).reduce(f32::min) {
            Some(t) => Pos2::new(at.x, t - GAP - size.y),
            None => at,
        }
    };
    let candidates = [
        Pos2::new(sel.right() + GAP, clamp_y(sel.top())),
        Pos2::new(sel.left() - GAP - size.x, clamp_y(sel.top())),
        below,
        above,
    ];
    if let Some(at) = candidates.into_iter().find(|&at| fits(at)) {
        return (at, false);
    }
    // Inside, bottom-left, moved up past anything in the way there (the toolbar).
    let inside = sel.intersect(monitor).shrink(12.0);
    let mut at = Pos2::new(inside.left(), inside.bottom().min(bottom) - size.y);
    if let Some(t) = avoid.iter().filter(|a| a.intersects(Rect::from_min_size(at, size))).map(|a| a.top()).reduce(f32::min) {
        at.y = t - GAP - size.y;
    }
    if pointer.is_some_and(|p| Rect::from_min_size(at, size).expand(40.0).contains(p)) {
        at = Pos2::new(inside.right() - size.x, inside.top());
    }
    (Pos2::new(clamp_x(at.x), clamp_y(at.y)), true)
}

/// Show keys as small keycaps, in a row.
fn keycaps(ui: &mut egui::Ui, keys: &[&str]) {
    ui.spacing_mut().item_spacing.x = 3.0;
    let fill = ui.visuals().widgets.inactive.bg_fill;
    for key in keys {
        ui.label(RichText::new(format!(" {key} ")).monospace().size(11.0).background_color(fill));
    }
}

/// Draw the tool box for `help` (picked with `key`) at `at`. Returns its size, for
/// placing it next frame.
pub fn tool_box(ctx: &egui::Context, help: &ToolHelp, key: Option<&str>, level: HelpLevel, at: Pos2, docked: bool) -> Vec2 {
    let response = egui::Area::new(egui::Id::unique("darkshot-tool-help"))
        .order(egui::Order::Middle)
        .fixed_pos(at)
        .interactable(false)
        .show(ctx, |ui| {
            let style = ui.style().clone();
            let mut frame = egui::Frame::popup(&style);
            if docked {
                // See-through, so the capture under it still shows.
                frame = frame.fill(style.visuals.window_fill.gamma_multiply(0.72));
            }
            frame.show(ui, |ui| {
                ui.set_width(BOX_WIDTH - 2.0 * frame.inner_margin.leftf() - 2.0);
                let weak = ui.visuals().weak_text_color();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(help.name).strong());
                    if let Some(key) = key {
                        keycaps(ui, &[key]);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new("F1 help").small().color(weak));
                    });
                });
                if level == HelpLevel::Full {
                    ui.add(egui::Label::new(RichText::new(help.does).color(weak)).wrap());
                    ui.add_space(2.0);
                    egui::Grid::new("help-acts").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                        for &(what, does) in help.acts {
                            ui.label(RichText::new(what).strong());
                            ui.add(egui::Label::new(does).wrap());
                            ui.end_row();
                        }
                    });
                    ui.add_space(4.0);
                    ui.label(RichText::new("Keys and modifiers").small().color(weak));
                }
                egui::Grid::new("help-mods").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                    for &(keys, does) in help.mods {
                        ui.horizontal(|ui| keycaps(ui, keys));
                        ui.add(egui::Label::new(does).wrap());
                        ui.end_row();
                    }
                });
                if let (HelpLevel::Full, Some(opts)) = (level, help.opts) {
                    ui.add_space(4.0);
                    ui.label(RichText::new("In the toolbar").small().color(weak));
                    ui.add(egui::Label::new(opts).wrap());
                }
            });
        });
    response.response.rect.size()
}

// --- The strip ------------------------------------------------------------------------

/// One entry in the strip: its keys, and what they do.
pub struct Entry {
    pub keys: Vec<String>,
    pub what: What,
    /// The current tool: drawn lit up.
    pub active: bool,
}

pub enum What {
    Word(&'static str),
    Tool(Icon),
}

impl Entry {
    pub fn new(keys: &[&str], word: &'static str) -> Self {
        Self { keys: keys.iter().map(|k| (*k).to_owned()).collect(), what: What::Word(word), active: false }
    }
}

const STRIP_FONT: f32 = 11.0;
const WORD_FONT: f32 = 12.0;
const CAP_HEIGHT: f32 = 17.0;
const ROW_GAP: f32 = 4.0;
const WHAT_HEIGHT: f32 = 16.0;
/// The strip's full height, in points.
pub const STRIP_HEIGHT: f32 = CAP_HEIGHT + ROW_GAP + WHAT_HEIGHT + 14.0;

/// Draw the strip, its bottom edge centred on `bottom`. Groups are set apart by a thin
/// line. Returns the strip's rectangle.
pub fn strip(painter: &egui::Painter, bottom: Pos2, groups: &[Vec<Entry>]) -> Rect {
    let white = Color32::WHITE;
    let cap_font = FontId::monospace(STRIP_FONT);
    let word_font = FontId::proportional(WORD_FONT);
    // Lay out every column first, to know the width.
    struct Column {
        caps: Vec<(std::sync::Arc<egui::Galley>, f32)>,
        word: Option<std::sync::Arc<egui::Galley>>,
        icon: Option<Icon>,
        active: bool,
        width: f32,
    }
    let columns: Vec<Vec<Column>> = groups
        .iter()
        .map(|group| {
            group
                .iter()
                .map(|e| {
                    let caps: Vec<_> = e
                        .keys
                        .iter()
                        .map(|k| {
                            let g = painter.layout_no_wrap(k.clone(), cap_font.clone(), white);
                            let w = (g.size().x + 8.0).max(CAP_HEIGHT);
                            (g, w)
                        })
                        .collect();
                    let caps_width = caps.iter().map(|(_, w)| w).sum::<f32>() + 3.0 * (caps.len().saturating_sub(1)) as f32;
                    let (word, icon, what_width) = match e.what {
                        What::Word(w) => {
                            let g = painter.layout_no_wrap(w.to_owned(), word_font.clone(), Color32::from_gray(215));
                            let width = g.size().x;
                            (Some(g), None, width)
                        }
                        What::Tool(icon) => (None, Some(icon), WHAT_HEIGHT),
                    };
                    Column { caps, word, icon, active: e.active, width: caps_width.max(what_width) }
                })
                .collect()
        })
        .collect();
    let inner_gap = |group: &[Column]| if group.iter().all(|c| c.icon.is_some()) { 12.0 } else { 18.0 };
    let group_width = |group: &[Column]| {
        group.iter().map(|c| c.width).sum::<f32>() + inner_gap(group) * group.len().saturating_sub(1) as f32
    };
    let separator = 37.0; // gap, line, gap
    let width = columns.iter().map(|g| group_width(g)).sum::<f32>() + separator * columns.len().saturating_sub(1) as f32;
    let height = CAP_HEIGHT + ROW_GAP + WHAT_HEIGHT;
    let pad = vec2(14.0, 7.0);
    let rect = Rect::from_min_size(bottom - vec2(width / 2.0 + pad.x, height + 2.0 * pad.y), vec2(width, height) + 2.0 * pad);
    painter.rect_filled(rect, 6.0, Color32::from_black_alpha(200));

    let cap_fill = Color32::from_white_alpha(26);
    let cap_line = Stroke::new(1.0, Color32::from_white_alpha(51));
    let mut x = rect.left() + pad.x;
    let top = rect.top() + pad.y;
    for (i, group) in columns.iter().enumerate() {
        if i > 0 {
            let lx = x + separator / 2.0;
            painter.line_segment([Pos2::new(lx, top), Pos2::new(lx, top + height)], Stroke::new(1.0, Color32::from_white_alpha(51)));
            x += separator;
        }
        let gap = inner_gap(group);
        for (j, c) in group.iter().enumerate() {
            if j > 0 {
                x += gap;
            }
            if c.active {
                let lit = Rect::from_min_size(Pos2::new(x, top), vec2(c.width, height)).expand2(vec2(4.0, 3.0));
                painter.rect_filled(lit, 4.0, Color32::from_rgba_unmultiplied(80, 150, 255, 90));
                painter.rect_stroke(lit, 4.0, Stroke::new(1.0, Color32::from_rgb(110, 170, 255)), StrokeKind::Inside);
            }
            let caps_width = c.caps.iter().map(|(_, w)| w).sum::<f32>() + 3.0 * (c.caps.len().saturating_sub(1)) as f32;
            let mut cx = x + (c.width - caps_width) / 2.0;
            for (g, w) in &c.caps {
                let cap = Rect::from_min_size(Pos2::new(cx, top), vec2(*w, CAP_HEIGHT));
                painter.rect_filled(cap, 3.0, cap_fill);
                painter.rect_stroke(cap, 3.0, cap_line, StrokeKind::Inside);
                painter.galley(cap.center() - g.size() / 2.0, g.clone(), white);
                cx += w + 3.0;
            }
            let what = Rect::from_min_size(Pos2::new(x, top + CAP_HEIGHT + ROW_GAP), vec2(c.width, WHAT_HEIGHT));
            if let Some(g) = &c.word {
                painter.galley(what.center() - g.size() / 2.0, g.clone(), white);
            }
            if let Some(icon) = c.icon {
                let area = Rect::from_center_size(what.center(), Vec2::splat(WHAT_HEIGHT));
                icons::paint(painter, area, icon, Color32::from_gray(225), Color32::BLACK);
            }
            x += c.width;
        }
    }
    rect
}

// --- The shortcut sheet ---------------------------------------------------------------

/// Draw the shortcut sheet in the middle of `monitor`. `tools` are the enabled tools
/// with their keys.
pub fn sheet(ctx: &egui::Context, monitor: Rect, tools: &[(&'static str, String)]) {
    egui::Area::new(egui::Id::unique("darkshot-help-sheet"))
        .order(egui::Order::Foreground)
        .pivot(Align2::CENTER_CENTER)
        .fixed_pos(monitor.center())
        .constrain_to(monitor)
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).inner_margin(16.0).show(ui, |ui| {
                let weak = ui.visuals().weak_text_color();
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Shortcuts").strong().size(16.0));
                    ui.add_space(12.0);
                    ui.label(RichText::new("Only tools that are switched on are listed · any key or click closes this").color(weak));
                });
                ui.add_space(8.0);
                let tool_rows: Vec<(Vec<String>, String)> =
                    tools.iter().map(|(name, key)| (vec![key.clone()], (*name).to_owned())).collect();
                let fixed = |rows: &[(&[&str], &str)]| -> Vec<(Vec<String>, String)> {
                    rows.iter().map(|(k, t)| (k.iter().map(|s| (*s).to_owned()).collect(), (*t).to_owned())).collect()
                };
                let groups: [(&str, Vec<(Vec<String>, String)>); 6] = [
                    (
                        "Region",
                        fixed(&[
                            (&["Drag"], "Select an area"),
                            (&["Ctrl", "click"], "Select a window"),
                            (&["Ctrl"], "Hold to snap edges"),
                            (&["Ctrl", "A"], "Whole screen (or image)"),
                            (&["Shift"], "Keep proportions"),
                            (&["Alt"], "Resize from the centre"),
                            (&["←", "→"], "Nudge, Shift ×10"),
                            (&["↻"], "Turn: drag the round handle, Shift 15°, right-click resets"),
                        ]),
                    ),
                    ("Tools", tool_rows),
                    (
                        "Drawing",
                        fixed(&[
                            (&["Shift"], "Square, circle, 45° or straight"),
                            (&["Alt"], "From the centre"),
                            (&["Ctrl", "drag"], "Move element, with any tool"),
                            (&["Wheel"], "Thickness or size (each tool keeps its own)"),
                            (&["Del"], "Delete selected"),
                            (&["Esc"], "Deselect"),
                            (&["Ctrl", "Z"], "Undo"),
                            (&["Ctrl", "Y"], "Redo (or Ctrl+Shift+Z)"),
                        ]),
                    ),
                    (
                        "Copy and save",
                        fixed(&[
                            (&["Ctrl", "C"], "Copy"),
                            (&["Enter"], "Copy"),
                            (&["Ctrl", "S"], "Save"),
                            (&["Esc"], "Close (asks first if set)"),
                        ]),
                    ),
                    (
                        "Reading",
                        fixed(&[
                            (&["click"], "Pick a line or code"),
                            (&["Shift", "click"], "Pick a word"),
                            (&["Ctrl", "Enter"], "Copy and close"),
                        ]),
                    ),
                    ("Help", fixed(&[(&["F1"], "This sheet (or ?)"), (&["any key"], "Close it")])),
                ];
                ui.columns(3, |cols| {
                    for (i, (title, rows)) in groups.iter().enumerate() {
                        let ui = &mut cols[i % 3];
                        ui.label(RichText::new(*title).strong());
                        egui::Grid::new(("help-sheet", i)).num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                            for (keys, text) in rows {
                                ui.horizontal(|ui| keycaps(ui, &keys.iter().map(String::as_str).collect::<Vec<_>>()));
                                ui.label(text);
                                ui.end_row();
                            }
                        });
                        ui.add_space(10.0);
                    }
                });
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITOR: Rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(1280.0, 720.0));
    const SIZE: Vec2 = vec2(290.0, 200.0);
    const FLOOR: f32 = 640.0;

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1))
    }

    #[test]
    fn every_tool_has_help() {
        for tool in &crate::tools::TOOLS {
            let help = tool_help(tool.id).unwrap_or_else(|| panic!("{}", tool.id));
            assert!(!help.mods.is_empty(), "{}", tool.id);
        }
    }

    #[test]
    fn box_goes_right_then_left_then_below_then_above() {
        // Room on the right.
        let (at, docked) = place_box(SIZE, rect(100.0, 100.0, 500.0, 400.0), MONITOR, &[], FLOOR, None);
        assert_eq!((at, docked), (Pos2::new(514.0, 100.0), false));
        // At the right edge: on the left.
        let (at, _) = place_box(SIZE, rect(900.0, 100.0, 1270.0, 400.0), MONITOR, &[], FLOOR, None);
        assert_eq!(at.x, 900.0 - 14.0 - 290.0);
        // Wide: below, under the toolbar.
        let toolbar = rect(800.0, 408.0, 1150.0, 446.0);
        let (at, docked) = place_box(SIZE, rect(50.0, 40.0, 1250.0, 400.0), MONITOR, &[toolbar], FLOOR, None);
        assert!(!docked);
        assert_eq!(at, Pos2::new(50.0, 414.0));
        // Wide and low: above.
        let (at, _) = place_box(SIZE, rect(50.0, 300.0, 1250.0, 620.0), MONITOR, &[], FLOOR, None);
        assert_eq!(at.y, 300.0 - 14.0 - 200.0);
    }

    #[test]
    fn whole_screen_docks_inside_and_dodges_the_pointer() {
        let (at, docked) = place_box(SIZE, MONITOR, MONITOR, &[], FLOOR, None);
        assert!(docked);
        assert_eq!(at.x, 12.0);
        assert!(at.y + SIZE.y <= FLOOR);
        let (moved, _) = place_box(SIZE, MONITOR, MONITOR, &[], FLOOR, Some(at + vec2(10.0, 10.0)));
        assert!(moved.x > 600.0 && moved.y < 100.0, "{moved:?}");
    }

    #[test]
    fn box_keeps_clear_of_things_to_avoid() {
        let handle = rect(250.0, 50.0, 280.0, 80.0);
        let sel = rect(50.0, 100.0, 1250.0, 620.0);
        let (at, docked) = place_box(SIZE, sel, MONITOR, &[handle], FLOOR, None);
        assert!(docked || !Rect::from_min_size(at, SIZE).intersects(handle));
    }
}
