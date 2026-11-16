//! The settings window (`duskshot --settings`), opened from the tray menu.

use eframe::egui::{self, Color32, Key, RichText, Sense, Vec2};

use crate::save::{Format, SaveOptions, QUALITY_HIGH, QUALITY_MEDIUM};
use crate::settings::{
    self, parse_hex, shortcut_text, shortcut_valid, to_hex, FormatSetting, Settings, ThemeSetting, DEFAULT_HOTKEY,
    DEFAULT_MARKER_COLOR, DEFAULT_SWATCHES,
};
use crate::tools::{ToolInfo, TOOLS};
use crate::AnyResult;

use recorder::Recorder;

const VK_ESCAPE: u32 = 0x1B;
/// Errors and warnings.
const ERROR_RED: Color32 = Color32::from_rgb(0xE5, 0x48, 0x4D);

/// Recording a shortcut from this window's own keyboard input, so only keys pressed
/// while it has focus are seen. Combinations Windows keeps for itself never arrive
/// (until they are freed from Windows, with `free-shortcut`). No keyboard hook.
mod recorder {
    use eframe::egui::{self, Key};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_LWIN, VK_RWIN};

    use crate::settings::{MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_WIN};

    pub struct Recorder;

    impl Recorder {
        pub fn start() -> Option<Self> {
            Some(Self)
        }

        /// The combination pressed in this window, as (virtual-key code, `MOD_*` bits).
        /// The key event is consumed, so it does not also press the focused button.
        pub fn recorded(&self, ctx: &egui::Context) -> Option<(u32, u8)> {
            ctx.input_mut(|input| {
                let found = input.events.iter().find_map(|event| match event {
                    // Windows reports only the release of PrintScreen.
                    egui::Event::Key { key, physical_key, pressed, modifiers, .. }
                        if *pressed || *key == Key::PrintScreen =>
                    {
                        // The key's own code in this layout: egui may name Ö after the
                        // US key in its place.
                        let vk = physical_key.and_then(crate::keys::virtual_key_at).or_else(|| vk_code(*key))?;
                        let mut mods = 0;
                        for (held, bit) in [(modifiers.ctrl, MOD_CTRL), (modifiers.alt, MOD_ALT), (modifiers.shift, MOD_SHIFT)] {
                            if held {
                                mods |= bit;
                            }
                        }
                        // egui has no Windows-key modifier: ask the state of this
                        // thread's own input, as of the key event being handled.
                        let down = |vk: u16| unsafe { GetKeyState(i32::from(vk)) } < 0;
                        if down(VK_LWIN) || down(VK_RWIN) {
                            mods |= MOD_WIN;
                        }
                        Some((vk, mods))
                    }
                    _ => None,
                });
                if found.is_some() {
                    input.events.clear();
                }
                found
            })
        }
    }

    /// The virtual-key code for keys that can be part of a shortcut.
    fn vk_code(key: Key) -> Option<u32> {
        let name = key.name();
        if let [c] = name.as_bytes() {
            if c.is_ascii_uppercase() || c.is_ascii_digit() {
                return Some(u32::from(*c));
            }
        }
        if let Some(n) = name.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
            return (1..=24).contains(&n).then_some(0x70 + n - 1);
        }
        Some(match key {
            Key::PrintScreen => 0x2C,
            Key::Pause => 0x13,
            Key::Insert => 0x2D,
            Key::Delete => 0x2E,
            Key::Home => 0x24,
            Key::End => 0x23,
            Key::PageUp => 0x21,
            Key::PageDown => 0x22,
            Key::Space => 0x20,
            Key::Tab => 0x09,
            Key::Enter => 0x0D,
            Key::Escape => 0x1B,
            Key::Backspace => 0x08,
            Key::ArrowLeft => 0x25,
            Key::ArrowUp => 0x26,
            Key::ArrowRight => 0x27,
            Key::ArrowDown => 0x28,
            Key::Minus => 0xBD,
            Key::Equals => 0xBB,
            Key::Comma => 0xBC,
            Key::Period => 0xBE,
            Key::Slash => 0xBF,
            Key::Backtick => 0xC0,
            Key::Semicolon => 0xBA,
            Key::Quote => 0xDE,
            Key::OpenBracket => 0xDB,
            Key::Backslash => 0xDC,
            Key::CloseBracket => 0xDD,
            _ => return None,
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn key_codes() {
            assert_eq!(vk_code(Key::S), Some(u32::from(b'S')));
            assert_eq!(vk_code(Key::Num7), Some(u32::from(b'7')));
            assert_eq!(vk_code(Key::F1), Some(0x70));
            assert_eq!(vk_code(Key::F9), Some(0x78));
            assert_eq!(vk_code(Key::F24), Some(0x87));
            assert_eq!(vk_code(Key::F25), None);
            assert_eq!(vk_code(Key::PrintScreen), Some(0x2C));
            assert_eq!(vk_code(Key::ArrowDown), Some(0x28));
            assert_eq!(vk_code(Key::CapsLock), None);
        }
    }
}

