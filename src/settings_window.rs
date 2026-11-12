//! The settings window (`darkshot --settings`), opened from the tray menu.

use eframe::egui::{self, Color32, Key, RichText};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LWIN, VK_RWIN};

use crate::save::{Format, SaveOptions, QUALITY_HIGH, QUALITY_MEDIUM};
use crate::settings::{
    self, parse_hex, parse_hotkey, to_hex, FormatSetting, Settings, DEFAULT_HOTKEY,
    DEFAULT_MARKER_COLOR, DEFAULT_SWATCHES,
};
use crate::AnyResult;

pub fn run() -> AnyResult<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Darkshot settings")
            .with_inner_size([470.0, 470.0])
            .with_resizable(false),
        ..Default::default()
    };
    eframe::run_native(
        "Darkshot settings",
        options,
        Box::new(|cc| {
            crate::overlay::setup_text(&cc.egui_ctx);
            Ok(Box::new(SettingsApp::new()))
        }),
    )?;
    Ok(())
}

struct SettingsApp {
    settings: Settings,
    saved: Settings,
    /// Waiting for the user to press a new shortcut.
    recording: bool,
    message: Option<(String, bool)>,
}

impl SettingsApp {
    fn new() -> Self {
        let settings = Settings::load();
        Self { saved: settings.clone(), settings, recording: false, message: None }
    }

    /// While recording, turn the next key press into a shortcut such as `Win+Shift+S`.
    fn record(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        for event in events {
            let egui::Event::Key { key, pressed, modifiers, .. } = event else { continue };
            // PrintScreen only reports its release on Windows.
            if !pressed && key != Key::PrintScreen {
                continue;
            }
            // Modifier keys on their own: wait for the key they go with.
            if matches!(
                key,
                Key::ShiftLeft
                    | Key::ShiftRight
                    | Key::ControlLeft
                    | Key::ControlRight
                    | Key::AltLeft
                    | Key::AltRight
                    | Key::SuperLeft
                    | Key::SuperRight
            ) {
                continue;
            }
            let win = unsafe { GetAsyncKeyState(i32::from(VK_LWIN)) < 0 || GetAsyncKeyState(i32::from(VK_RWIN)) < 0 };
            if key == Key::Escape && !modifiers.any() && !win {
                self.recording = false;
                return;
            }
            // Windows writes the Windows key first (Win+Shift+S).
            let mut parts = Vec::new();
            if win {
                parts.push("Win");
            }
            if modifiers.ctrl {
                parts.push("Ctrl");
            }
            if modifiers.alt {
                parts.push("Alt");
            }
            if modifiers.shift {
                parts.push("Shift");
            }
            parts.push(key.name());
            let combo = parts.join("+");
            self.recording = false;
            if parse_hotkey(&combo).is_some() {
                self.settings.hotkey = combo;
                self.message = None;
            } else {
                self.message = Some((format!("{combo} can't be used as a shortcut."), true));
            }
            return;
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
        if self.recording {
            self.record(&ctx);
        }
        let weak = ui.visuals().weak_text_color();

        egui::CentralPanel::default().show(ui, |ui| {
            section(ui, "Capture shortcut");
            ui.horizontal(|ui| {
                let text = if self.recording { "Press keys…".to_owned() } else { self.settings.hotkey.clone() };
                let button = ui.add_sized([160.0, 26.0], egui::Button::new(text).selected(self.recording));
                if button.on_hover_text("Click, then press the combination. Esc cancels.").clicked() {
                    self.recording = !self.recording;
                    self.message = None;
                }
                if ui.button("Reset").on_hover_text(DEFAULT_HOTKEY).clicked() {
                    self.settings.hotkey = DEFAULT_HOTKEY.into();
                    self.recording = false;
                }
            });
            ui.label(
                RichText::new("Click the button and press a combination, e.g. Win+Shift+S or PrintScreen.")
                    .color(weak)
                    .small(),
            );

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
