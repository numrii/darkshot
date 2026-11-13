//! The settings window (`darkshot --settings`), opened from the tray menu.

use eframe::egui::{self, Color32, RichText};

use crate::save::{Format, SaveOptions, QUALITY_HIGH, QUALITY_MEDIUM};
use crate::settings::{
    self, parse_hex, shortcut_text, shortcut_valid, to_hex, FormatSetting, Settings, DEFAULT_HOTKEY,
    DEFAULT_MARKER_COLOR, DEFAULT_SWATCHES,
};
use crate::AnyResult;

use recorder::Recorder;

const VK_ESCAPE: u32 = 0x1B;
/// The shortcut suggested when taking over from Windows.
#[cfg(feature = "shortcut-takeover")]
const TAKE_OVER_SUGGESTION: &str = "Win+Shift+S";

/// Recording a shortcut with the low-level keyboard hook, so it also catches
/// combinations Windows keeps for itself (Win+Shift+S), which would otherwise trigger
/// Windows' own action instead of reaching this window.
#[cfg(feature = "shortcut-takeover")]
mod recorder {
    use eframe::egui;

    pub struct Recorder(crate::keyhook::Hook);

    impl Recorder {
        pub fn start() -> Option<Self> {
            crate::keyhook::Hook::record().map(Self)
        }

        /// The combination caught, as (virtual-key code, `MOD_*` bits).
        pub fn recorded(&self, _ctx: &egui::Context) -> Option<(u32, u8)> {
            self.0.recorded()
        }
    }
}

/// Recording a shortcut from this window's own keyboard input, so only keys pressed
/// while it has focus are seen. Combinations Windows keeps for itself never arrive.
#[cfg(not(feature = "shortcut-takeover"))]
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
                    egui::Event::Key { key, pressed, modifiers, .. } if *pressed || *key == Key::PrintScreen => {
                        let vk = vk_code(*key)?;
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
            .with_title("Darkshot settings")
            .with_icon(crate::appicon::window_icon())
            .with_inner_size([470.0, if cfg!(feature = "autostart") { 560.0 } else { 500.0 }])
            .with_resizable(false),
        ..Default::default()
    };
    // Own event loop, with raw device input switched off: with it on, the low-level
    // keyboard hook used for recording shortcuts (with `shortcut-takeover`) is not
    // called in this process. Nothing else here needs raw input.
    let event_loop = winit::event_loop::EventLoop::<eframe::UserEvent>::with_user_event().build()?;
    event_loop.listen_device_events(winit::event_loop::DeviceEvents::Never);
    let mut app = eframe::create_native(
        "Darkshot settings",
        options,
        Box::new(|cc| {
            crate::overlay::setup_text(&cc.egui_ctx);
            Ok(Box::new(SettingsApp::new()))
        }),
        &event_loop,
    );
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct SettingsApp {
    settings: Settings,
    saved: Settings,
    /// While recording a shortcut: what catches it.
    recorder: Option<Recorder>,
    message: Option<(String, bool)>,
    /// Whether Darkshot starts with Windows. Kept in the registry, not the settings
    /// file, and changed as soon as the box is ticked.
    #[cfg(feature = "autostart")]
    autostart: crate::autostart::Status,
}

impl SettingsApp {
    fn new() -> Self {
        let settings = Settings::load();
        Self {
            saved: settings.clone(),
            settings,
            recorder: None,
            message: None,
            #[cfg(feature = "autostart")]
            autostart: crate::autostart::status(),
        }
    }

    /// Start or stop recording a shortcut.
    fn toggle_recording(&mut self) {
        self.message = None;
        if self.recorder.take().is_none() {
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
            Some(text) if shortcut_valid(&text, self.settings.take_over) => {
                self.settings.hotkey = text;
                self.message = None;
            }
            Some(text) => self.message = Some((format!("{text} can't be used as a shortcut."), true)),
            None => self.message = Some(("That key can't be used as a shortcut.".to_owned(), true)),
        }
    }

    /// The "Start with Windows" checkbox, applied at once.
    #[cfg(feature = "autostart")]
    fn startup_setting(&mut self, ui: &mut egui::Ui) {
        use crate::autostart::{self, Status};
        section(ui, "Startup");
        let mut on = self.autostart == Status::On;
        let toggle = ui
            .checkbox(&mut on, "Start Darkshot when I sign in to Windows")
            .on_hover_text("Adds Darkshot to your own startup programs. No administrator rights are needed.");
        if toggle.changed() {
            match autostart::set(on) {
                Ok(()) => self.autostart = autostart::status(),
                Err(e) => self.message = Some((format!("Could not change startup: {e}"), true)),
            }
        }
        if let Status::Elsewhere(path) = &self.autostart {
            let weak = ui.visuals().weak_text_color();
            let text = format!("Windows starts a Darkshot at {path}. Tick the box to start this one instead.");
            ui.label(RichText::new(text).color(weak).small());
        }
    }