pub fn run() -> AnyResult<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Duskshot settings")
            .with_icon(crate::appicon::window_icon())
            .with_inner_size([880.0, 700.0])
            .with_min_inner_size([680.0, 300.0])
            .with_resizable(true),
        wgpu_options: crate::overlay::gpu_options(eframe::egui_wgpu::SurfaceConfig::HIGH_THROUGHPUT),
        ..Default::default()
    };
    eframe::run_native(
        "Duskshot settings",
        options,
        Box::new(|cc| {
            crate::overlay::setup_text(&cc.egui_ctx);
            Ok(Box::new(SettingsApp::new()))
        }),
    )?;
    Ok(())
}

/// A shortcut set in this window.
#[derive(Clone, Copy, PartialEq)]
enum Which {
    Capture,
    Clipboard,
    Reopen,
}

impl Which {
    fn name(self) -> &'static str {
        match self {
            Which::Capture => "capture",
            Which::Clipboard => "Edit clipboard image",
            Which::Reopen => "Reopen last capture",
        }
    }
}

struct SettingsApp {
    settings: Settings,
    saved: Settings,
    /// While recording a shortcut: what catches it.
    recorder: Option<Recorder>,
    /// The shortcut being recorded.
    recording_for: Which,
    /// Whether Explorer offers Duskshot under Open with. Kept in the registry, not the
    /// settings file, and changed as soon as the box is ticked.
    #[cfg(feature = "open-with")]
    open_with: bool,
    /// While choosing a tool's key: that tool.
    tool_recording: Option<&'static ToolInfo>,
    message: Option<(String, bool)>,
    /// Whether Duskshot starts with Windows. Kept in the registry, not the settings
    /// file, and changed as soon as the box is ticked.
    #[cfg(feature = "autostart")]
    autostart: crate::autostart::Status,
    /// The letters whose Win+ shortcuts Windows leaves alone (`DisabledHotkeys`). Kept
    /// in the registry, not the settings file, and changed as soon as a box is ticked.
    #[cfg(feature = "free-shortcut")]
    freed: String,
    /// The freed letters changed here, and Explorer has not been restarted since.
    #[cfg(feature = "free-shortcut")]
    explorer_stale: bool,
}

impl SettingsApp {
    fn new() -> Self {
        let settings = Settings::load();
        Self {
            saved: settings.clone(),
            settings,
            recorder: None,
            recording_for: Which::Capture,
            #[cfg(feature = "open-with")]
            open_with: crate::openwith::is_on(),
            tool_recording: None,
            message: None,
            #[cfg(feature = "autostart")]
            autostart: crate::autostart::status(),
            #[cfg(feature = "free-shortcut")]
            freed: crate::winkeys::freed(),
            #[cfg(feature = "free-shortcut")]
            explorer_stale: false,
        }
    }

    /// The shortcut setting for `which`.
    fn shortcut_mut(&mut self, which: Which) -> &mut String {
        match which {
            Which::Capture => &mut self.settings.hotkey,
            Which::Clipboard => &mut self.settings.clipboard_hotkey,
            Which::Reopen => &mut self.settings.reopen_hotkey,
        }
    }

