//! Settings UI: panel routing + the editable General/Hotkey panels (P2).
//!
//! The UI never touches the filesystem or the hotkey manager itself — edits
//! are collected as [`SettingsAction`]s, drained by `App` after each frame,
//! applied on the main loop, and the outcome is reported back via the
//! `*_feedback` methods. That keeps config writes and hotkey re-registration
//! in one place (`App`) and the UI a pure function of its state.

use holler_config::Config;

/// One user-confirmed edit, applied by `App` after the frame.
pub enum SettingsAction {
    /// Persist the General panel fields (merged into the app config).
    SaveGeneral { injection_mode: String, vad: bool },
    /// Re-register the global PTT hotkey to this combo (e.g. "ctrl+alt+space")
    /// and persist it. The combo has already passed `try_parse_ptt_key` once
    /// in the UI, but `App` validates again — it owns the truth.
    ApplyPttKey(String),
}

/// The settings sections, in sidebar order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Panel {
    General,
    Hotkey,
    Providers,
    Permissions,
    History,
    Stats,
    About,
}

impl Panel {
    const ALL: [Self; 7] = [
        Self::General,
        Self::Hotkey,
        Self::Providers,
        Self::Permissions,
        Self::History,
        Self::Stats,
        Self::About,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Hotkey => "Hotkey",
            Self::Providers => "Providers",
            Self::Permissions => "Permissions",
            Self::History => "History",
            Self::Stats => "Stats",
            Self::About => "About",
        }
    }

    /// One-liner shown under the placeholder heading.
    fn blurb(self) -> &'static str {
        match self {
            Self::General => "Injection mode, VAD and other behaviour.",
            Self::Hotkey => "The push-to-talk combo.",
            Self::Providers => "Speech-to-text providers and API keys.",
            Self::Permissions => "Microphone and Accessibility status.",
            Self::History => "Your transcript history.",
            Self::Stats => "Local usage statistics.",
            Self::About => "About Holler.",
        }
    }
}

/// Pure UI state — kept apart from the GL/egui plumbing so `redraw` can
/// borrow it and `EguiGlow` mutably at the same time.
pub(super) struct UiState {
    selected: Panel,
    // General: draft vs last-saved values; Save enabled while they differ.
    injection_draft: String,
    vad_draft: bool,
    injection_saved: String,
    vad_saved: bool,
    general_status: Option<(bool, String)>,
    // Hotkey.
    ptt_label: String,
    capturing: bool,
    hotkey_status: Option<(bool, String)>,
    /// Edits confirmed this frame, drained by `SettingsWindow::redraw`.
    pub(super) actions: Vec<SettingsAction>,
}

impl UiState {
    pub(super) fn new(config: &Config, ptt_label: &str) -> Self {
        Self {
            selected: Panel::General,
            injection_draft: config.injection_mode.clone(),
            vad_draft: config.vad,
            injection_saved: config.injection_mode.clone(),
            vad_saved: config.vad,
            general_status: None,
            ptt_label: ptt_label.to_string(),
            capturing: false,
            hotkey_status: None,
            actions: Vec::new(),
        }
    }

    /// Outcome of a `SaveGeneral` action.
    pub(super) fn general_feedback(&mut self, res: Result<(), String>) {
        self.general_status = Some(match res {
            Ok(()) => {
                self.injection_saved = self.injection_draft.clone();
                self.vad_saved = self.vad_draft;
                (true, "Saved ✓".to_string())
            }
            Err(e) => (false, e),
        });
    }

    /// Outcome of an `ApplyPttKey` action. `Ok` carries the new display label.
    pub(super) fn hotkey_feedback(&mut self, res: Result<String, String>) {
        self.hotkey_status = Some(match res {
            Ok(label) => {
                self.ptt_label = label.clone();
                (true, format!("Active — hold {label} to talk"))
            }
            Err(e) => (false, e),
        });
    }

    pub(super) fn draw(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("settings-nav")
            .resizable(false)
            .exact_size(160.0)
            .show_inside(ui, |ui| {
                ui.add_space(8.0);
                for panel in Panel::ALL {
                    if ui
                        .selectable_label(self.selected == panel, panel.label())
                        .clicked()
                    {
                        self.selected = panel;
                    }
                }
            });

        egui::CentralPanel::default().show_inside(ui, |ui| match self.selected {
            Panel::General => self.draw_general(ui),
            Panel::Hotkey => self.draw_hotkey(ui),
            Panel::About => draw_about(ui),
            panel => draw_placeholder(ui, panel),
        });
    }

