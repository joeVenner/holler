//! Settings UI: panel routing + the editable General/Hotkey panels (P2).
//!
//! The UI never touches the filesystem or the hotkey manager itself — edits
//! are collected as [`SettingsAction`]s, drained by `App` after each frame,
//! applied on the main loop, and the outcome is reported back via the
//! `*_feedback` methods. That keeps config writes and hotkey re-registration
//! in one place (`App`) and the UI a pure function of its state.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use holler_config::{Config, SecretStatus};
use holler_store::Entry;

use crate::permissions::{self, MicStatus};

/// Green/red status colours, shared by every panel's outcome line.
const OK_GREEN: egui::Color32 = egui::Color32::from_rgb(110, 200, 110);
const ERR_RED: egui::Color32 = egui::Color32::from_rgb(230, 110, 100);

/// One user-confirmed edit, applied by `App` after the frame.
pub enum SettingsAction {
    /// Persist the General panel fields (merged into the app config).
    SaveGeneral { injection_mode: String, vad: bool },
    /// Re-register the global PTT hotkey to this combo (e.g. "ctrl+alt+space")
    /// and persist it. The combo has already passed `try_parse_ptt_key` once
    /// in the UI, but `App` validates again — it owns the truth.
    ApplyPttKey(String),
    /// Persist the active STT provider + model override.
    SaveProviders { provider: String, model: String },
    /// Store an API key in secrets.toml. The key string lives only for the
    /// trip UI → App → file; it is never echoed back.
    SetKey { provider: String, key: String },
    /// Remove a provider's API key from secrets.toml.
    ClearKey { provider: String },
    /// Open the OS Accessibility privacy pane (macOS Grant button).
    OpenAccessibilitySettings,
    /// Open the OS Microphone privacy pane.
    OpenMicrophoneSettings,
    /// Load (or reload) the history list filtered by `query` (empty = all),
    /// newest first. `App` replies via `SettingsWindow::history_loaded`.
    LoadHistory { query: String },
    /// Copy a history entry's text to the system clipboard.
    CopyHistory { text: String },
    /// Delete the history entry `id`, then reload the list filtered by `query`
    /// so the view reflects the new truth.
    DeleteHistory { id: i64, query: String },
}

/// Everything the Providers panel needs to render one provider row. Adding a
/// provider = adding a line here (+ its real backend in holler-stt when ready).
struct ProviderMeta {
    /// Config/secrets identifier ("deepgram") — also the `set-key` account.
    id: &'static str,
    name: &'static str,
    kind: &'static str, // "Cloud" | "Local"
    default_model: &'static str,
    /// false → disabled "Coming soon" row.
    available: bool,
}

const PROVIDERS: &[ProviderMeta] = &[
    ProviderMeta {
        id: "deepgram",
        name: "Deepgram",
        kind: "Cloud",
        default_model: holler_stt::DeepgramStt::DEFAULT_MODEL,
        available: true,
    },
    ProviderMeta {
        id: "openai",
        name: "OpenAI",
        kind: "Cloud",
        default_model: holler_stt::OpenAiStt::DEFAULT_MODEL,
        available: true,
    },
    ProviderMeta {
        id: "groq",
        name: "Groq Whisper",
        kind: "Cloud",
        default_model: "",
        available: false,
    },
    ProviderMeta {
        id: "elevenlabs",
        name: "ElevenLabs Scribe",
        kind: "Cloud",
        default_model: "",
        available: false,
    },
    ProviderMeta {
        id: "local-whisper",
        name: "LocalWhisper",
        kind: "Local",
        default_model: "",
        available: false,
    },
    ProviderMeta {
        id: "parakeet",
        name: "NVIDIA Parakeet",
        kind: "Local",
        default_model: "",
        available: false,
    },
];

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
    // Providers: selection draft vs saved, per-provider key entry + status.
    provider_draft: String,
    model_draft: String,
    provider_saved: String,
    model_saved: String,
    provider_status: Option<(bool, String)>,
    key_drafts: BTreeMap<&'static str, String>,
    key_status: BTreeMap<&'static str, SecretStatus>,
    // Permissions: live OS status, refreshed by the main loop's poll (so the
    // panel reflects a grant/revoke done in System Settings without a restart).
    ax_granted: bool,
    mic_status: MicStatus,
    // History: the search box, the query we last asked `App` to load (so we
    // request a reload only when it actually changes — natural debounce), the
    // loaded entries, and the inline delete-confirm target.
    history_query: String,
    history_requested: Option<String>,
    history_entries: Vec<Entry>,
    history_loaded: bool,
    history_status: Option<(bool, String)>,
    confirm_delete: Option<i64>,
    /// Edits confirmed this frame, drained by `SettingsWindow::redraw`.
    pub(super) actions: Vec<SettingsAction>,
}