    /// Start or stop recording the shortcut for `which`.
    fn toggle_recording(&mut self, which: Which) {
        self.message = None;
        self.tool_recording = None;
        let was = self.recorder.take().is_some() && self.recording_for == which;
        self.recording_for = which;
        if !was {
            self.recorder = Recorder::start();
            if self.recorder.is_none() {
                self.message = Some(("Could not listen to the keyboard.".to_owned(), true));
            }
        }
    }

    /// Pick up a combination caught by the recorder, if any.
    fn poll_recorder(&mut self, ctx: &egui::Context) {
        let Some(recorder) = &self.recorder else { return };
        let Some((vk, mods)) = recorder.recorded(ctx) else {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
            return;
        };
        self.recorder = None;
        if vk == VK_ESCAPE && mods == 0 {
            return; // cancelled
        }
        match shortcut_text(vk, mods) {
            Some(text) if shortcut_valid(&text) => {
                let which = self.recording_for;
                let taken = [Which::Capture, Which::Clipboard, Which::Reopen]
                    .into_iter()
                    .filter(|w| *w != which)
                    .find(|w| self.shortcut_mut(*w).eq_ignore_ascii_case(&text));
                match taken {
                    Some(other) => {
                        self.message = Some((format!("{} is already the {} shortcut.", crate::keys::shortcut_label(&text), other.name()), true));
                    }
                    None => {
                        *self.shortcut_mut(which) = text;
                        self.message = None;
                    }
                }
            }
            Some(text) => {
                self.message = Some((format!("{} can't be used as a shortcut.", crate::keys::shortcut_label(&text)), true));
            }
            None => self.message = Some(("That key can't be used as a shortcut.".to_owned(), true)),
        }
    }

    /// Pick up the key pressed for the tool whose key is being chosen, if any. Only
    /// keys pressed while this window has focus are seen.
    fn poll_tool_key(&mut self, ctx: &egui::Context) {
        let Some(tool) = self.tool_recording else { return };
        let pressed = ctx.input_mut(|input| {
            let found = input.events.iter().find_map(|event| match event {
                egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                _ => None,
            });
            if found.is_some() {
                input.events.clear();
            }
            found
        });
        let Some((key, modifiers)) = pressed else { return };
        self.tool_recording = None;
        self.message = None;
        if key == Key::Escape {
            return; // cancelled
        }
        let problem = if modifiers.ctrl || modifiers.alt || modifiers.command {
            Some("A tool's key is a single key, without Ctrl or Alt.".to_owned())
        } else if !settings::tool_key_allowed(key) {
            Some(format!("{} can't pick a tool: the capture uses it.", crate::keys::label(key)))
        } else {
            self.settings.tool_with_key(key, tool.id).map(|other| {
                format!("{} already picks {}. Give that tool another key first.", crate::keys::label(key), other.name)
            })
        };
        match problem {
            Some(text) => self.message = Some((text, true)),
            None => self.settings.set_tool_key(tool, Some(key)),
        }
    }

    /// The "Start with Windows" checkbox, applied at once.
    #[cfg(feature = "autostart")]
    fn startup_setting(&mut self, ui: &mut egui::Ui) {
        use crate::autostart::{self, Status};
        let mut on = self.autostart == Status::On;
        let toggle = ui.checkbox(&mut on, "Start with Windows").on_hover_text("Duskshot starts in the tray when you sign in.");
        if toggle.changed() {
            match autostart::set(on) {
                Ok(()) => self.autostart = autostart::status(),
                Err(e) => self.message = Some((format!("Couldn't change startup: {e}"), true)),
            }
        }
        if let Status::Elsewhere(path) = &self.autostart {
            let weak = ui.visuals().weak_text_color();
            let text = "Another copy of Duskshot starts with Windows. Tick to start this one instead.";
            ui.label(RichText::new(text).color(weak).small()).on_hover_text(path.as_str());
        }
    }