    fn draw_general(&mut self, ui: &mut egui::Ui) {
        ui.heading("General");
        ui.add_space(8.0);

        egui::ComboBox::from_label("Injection mode")
            .selected_text(self.injection_draft.as_str())
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.injection_draft, "paste".to_string(), "paste");
                ui.selectable_value(&mut self.injection_draft, "type".to_string(), "type");
            });
        ui.weak("paste is fast and uses the clipboard; type is slower but works in apps that block paste.");
        ui.add_space(8.0);

        ui.checkbox(
            &mut self.vad_draft,
            "Trim leading/trailing silence before transcription (VAD)",
        );
        ui.add_space(12.0);

        let dirty =
            self.injection_draft != self.injection_saved || self.vad_draft != self.vad_saved;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(dirty, egui::Button::new("Save"))
                .clicked()
            {
                self.general_status = None;
                self.actions.push(SettingsAction::SaveGeneral {
                    injection_mode: self.injection_draft.clone(),
                    vad: self.vad_draft,
                });
            }
            if ui
                .add_enabled(dirty, egui::Button::new("Revert"))
                .clicked()
            {
                self.injection_draft = self.injection_saved.clone();
                self.vad_draft = self.vad_saved;
                self.general_status = None;
            }
        });
        draw_status(ui, &self.general_status);
    }

    fn draw_hotkey(&mut self, ui: &mut egui::Ui) {
        ui.heading("Hotkey");
        ui.add_space(8.0);
        ui.label("Hold this combo to talk; release to transcribe:");
        ui.add_space(4.0);
        ui.strong(egui::RichText::new(&self.ptt_label).size(20.0));
        ui.add_space(12.0);

        if self.capturing {
            let _ = ui.button("Press the new combo…  (Esc cancels)");
            match captured_combo(ui) {
                Capture::None => {}
                Capture::Cancel => {
                    self.capturing = false;
                    self.hotkey_status = None;
                }
                Capture::Combo(combo) => match holler_config::try_parse_ptt_key(&combo) {
                    Ok((_, label)) => {
                        self.capturing = false;
                        self.hotkey_status = Some((true, format!("Registering {label}…")));
                        self.actions.push(SettingsAction::ApplyPttKey(combo));
                    }
                    // Stay in capture mode so the user can try another key.
                    Err(e) => self.hotkey_status = Some((false, format!("Unsupported combo: {e}"))),
                },
            }
        } else if ui.button("Change combo…").clicked() {
            self.capturing = true;
            self.hotkey_status = None;
        }
        ui.add_space(4.0);
        ui.weak("Applied immediately — no restart. The old combo stays active until the new one registers.");
        draw_status(ui, &self.hotkey_status);
    }
}

/// What the capture widget saw this frame.
enum Capture {
    None,
    Cancel,
    Combo(String),
}

/// Read the first key press of this frame as a hotkey combo string in
/// `holler-config` syntax (e.g. "ctrl+shift+p"). Modifier-only presses
/// produce no `Event::Key`, so holding modifiers while hunting is fine.
fn captured_combo(ui: &egui::Ui) -> Capture {
    ui.input(|i| {
        for ev in &i.events {
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = ev
            {
                if *key == egui::Key::Escape && modifiers.is_none() {
                    return Capture::Cancel;
                }
                return Capture::Combo(combo_string(*key, *modifiers));
            }
        }
        Capture::None
    })
}

/// egui key + modifiers → `holler-config` combo syntax. Key names mostly
/// coincide with `global-hotkey`'s parser tokens; the few that differ are
/// aliased here, and anything still unknown is rejected by validation with a
/// visible message.
fn combo_string(key: egui::Key, m: egui::Modifiers) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if m.ctrl {
        parts.push("ctrl");
    }
    if m.alt {
        parts.push("alt");
    }
    if m.shift {
        parts.push("shift");
    }
    if m.mac_cmd {
        parts.push("cmd");
    }
    let key_name = match key {
        egui::Key::Backtick => "backquote",
        egui::Key::Equals => "equal",
        egui::Key::OpenBracket => "bracketleft",
        egui::Key::CloseBracket => "bracketright",
        k => k.name(),
    };
    parts.push(key_name);
    parts.join("+").to_lowercase()
}

/// Green/red one-line outcome under a panel's controls.
fn draw_status(ui: &mut egui::Ui, status: &Option<(bool, String)>) {
    if let Some((ok, msg)) = status {
        ui.add_space(8.0);
        let color = if *ok {
            egui::Color32::from_rgb(110, 200, 110)
        } else {
            egui::Color32::from_rgb(230, 110, 100)
        };
        ui.colored_label(color, msg);
    }
}

/// Placeholder panel body — replaced section by section in P3–P6.
fn draw_placeholder(ui: &mut egui::Ui, panel: Panel) {
    ui.heading(panel.label());
    ui.add_space(4.0);
    ui.label(panel.blurb());
    ui.add_space(12.0);
    ui.weak("Coming soon.");
}

/// About — already real: name, version, licence. Cheap and final.
fn draw_about(ui: &mut egui::Ui) {
    ui.heading("Holler");
    ui.add_space(4.0);
    ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
    ui.add_space(8.0);
    ui.label("Push-to-talk dictation — a walkie-talkie for your agents.");
    ui.label("Hold the hotkey, speak, release: the transcript lands at your cursor.");
    ui.add_space(12.0);
    ui.weak("© 2026 joeVenner — MIT OR Apache-2.0");
}