    /// The "Take over the shortcut from Windows" checkbox.
    #[cfg(feature = "shortcut-takeover")]
    fn take_over_setting(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        let take_over = ui
            .checkbox(&mut self.settings.take_over, "Take over the shortcut from Windows")
            .on_hover_text(
                "For combinations Windows keeps for itself, such as Win+Shift+S. Darkshot \
                 catches the keys before Windows does while it runs; nothing in Windows is changed.",
            );
        // Taking over is mostly wanted for Win+Shift+S, so suggest it if the shortcut
        // has not been changed yet.
        if take_over.changed() && self.settings.take_over && self.settings.hotkey == DEFAULT_HOTKEY {
            self.settings.hotkey = TAKE_OVER_SUGGESTION.into();
        }
    }
}

/// A colour button that edits a `#RRGGBB` setting.
fn color_setting(ui: &mut egui::Ui, hex: &mut String, fallback: Color32) -> egui::Response {
    let mut color = parse_hex(hex).unwrap_or(fallback);
    let response = ui.color_edit_button_srgba(&mut color);
    let new = to_hex(color);
    if *hex != new {
        *hex = new;
    }
    response
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).strong().size(15.0));
    ui.add_space(4.0);
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_recorder(&ctx);
        let weak = ui.visuals().weak_text_color();

        egui::CentralPanel::default().show(ui, |ui| {
            section(ui, "Capture shortcut");
            ui.horizontal(|ui| {
                let recording = self.recorder.is_some();
                let text = if recording { "Press keys…".to_owned() } else { self.settings.hotkey.clone() };
                let button = ui.add_sized([160.0, 26.0], egui::Button::new(text).selected(recording));
                if button.on_hover_text("Click, then press the combination. Esc cancels.").clicked() {
                    self.toggle_recording();
                }
                if ui.button("Reset").on_hover_text(DEFAULT_HOTKEY).clicked() {
                    self.settings.hotkey = DEFAULT_HOTKEY.into();
                    self.settings.take_over = false;
                    self.recorder = None;
                }
            });
            let hint = if settings::TAKEOVER_AVAILABLE {
                "Click the button and press a combination, e.g. Win+Shift+S or PrintScreen."
            } else {
                "Click the button and press a combination, e.g. Ctrl+F9 or PrintScreen. \
                 Combinations Windows keeps for itself cannot be used."
            };
            ui.label(RichText::new(hint).color(weak).small());
            #[cfg(feature = "shortcut-takeover")]
            self.take_over_setting(ui);

            section(ui, "Colours");

            egui::Grid::new("colours").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label("Swatches");
                ui.horizontal(|ui| {
                    for (hex, fallback) in self.settings.swatches.iter_mut().zip(DEFAULT_SWATCHES) {
                        color_setting(ui, hex, fallback);
                    }
                });
                ui.end_row();
                ui.label("Highlighter");
                color_setting(ui, &mut self.settings.marker_color, DEFAULT_MARKER_COLOR);
                ui.end_row();
            });
            if ui.small_button("Reset colours").clicked() {
                let defaults = Settings::default();
                self.settings.swatches = defaults.swatches;
                self.settings.marker_color = defaults.marker_color;
            }

            #[cfg(feature = "autostart")]
            self.startup_setting(ui);

            section(ui, "Saving");
            let save = &mut self.settings.save;
            egui::Grid::new("saving").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label("Prefix");
                ui.add(egui::TextEdit::singleline(&mut save.prefix).hint_text("none").desired_width(240.0));
                ui.end_row();
                ui.label("Suffix");
                ui.add(egui::TextEdit::singleline(&mut save.suffix).hint_text("none").desired_width(240.0));
                ui.end_row();
                ui.label("");
                ui.label(RichText::new("{date} and {time} are filled in when saving").color(weak).small());
                ui.end_row();

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
                    ui.add(egui::Slider::new(&mut save.jpeg_quality, 10..=100));
                });
                ui.end_row();

                ui.label("Folder");
                ui.horizontal(|ui| {
                    let shown = if save.folder.trim().is_empty() { "Pictures".to_owned() } else { save.folder.clone() };
                    ui.add(egui::Label::new(shown).truncate()).on_hover_text("The save dialog opens here");
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
            ui.label(RichText::new(format!("Example: {}", preview.file_name())).color(weak).small());

            ui.add_space(14.0);
            ui.separator();
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
                if ui.small_button("Restore all defaults").clicked() {
                    self.settings = Settings::default();
                }
            });
            if let Some((text, error)) = &self.message {
                let color = if *error { Color32::from_rgb(0xE5, 0x48, 0x4D) } else { weak };
                ui.label(RichText::new(text).color(color));
            }
        });
    }
}