    /// Free the capture shortcut from Windows, or give shortcuts back; applied at once.
    #[cfg(feature = "free-shortcut")]
    fn free_shortcut_setting(&mut self, ui: &mut egui::Ui, weak: Color32) {
        use crate::winkeys;
        let current = winkeys::letter_of(&self.settings.hotkey);
        let mut changes = Vec::new();
        if let Some(letter) = current {
            let shortcut = crate::keys::shortcut_label(&self.settings.hotkey);
            let mut on = self.freed.contains(letter);
            let tip = format!(
                "Windows stops using Win+{letter} and its combinations with Ctrl, Alt or Shift, so Duskshot \
                 can use {shortcut}. Untick to give them back. Doesn't work on every version of Windows."
            );
            ui.horizontal(|ui| {
                if ui.checkbox(&mut on, format!("Free {shortcut} from Windows")).on_hover_text(tip).changed() {
                    changes.push((letter, on));
                }
                ui.label(RichText::new("(experimental)").color(ERROR_RED));
            });
        }
        let others: Vec<char> = self.freed.chars().filter(|&c| Some(c) != current).collect();
        if !others.is_empty() {
            ui.horizontal_wrapped(|ui| {
                let list: Vec<String> = others.iter().map(|c| format!("Win+{c}")).collect();
                ui.label(RichText::new(format!("Also freed: {}", list.join(", "))).color(weak).small());
                if ui.small_button("Give back").on_hover_text("Let Windows use these again").clicked() {
                    changes.extend(others.iter().map(|&c| (c, false)));
                }
            });
        }
        if !changes.is_empty() {
            for (letter, free) in changes {
                if let Err(e) = winkeys::set(letter, free) {
                    self.message = Some((format!("Couldn't change Windows shortcuts: {e}"), true));
                }
            }
            self.freed = winkeys::freed();
            self.explorer_stale = true;
        }
        if self.explorer_stale {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Takes effect when Explorer restarts.").color(weak).small());
                let restart = ui
                    .small_button("Restart Explorer")
                    .on_hover_text("The taskbar disappears for a moment, and File Explorer windows close.");
                if restart.clicked() {
                    match winkeys::restart_explorer() {
                        Ok(()) => self.explorer_stale = false,
                        Err(e) => self.message = Some((format!("Couldn't restart Explorer: {e}"), true)),
                    }
                }
            });
        }
    }

    /// The tools list: a box per tool, with its icon and the key that picks it.
    fn tools_setting(&mut self, ui: &mut egui::Ui, weak: Color32) {
        section(ui, "Tools");
        hint(ui, "Untick tools you don't use. Click a key to change it.");
        ui.add_space(4.0);
        let (ink, paper) = (ui.visuals().text_color(), ui.visuals().panel_fill);
        // The key buttons line up in a column of their own, just past the longest name.
        egui::Grid::new("tools").num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
            for info in &TOOLS {
                ui.horizontal(|ui| {
                    let mut on = self.settings.tool_enabled(info.id);
                    let mut changed = ui.checkbox(&mut on, "").changed();
                    let (area, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                    crate::icons::paint(ui.painter(), area, info.icon, if on { ink } else { weak }, paper);
                    ui.add_space(4.0);
                    if ui.add(egui::Label::new(info.name).sense(Sense::click())).clicked() {
                        on = !on;
                        changed = true;
                    }
                    if changed {
                        self.settings.set_tool_enabled(info.id, on);
                    }
                });
                ui.horizontal(|ui| {
                    let recording = self.tool_recording.is_some_and(|t| t.id == info.id);
                    let key = self.settings.tool_key(info);
                    let custom = key != info.key;
                    let text = if recording { "Press a key…".to_owned() } else { crate::keys::label(key) };
                    let button = ui.add_sized([100.0, 22.0], egui::Button::new(text).selected(recording));
                    let tip = format!("The key that picks {}. Click, then press a key. Esc cancels.", info.name);
                    if button.on_hover_text(tip).clicked() {
                        self.message = None;
                        self.recorder = None;
                        self.tool_recording = if recording { None } else { Some(info) };
                    }
                    let reset = custom && ui.small_button("Reset").on_hover_text(format!("Back to {}", crate::keys::label(info.key))).clicked();
                    if reset {
                        self.settings.set_tool_key(info, None);
                        self.tool_recording = None;
                        self.message = None;
                    }
                });
                ui.end_row();
            }
        });
        if !self.settings.disabled_tools.is_empty() {
            ui.add_space(4.0);
            if ui.small_button("Enable all").clicked() {
                self.settings.disabled_tools.clear();
            }
        }
    }
}