impl UiState {
    pub(super) fn new(config: &Config, ptt_label: &str) -> Self {
        // Snapshot key presence once at window open (refreshed after every
        // set/clear). This is a plain file read — secrets.toml replaced the
        // keychain, so there is no OS prompt to worry about; the values
        // themselves are never loaded here.
        let key_status = PROVIDERS
            .iter()
            .filter(|p| p.available)
            .map(|p| (p.id, holler_config::secret_status(p.id)))
            .collect();
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
            provider_draft: config.stt_provider.clone(),
            model_draft: config.stt_model.clone(),
            provider_saved: config.stt_provider.clone(),
            model_saved: config.stt_model.clone(),
            provider_status: None,
            key_drafts: BTreeMap::new(),
            key_status,
            ax_granted: permissions::accessibility_granted(),
            mic_status: permissions::microphone_status(),
            history_query: String::new(),
            history_requested: None,
            history_entries: Vec::new(),
            history_loaded: false,
            history_status: None,
            confirm_delete: None,
            actions: Vec::new(),
        }
    }

    /// Re-query the live OS permission status. Called by the main loop's poll
    /// (via `SettingsWindow::refresh_permissions`) whenever it detects a change,
    /// so the open panel tracks grants/revokes made in System Settings.
    pub(super) fn refresh_permissions(&mut self) {
        self.ax_granted = permissions::accessibility_granted();
        self.mic_status = permissions::microphone_status();
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

    /// Outcome of a `SaveProviders` action.
    pub(super) fn provider_feedback(&mut self, res: Result<(), String>) {
        self.provider_status = Some(match res {
            Ok(()) => {
                self.provider_saved = self.provider_draft.clone();
                self.model_saved = self.model_draft.clone();
                (true, "Saved ✓ — used on the next dictation".to_string())
            }
            Err(e) => (false, e),
        });
    }

    /// Outcome of a `SetKey`/`ClearKey` action; re-probes the key status so
    /// the ✓/✗ reflects the file truth, not what we think happened.
    pub(super) fn key_feedback(&mut self, provider: &str, res: Result<(), String>) {
        if let Some(meta) = PROVIDERS.iter().find(|p| p.id == provider) {
            self.key_status
                .insert(meta.id, holler_config::secret_status(meta.id));
            self.key_drafts.remove(meta.id); // never retain typed key material
        }
        self.provider_status = Some(match res {
            Ok(()) => (true, "Key updated ✓".to_string()),
            Err(e) => (false, e),
        });
    }

    /// Outcome of a `LoadHistory`/`DeleteHistory` reload: replace the list on
    /// success, or surface the DB error.
    pub(super) fn history_loaded(&mut self, res: Result<Vec<Entry>, String>) {
        self.history_loaded = true;
        match res {
            Ok(entries) => self.history_entries = entries,
            Err(e) => self.history_status = Some((false, e)),
        }
    }

    /// Outcome of a `CopyHistory`/`DeleteHistory` action — a transient status
    /// line. `Ok` carries the success message (e.g. "Copied ✓").
    pub(super) fn history_action_feedback(&mut self, res: Result<String, String>) {
        self.history_status = Some(match res {
            Ok(msg) => (true, msg),
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
            Panel::Providers => self.draw_providers(ui),
            Panel::Permissions => self.draw_permissions(ui),
            Panel::History => self.draw_history(ui),
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

    fn draw_providers(&mut self, ui: &mut egui::Ui) {
        ui.heading("Providers");
        ui.add_space(4.0);
        ui.label("Speech-to-text runs through the selected provider (bring your own key).");
        ui.add_space(8.0);

        egui::ScrollArea::vertical().show(ui, |ui| {
            for meta in PROVIDERS {
                self.draw_provider_row(ui, meta);
                ui.add_space(6.0);
            }

            ui.add_space(6.0);
            let dirty = self.provider_draft != self.provider_saved
                || self.model_draft != self.model_saved;
            if ui.add_enabled(dirty, egui::Button::new("Save")).clicked() {
                self.provider_status = None;
                self.actions.push(SettingsAction::SaveProviders {
                    provider: self.provider_draft.clone(),
                    model: self.model_draft.trim().to_string(),
                });
            }
            draw_status(ui, &self.provider_status);
            ui.add_space(8.0);
            ui.weak("Keys are stored in secrets.toml next to config.toml (never displayed here). \
                     A HOLLER_<PROVIDER>_KEY environment variable overrides the file.");
        });
    }

    /// One provider row: radio + key state for available providers, a
    /// disabled "Coming soon" line for future ones.
    fn draw_provider_row(&mut self, ui: &mut egui::Ui, meta: &ProviderMeta) {
        if !meta.available {
            ui.horizontal(|ui| {
                ui.add_enabled(false, egui::RadioButton::new(false, meta.name));
                ui.weak(format!("{} · Coming soon", meta.kind));
            });
            return;
        }

        ui.horizontal(|ui| {
            if ui
                .radio(self.provider_draft == meta.id, meta.name)
                .clicked()
                && self.provider_draft != meta.id
            {
                self.provider_draft = meta.id.to_string();
                // A model override belongs to one provider — don't carry it over.
                self.model_draft.clear();
            }
            ui.weak(meta.kind);
            match self.key_status.get(meta.id) {
                Some(SecretStatus::FromFile) => {
                    ui.colored_label(OK_GREEN, "key configured ✓");
                }
                Some(SecretStatus::FromEnv) => {
                    ui.colored_label(OK_GREEN, "key configured ✓ (env var)");
                }
                _ => {
                    ui.weak("no key ✗");
                }
            }
        });

        // Model override applies to the selected provider only.
        if self.provider_draft == meta.id {
            ui.indent(meta.id, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Model:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.model_draft)
                            .hint_text(format!("default: {}", meta.default_model))
                            .desired_width(220.0),
                    );
                });
            });
        }

        // Key entry: typed key material lives only until Set is clicked.
        ui.indent((meta.id, "key"), |ui| {
            let from_env = self.key_status.get(meta.id) == Some(&SecretStatus::FromEnv);
            let draft = self.key_drafts.entry(meta.id).or_default();
            ui.horizontal(|ui| {
                ui.add_enabled(
                    !from_env,
                    egui::TextEdit::singleline(draft)
                        .password(true)
                        .hint_text("paste API key")
                        .desired_width(220.0),
                );
                let can_set = !from_env && !draft.trim().is_empty();
                if ui.add_enabled(can_set, egui::Button::new("Set key")).clicked() {
                    let key = std::mem::take(draft);
                    self.actions.push(SettingsAction::SetKey {
                        provider: meta.id.to_string(),
                        key: key.trim().to_string(),
                    });
                }
                let can_clear =
                    self.key_status.get(meta.id) == Some(&SecretStatus::FromFile);
                if ui
                    .add_enabled(can_clear, egui::Button::new("Clear"))
                    .clicked()
                {
                    self.actions.push(SettingsAction::ClearKey {
                        provider: meta.id.to_string(),
                    });
                }
            });
            if from_env {
                ui.weak("Managed by the environment variable — unset it in your shell to change.");
            }
        });
    }

    fn draw_permissions(&mut self, ui: &mut egui::Ui) {
        ui.heading("Permissions");
        ui.add_space(4.0);
        ui.label("Holler needs to hear you and to type the transcript at your cursor.");
        ui.add_space(14.0);

        // --- Microphone: required to capture any audio at all. ---
        ui.strong("Microphone");
        ui.add_space(2.0);
        if cfg!(target_os = "macos") {
            match self.mic_status {
                MicStatus::Granted => perm_line(ui, true, "Granted — Holler can hear you."),
                MicStatus::NotDetermined => {
                    ui.label("Not requested yet — macOS will ask the first time you record.");
                }
                MicStatus::Denied => {
                    perm_line(ui, false, "Denied — recordings will be silent.");
                    if ui.button("Open Microphone Settings…").clicked() {
                        self.actions.push(SettingsAction::OpenMicrophoneSettings);
                    }
                }
                MicStatus::Restricted => {
                    perm_line(ui, false, "Blocked by your organization — ask your administrator.");
                }
            }
        } else {
            ui.label("Managed by Windows (Settings → Privacy → Microphone).");
            if ui.button("Open Microphone Settings…").clicked() {
                self.actions.push(SettingsAction::OpenMicrophoneSettings);
            }
        }
        ui.add_space(16.0);

        // --- Accessibility / input injection: needed for auto-paste/type. ---
        if cfg!(target_os = "macos") {
            ui.strong("Accessibility");
            ui.add_space(2.0);
            if self.ax_granted {
                perm_line(ui, true, "Granted — auto-paste is active.");
            } else {
                perm_line(
                    ui,
                    false,
                    "Not granted — transcripts land on the clipboard for you to paste.",
                );
                if ui.button("Grant Accessibility Access…").clicked() {
                    self.actions
                        .push(SettingsAction::OpenAccessibilitySettings);
                }
            }
        } else {
            ui.strong("Input injection");
            ui.add_space(2.0);
            ui.label("No permission required on Windows.");
            ui.weak("Auto-paste can only fail against apps run as Administrator (UIPI); \
                     run Holler elevated too if you need to type into them.");
        }

        ui.add_space(16.0);
        ui.separator();
        ui.add_space(6.0);
        if cfg!(target_os = "macos") {
            ui.weak("Status refreshes automatically — grant access in System Settings and \
                     this updates within a couple of seconds, no restart needed.");
        }
    }

    fn draw_history(&mut self, ui: &mut egui::Ui) {
        ui.heading("History");
        ui.add_space(4.0);
        ui.label("Your past transcripts, newest first.");
        ui.add_space(8.0);

        // Search + manual refresh. The list lives only while this window is
        // open (UiState is dropped on close), and loads lazily on first view.
        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.add(
                egui::TextEdit::singleline(&mut self.history_query)
                    .hint_text("filter transcripts")
                    .desired_width(260.0),
            );
            // Re-pull the current query (catches transcripts dictated since the
            // panel was first opened).
            if ui.button("Refresh").clicked() {
                self.history_requested = None;
            }
        });

        // Ask `App` to (re)load whenever the query changes or a refresh is
        // forced. Setting `history_requested` here — not on the reply — stops
        // us re-queuing the same query every frame.
        if self.history_requested.as_deref() != Some(self.history_query.as_str()) {
            self.history_requested = Some(self.history_query.clone());
            self.history_status = None;
            self.confirm_delete = None;
            self.actions.push(SettingsAction::LoadHistory {
                query: self.history_query.clone(),
            });
        }

        draw_status(ui, &self.history_status);
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);

        if !self.history_loaded {
            ui.weak("Loading…");
            return;
        }
        if self.history_entries.is_empty() {
            ui.weak(if self.history_query.trim().is_empty() {
                "No transcripts yet — hold the hotkey and speak."
            } else {
                "No transcripts match your search."
            });
            return;
        }

        // Collect intents during the (immutable) borrow of the entry list, then
        // apply them after — we can't push actions / flip `confirm_delete`
        // while iterating `&self.history_entries`.
        let now = now_unix();
        let confirm = self.confirm_delete;
        let mut to_copy: Option<String> = None;
        let mut to_delete: Option<i64> = None;
        let mut set_confirm: Option<Option<i64>> = None;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for entry in &self.history_entries {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.strong(provider_display(&entry.provider));
                            ui.weak("·");
                            ui.weak(format_relative(now - entry.created_at))
                                .on_hover_text(format_utc(entry.created_at));

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if confirm == Some(entry.id) {
                                        if ui.button("Cancel").clicked() {
                                            set_confirm = Some(None);
                                        }
                                        if ui
                                            .button(
                                                egui::RichText::new("Confirm delete")
                                                    .color(ERR_RED),
                                            )
                                            .clicked()
                                        {
                                            to_delete = Some(entry.id);
                                        }
                                    } else {
                                        if ui.button("Delete").clicked() {
                                            set_confirm = Some(Some(entry.id));
                                        }
                                        if ui.button("Copy").clicked() {
                                            to_copy = Some(entry.text.clone());
                                        }
                                    }
                                },
                            );
                        });
                        ui.add_space(2.0);
                        // Wrap long transcripts rather than stretching the row.
                        ui.add(egui::Label::new(&entry.text).wrap());
                    });
                    ui.add_space(6.0);
                }
            });

        if let Some(c) = set_confirm {
            self.confirm_delete = c;
            self.history_status = None;
        }
        if let Some(text) = to_copy {
            self.actions.push(SettingsAction::CopyHistory { text });
        }
        if let Some(id) = to_delete {
            self.confirm_delete = None;
            self.actions.push(SettingsAction::DeleteHistory {
                id,
                query: self.history_query.clone(),
            });
        }
    }
}