/// A colour button that edits a `#RRGGBB` setting, or `#RRGGBBAA` with `alpha`.
fn color_setting(ui: &mut egui::Ui, hex: &mut String, fallback: Color32, alpha: bool) -> egui::Response {
    let mut color = parse_hex(hex).unwrap_or(fallback);
    let response = crate::picker::swatch_button(ui, &mut color, alpha);
    let new = if alpha { crate::settings::to_hex_alpha(color) } else { to_hex(color) };
    if *hex != new {
        *hex = new;
    }
    response
}

/// Duskshot's source and releases, from Cargo.toml.
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// Hand this one fixed address to Windows, which opens it in the default browser.
/// Duskshot itself still connects to nothing, and nothing it reads (a QR code, say) is
/// ever opened: this is the only address it opens, and only when clicked.
fn open_repository() {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (verb, url) = (wide("open"), wide(REPOSITORY));
    // SAFETY: both strings are null-terminated and outlive the call.
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), url.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).strong().size(15.0));
    ui.add_space(4.0);
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let theme = self.settings.theme.into();
        if ctx.options(|o| o.theme_preference) != theme {
            ctx.set_theme(theme);
        }
        self.poll_recorder(&ctx);
        self.poll_tool_key(&ctx);
        let weak = ui.visuals().weak_text_color();

        // Save and Cancel stay at the bottom; the options above scroll if the window is
        // too short to show them all.
        egui::Panel::bottom("buttons").show(ui, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let changed = self.settings != self.saved;
                if ui.add_enabled(changed, egui::Button::new("Save")).clicked() {
                    match self.settings.store() {
                        Ok(_) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(e) => self.message = Some((format!("Could not save settings: {e}"), true)),
                    }
                }
                if ui.button("Cancel").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if ui.small_button("Restore defaults").clicked() {
                    self.settings = Settings::default();
                    self.tool_recording = None;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let link = ui
                        .link(REPOSITORY.trim_start_matches("https://"))
                        .on_hover_text("Opens Duskshot's page on GitHub in your browser");
                    if link.clicked() {
                        open_repository();
                    }
                    ui.label(RichText::new(concat!("Duskshot ", env!("CARGO_PKG_VERSION"))).color(weak));
                });
            });
            if let Some((text, error)) = &self.message {
                let color = if *error { ERROR_RED } else { weak };
                ui.label(RichText::new(text).color(color));
            }
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                // Columns lay out justified (buttons as wide as the column): undo that.
                let layout = egui::Layout::top_down(egui::Align::Min);
                ui.columns(2, |columns| {
                    columns[0].with_layout(layout, |ui| self.keys_column(ui, weak));
                    columns[1].with_layout(layout, |ui| self.options_column(ui, weak));
                });
            });
        });
    }
}

/// Width of the label column, the same in every section so the controls line up.
const LABEL_WIDTH: f32 = 128.0;

/// A two-column grid of labelled rows, lined up with the other sections.
fn rows(id: &str) -> egui::Grid {
    egui::Grid::new(id).num_columns(2).min_col_width(LABEL_WIDTH).spacing([8.0, 6.0])
}

fn hint(ui: &mut egui::Ui, text: &str) {
    let weak = ui.visuals().weak_text_color();
    ui.label(RichText::new(text).color(weak).small());
}

impl SettingsApp {
    /// The left column: shortcuts and tools.
    fn keys_column(&mut self, ui: &mut egui::Ui, weak: Color32) {
        section(ui, "Shortcuts");
        hint(ui, "Click a button, then press the keys. Esc cancels.");
        ui.add_space(2.0);
        rows("capture-shortcut").show(ui, |ui| {
            ui.label("Capture");
            ui.horizontal(|ui| {
                let recording = self.recorder.is_some() && self.recording_for == Which::Capture;
                let text = if recording { "Press keys…".to_owned() } else { crate::keys::shortcut_label(&self.settings.hotkey) };
                if ui.add_sized([140.0, 22.0], egui::Button::new(text).selected(recording)).clicked() {
                    self.toggle_recording(Which::Capture);
                }
                if self.settings.hotkey != DEFAULT_HOTKEY && ui.small_button("Reset").on_hover_text(DEFAULT_HOTKEY).clicked() {
                    self.settings.hotkey = DEFAULT_HOTKEY.into();
                    self.recorder = None;
                }
            });
            ui.end_row();
        });
        #[cfg(feature = "free-shortcut")]
        self.free_shortcut_setting(ui, weak);
        rows("other-shortcuts").show(ui, |ui| {
            for which in [Which::Clipboard, Which::Reopen] {
                ui.label(which.name());
                ui.horizontal(|ui| {
                    let recording = self.recorder.is_some() && self.recording_for == which;
                    let current = self.shortcut_mut(which).clone();
                    let text = if recording {
                        "Press keys…".to_owned()
                    } else if current.is_empty() {
                        "None".to_owned()
                    } else {
                        crate::keys::shortcut_label(&current)
                    };
                    if ui.add_sized([140.0, 22.0], egui::Button::new(text).selected(recording)).clicked() {
                        self.toggle_recording(which);
                    }
                    if !current.is_empty() && ui.small_button("Clear").clicked() {
                        self.shortcut_mut(which).clear();
                        self.recorder = None;
                    }
                });
                ui.end_row();
            }
        });
        self.tools_setting(ui, weak);
    }