/// Display name for a stored provider id, falling back to the raw value for
/// anything not in the table (forward-compatible with future providers).
fn provider_display(id: &str) -> &str {
    PROVIDERS
        .iter()
        .find(|p| p.id == id)
        .map(|p| p.name)
        .unwrap_or(id)
}

/// Current time as Unix epoch seconds (saturating to 0 before 1970).
fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A compact "how long ago" label. `secs` is now − created_at; a negative
/// value (clock skew / a row from the future) reads as "just now".
fn format_relative(secs: i64) -> String {
    match secs {
        i64::MIN..=4 => "just now".to_string(),
        5..=59 => format!("{secs}s ago"),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        86_400..=172_799 => "yesterday".to_string(),
        _ => format!("{}d ago", secs / 86_400),
    }
}

/// Absolute UTC timestamp for the hover tooltip, e.g. "2026-06-11 21:56 UTC".
/// Pure integer math (Hinnant's civil-from-days) — no date crate, no timezone
/// data, identical on macOS and Windows.
fn format_utc(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let secs_of_day = epoch.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let hh = secs_of_day / 3600;
    let mm = (secs_of_day % 3600) / 60;
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02} UTC")
}

/// Convert a count of days since the Unix epoch to a `(year, month, day)`
/// Gregorian date (Howard Hinnant's algorithm; exact for the supported range).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A green/red permission status line (no leading space — the caller controls
/// spacing, unlike [`draw_status`] which sits under a panel's controls).
fn perm_line(ui: &mut egui::Ui, ok: bool, msg: &str) {
    ui.colored_label(if ok { OK_GREEN } else { ERR_RED }, msg);
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
        ui.colored_label(if *ok { OK_GREEN } else { ERR_RED }, msg);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_time_buckets() {
        assert_eq!(format_relative(-10), "just now"); // clock skew
        assert_eq!(format_relative(0), "just now");
        assert_eq!(format_relative(42), "42s ago");
        assert_eq!(format_relative(120), "2m ago");
        assert_eq!(format_relative(7_200), "2h ago");
        assert_eq!(format_relative(90_000), "yesterday");
        assert_eq!(format_relative(3 * 86_400), "3d ago");
    }

    #[test]
    fn utc_timestamp_matches_known_epochs() {
        assert_eq!(format_utc(0), "1970-01-01 00:00 UTC");
        // 1_600_000_000 == 2020-09-13 12:26:40 UTC.
        assert_eq!(format_utc(1_600_000_000), "2020-09-13 12:26 UTC");
    }

    #[test]
    fn provider_display_falls_back_to_raw_id() {
        assert_eq!(provider_display("deepgram"), "Deepgram");
        assert_eq!(provider_display("openai"), "OpenAI");
        assert_eq!(provider_display("future-provider"), "future-provider");
    }
}