    /// The right column: general options, colours and saving.
    fn options_column(&mut self, ui: &mut egui::Ui, weak: Color32) {
        use crate::help::HelpLevel;
        section(ui, "General");
        rows("general").show(ui, |ui| {
            ui.label("Theme");
            ui.horizontal(|ui| {
                for (value, name) in [(ThemeSetting::System, "System"), (ThemeSetting::Light, "Light"), (ThemeSetting::Dark, "Dark")] {
                    ui.selectable_value(&mut self.settings.theme, value, name);
                }
            });
            ui.end_row();
            ui.label("Tool help");
            ui.horizontal(|ui| {
                for (value, name, tip) in [
                    (HelpLevel::Off, "Off", "No box beside the selection. The keys show at the bottom, and F1 lists them all."),
                    (HelpLevel::Compact, "Compact", "A box beside the selection explains the current tool and its options."),
                    (HelpLevel::Full, "Full", "As Compact, plus the tool's keys."),
                ] {
                    ui.selectable_value(&mut self.settings.help_level, value, name).on_hover_text(tip);
                }
            });
            ui.end_row();
        });
        ui.add_space(2.0);
        ui.checkbox(&mut self.settings.include_cursor, "Include the mouse cursor")
            .on_hover_text("Drawn where it was when you pressed the shortcut.");
        ui.checkbox(&mut self.settings.confirm_discard, "Ask before discarding annotations")
            .on_hover_text("When you press Esc or Close before copying or saving.");
        ui.horizontal(|ui| {
            let mut keep = self.settings.keep_minutes > 0;
            let tip = "Reopen it from the tray menu or its shortcut. Kept in memory only, until the next \
                       capture or until Duskshot quits.";
            if ui.checkbox(&mut keep, "Keep the last capture for").on_hover_text(tip).changed() {
                self.settings.keep_minutes = if keep { 5 } else { 0 };
            }
            ui.add_enabled_ui(keep, |ui| {
                let mut minutes = self.settings.keep_minutes.max(1);
                ui.add(egui::DragValue::new(&mut minutes).range(1..=settings::MAX_KEEP_MINUTES));
                if keep {
                    self.settings.keep_minutes = minutes;
                }
                ui.label("minutes");
            });
        });
        #[cfg(feature = "autostart")]
        self.startup_setting(ui);
        #[cfg(feature = "open-with")]
        {
            let toggle = ui
                .checkbox(&mut self.open_with, "Show in Open with for images")
                .on_hover_text("Right-click a PNG or JPEG, then Open with. Doesn't change the default app.");
            if toggle.changed() {
                if let Err(e) = crate::openwith::set(self.open_with) {
                    self.message = Some((format!("Couldn't change Open with: {e}"), true));
                }
                self.open_with = crate::openwith::is_on();
            }
        }

        section(ui, "Colours");
        hint(ui, "What every capture starts with. Click one to change it.");
        ui.add_space(2.0);
        let frame = &mut self.settings.frame;
        rows("colours").show(ui, |ui| {
            ui.label("Swatches");
            ui.horizontal(|ui| {
                for (hex, fallback) in self.settings.swatches.iter_mut().zip(DEFAULT_SWATCHES) {
                    color_setting(ui, hex, fallback, false);
                }
            });
            ui.end_row();
            ui.label("Highlighter");
            color_setting(ui, &mut self.settings.marker_color, DEFAULT_MARKER_COLOR, false);
            ui.end_row();
            ui.checkbox(&mut frame.border, "Border").on_hover_text("Added around the image when it is copied or saved.");
            ui.add_enabled_ui(frame.border, |ui| {
                ui.horizontal(|ui| {
                    color_setting(ui, &mut frame.border_color, settings::DEFAULT_BORDER_COLOR, true);
                    ui.add(
                        egui::DragValue::new(&mut frame.border_width)
                            .range(1..=settings::MAX_BORDER_WIDTH)
                            .speed(0.1)
                            .suffix(" px"),
                    )
                    .on_hover_text("Thickness");
                    crate::icons::picker(ui, &mut frame.border_style, true, &crate::annotate::Dash::ALL, crate::icons::dash_preview)
                        .on_hover_text("Line style");
                });
            });
            ui.end_row();
            ui.checkbox(&mut frame.shadow, "Drop shadow").on_hover_text("A soft shadow in a margin around the image.");
            ui.end_row();
        });
        if ui.small_button("Reset colours").clicked() {
            let defaults = Settings::default();
            self.settings.swatches = defaults.swatches;
            self.settings.marker_color = defaults.marker_color;
            self.settings.frame.border_color = defaults.frame.border_color;
        }

        section(ui, "Saving");
        let save = &mut self.settings.save;
        rows("saving").show(ui, |ui| {
            crate::save::name_row(ui, "File name", &mut save.name, 200.0);
            ui.label("Format");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut save.format, FormatSetting::Png, "PNG");
                ui.selectable_value(&mut save.format, FormatSetting::Jpeg, "JPEG");
            });
            ui.end_row();
            ui.label("JPEG quality");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut save.jpeg_quality, QUALITY_HIGH, "High");
                ui.selectable_value(&mut save.jpeg_quality, QUALITY_MEDIUM, "Medium");
                ui.add(egui::DragValue::new(&mut save.jpeg_quality).range(10..=100));
            });
            ui.end_row();
            ui.label("Folder");
            ui.horizontal(|ui| {
                let shown = if save.folder.trim().is_empty() { "Pictures".to_owned() } else { save.folder.clone() };
                ui.add(egui::Label::new(shown).truncate()).on_hover_text("Where the save dialog opens");
                if ui.button("Browse…").clicked() {
                    let mut dialog = rfd::FileDialog::new().set_title("Default save folder");
                    if let Some(dir) = settings::pictures_folder() {
                        dialog = dialog.set_directory(dir);
                    }
                    if let Some(dir) = dialog.pick_folder() {
                        save.folder = dir.display().to_string();
                    }
                }
                if !save.folder.is_empty() && ui.small_button("Use Pictures").clicked() {
                    save.folder.clear();
                }
            });
            ui.end_row();
        });
        let mut preview = SaveOptions::from_settings(&self.settings);
        preview.format = Format::from(self.settings.save.format);
        ui.label(RichText::new(preview.file_name()).color(weak).small());
        ui.add_space(8.0);
    }
}
