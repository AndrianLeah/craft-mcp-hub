//! `crafthub-panel`: the control panel window.
//!
//! The window never waits on anything: a worker thread refreshes what the apps are doing every
//! couple of seconds and runs the actions the buttons ask for, one at a time.

#![cfg_attr(windows, windows_subsystem = "windows")]

use crafthub::hub::{self, AddOptions, App, AppState, Hub, Origin, StateKind};
use crafthub::{catalog, clients, doctor};
use eframe::egui::{self, Color32, RichText};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

const GREEN: Color32 = Color32::from_rgb(61, 214, 140);
const YELLOW: Color32 = Color32::from_rgb(245, 185, 53);
const RED: Color32 = Color32::from_rgb(240, 67, 90);
const MUTED: Color32 = Color32::from_rgb(138, 140, 148);

#[derive(Clone)]
struct Row {
    app: App,
    state: AppState,
}

struct CheckResult {
    ok: bool,
    text: String,
    at: Instant,
}

struct Diagnosis {
    id: String,
    name: String,
    /// `None` while the checks run.
    text: Option<String>,
}

#[derive(Default)]
struct Shared {
    rows: Vec<Row>,
    message: String,
    /// What the worker is doing right now; buttons are disabled meanwhile.
    busy: Option<String>,
    checks: HashMap<String, CheckResult>,
    diagnosis: Option<Diagnosis>,
    error: Option<String>,
}

enum Job {
    Add(String),
    AddCustom { id: String, options: AddOptions },
    Remove(String),
    TurnOn(String),
    TurnOff(String),
    SetPort(String, u16),
    SetRoot(String, String),
    CloseLibrary(String),
    Diagnose(String),
    SetUpNew,
    AllOn,
    AllOff,
    CheckAll,
    UpdateCatalog,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------------------------------------------------------------- worker

fn refresh(shared: &Mutex<Shared>) {
    // Claude rewrites its settings often: when they can't be read this instant, keep what we have.
    let Some(servers) = clients::registered() else { return };
    let rows: Vec<Row> = Hub::load().apps().into_iter().map(|app| Row { state: hub::state(&app, Some(&servers)), app }).collect();
    lock(shared).rows = rows;
}

fn worker(shared: Arc<Mutex<Shared>>, jobs: Receiver<Job>, ctx: egui::Context) {
    loop {
        refresh(&shared);
        ctx.request_repaint();
        match jobs.recv_timeout(Duration::from_secs(2)) {
            Ok(job) => run(job, &shared, &ctx),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn run(job: Job, shared: &Mutex<Shared>, ctx: &egui::Context) {
    let busy = |text: String| {
        lock(shared).busy = Some(text);
        ctx.request_repaint();
    };
    let name = |id: &str| lock(shared).rows.iter().find(|r| r.app.id == id).map_or(id.to_string(), |r| r.app.name.clone());
    // Runs one action on several apps; returns what went wrong, app by app.
    let on_each = |ids: Vec<String>, verb: &str, action: &dyn Fn(&str) -> Result<(), String>| {
        let mut errors = Vec::new();
        for id in &ids {
            busy(format!("{verb} {}…", name(id)));
            if let Err(why) = action(id) {
                errors.push(format!("{}: {why}", name(id)));
            }
        }
        if errors.is_empty() {
            Ok(ids.len())
        } else {
            Err(errors.join("\n\n"))
        }
    };
    let ids_where = |keep: &dyn Fn(&Row) -> bool| -> Vec<String> { lock(shared).rows.iter().filter(|r| keep(r)).map(|r| r.app.id.clone()).collect() };
    let session_hint = "Open a new AI client session to use it.";

    let outcome: Result<String, String> = match job {
        Job::Add(id) | Job::TurnOn(id) => {
            busy(format!("Turning on {}…", name(&id)));
            hub::turn_on(&id).map(|()| format!("{} is on. {session_hint}", name(&id)))
        }
        Job::AddCustom { id, options } => {
            busy(format!("Adding {id}…"));
            hub::add(&id, options).and_then(|app| hub::turn_on(&app.id).map(|()| format!("{} is on, port {}. {session_hint}", app.name, app.port)))
        }
        Job::Remove(id) => {
            busy(format!("Removing {}…", name(&id)));
            hub::remove(&id).map(|()| format!("{id} removed."))
        }
        Job::TurnOff(id) => {
            busy(format!("Turning off {}…", name(&id)));
            hub::turn_off(&id).map(|()| format!("{} is off.", name(&id)))
        }
        Job::SetPort(id, port) => {
            busy(format!("Moving {} to port {port}…", name(&id)));
            hub::set_port(&id, port).map(|()| format!("{} is on port {port}.", name(&id)))
        }
        Job::SetRoot(id, dir) => {
            busy(format!("Changing the file folder of {}…", name(&id)));
            hub::set_root(&id, &dir).map(|()| format!("{} may now read and write {dir}.", name(&id)))
        }
        Job::CloseLibrary(id) => {
            busy(format!("Closing the library of {}…", name(&id)));
            hub::close_library(&id).map(|()| format!("{}: library closed, the desktop app can open it now.", name(&id)))
        }
        Job::Diagnose(id) => {
            lock(shared).diagnosis = Some(Diagnosis { id: id.clone(), name: name(&id), text: None });
            busy(format!("Checking {}…", name(&id)));
            let text = doctor::report(&id);
            if let Some(diagnosis) = lock(shared).diagnosis.as_mut().filter(|d| d.id == id) {
                diagnosis.text = Some(text);
            }
            Ok(String::new())
        }
        Job::SetUpNew => on_each(ids_where(&|r| r.state.kind == StateKind::New), "Turning on", &|id| hub::turn_on(id))
            .map(|n| format!("{n} new app(s) set up. {session_hint}")),
        Job::AllOn => on_each(ids_where(&|r| r.app.configured && r.app.installed()), "Turning on", &|id| hub::turn_on(id)).map(|n| format!("{n} app(s) on.")),
        Job::AllOff => on_each(ids_where(&|r| r.state.is_on()), "Turning off", &|id| hub::turn_off(id)).map(|n| format!("{n} app(s) off.")),
        Job::CheckAll => {
            let ids = ids_where(&|r| r.state.is_on());
            let mut failed = 0;
            for id in &ids {
                busy(format!("Checking {}…", name(id)));
                let result = match hub::check(id) {
                    Ok(ok) => CheckResult { ok: true, text: format!("{} tools in {} ms", ok.tools, ok.millis), at: Instant::now() },
                    Err(why) => {
                        failed += 1;
                        CheckResult { ok: false, text: why, at: Instant::now() }
                    }
                };
                lock(shared).checks.insert(id.clone(), result);
            }
            Ok(match (ids.len(), failed) {
                (0, _) => "No app is on.".to_string(),
                (n, 0) => format!("All {n} apps that are on answer."),
                (_, failed) => format!("{failed} app(s) have a problem: open their Diagnose."),
            })
        }
        Job::UpdateCatalog => {
            busy("Fetching the app catalog…".to_string());
            catalog::update().map(|n| format!("Catalog updated: {n} apps."))
        }
    };

    refresh(shared);
    let mut state = lock(shared);
    state.busy = None;
    match outcome {
        Ok(message) if message.is_empty() => {}
        Ok(message) => state.message = message,
        Err(why) => {
            state.message = "Something went wrong.".to_string();
            state.error = Some(why);
        }
    }
    drop(state);
    ctx.request_repaint();
}

// ---------------------------------------------------------------- window

#[derive(Default)]
struct CustomForm {
    id: String,
    name: String,
    cli: String,
    like: String,
    settings: String,
}

struct Panel {
    shared: Arc<Mutex<Shared>>,
    jobs: Sender<Job>,
    /// Port fields being edited, by app.
    ports: HashMap<String, u16>,
    custom: Option<CustomForm>,
    /// The file-folder window: app id, app name, the folder being typed.
    root_edit: Option<(String, String, String)>,
    /// For taking a picture of the panel: where to save it, and how far along we are.
    screenshot: Option<PathBuf>,
    frames: u32,
    shot_requested: bool,
}

impl Panel {
    fn new(cc: &eframe::CreationContext<'_>) -> Panel {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        cc.egui_ctx.style_mut(|style| {
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            style.spacing.button_padding = egui::vec2(10.0, 5.0);
        });
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (jobs, queue) = mpsc::channel();
        let (for_worker, ctx) = (shared.clone(), cc.egui_ctx.clone());
        thread::spawn(move || worker(for_worker, queue, ctx));
        // Used to take the pictures in the README: save one of the window and close, optionally
        // with an app's diagnosis open.
        let screenshot = std::env::var_os("CRAFTHUB_PANEL_SCREENSHOT").map(PathBuf::from);
        if let Some(id) = std::env::var("CRAFTHUB_PANEL_DIAGNOSE").ok().filter(|_| screenshot.is_some()) {
            let _ = jobs.send(Job::Diagnose(id));
        }
        Panel { shared, jobs, ports: HashMap::new(), custom: None, root_edit: None, screenshot, frames: 0, shot_requested: false }
    }

    fn send(&self, job: Job) {
        let _ = self.jobs.send(job);
    }

    fn app_row(&mut self, ui: &mut egui::Ui, row: &Row, busy: bool, check: Option<(bool, String)>) {
        let (app, state) = (&row.app, &row.state);

        // Name
        ui.horizontal(|ui| {
            let (bar, _) = ui.allocate_exact_size(egui::vec2(4.0, 38.0), egui::Sense::hover());
            ui.painter().rect_filled(bar, egui::CornerRadius::same(2), Color32::from_rgb(app.color[0], app.color[1], app.color[2]));
            ui.vertical(|ui| {
                ui.set_min_width(250.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&app.name).strong().size(15.0));
                    match app.origin {
                        Origin::Custom => drop(ui.label(RichText::new("custom").small().color(MUTED))),
                        Origin::Community => drop(ui.label(RichText::new("community").small().color(MUTED))),
                        Origin::Official => {}
                    }
                    if state.kind == StateKind::New {
                        ui.label(RichText::new("NEW").small().strong().color(YELLOW));
                    }
                });
                let tagline = if app.tagline.is_empty() { app.id.as_str() } else { app.tagline.as_str() };
                ui.label(RichText::new(crafthub::rpc::clip(tagline, 44)).small().color(MUTED)).on_hover_text(tagline);
            });
        });

        // What the engine works on: a library the AI client opens, or a folder it may use.
        ui.vertical(|ui| {
            ui.set_min_width(250.0);
            if app.profile.library {
                ui.label(RichText::new("LIBRARY").small().color(MUTED));
                ui.horizontal(|ui| match &state.library {
                    Some(library) => {
                        let name = std::path::Path::new(library).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| library.clone());
                        ui.label(name).on_hover_text(library);
                        if ui.add_enabled(!busy, egui::Button::new("Release").small()).on_hover_text("Close it, so the desktop app can open it").clicked() {
                            self.send(Job::CloseLibrary(app.id.clone()));
                        }
                    }
                    None if state.is_on() => drop(ui.label(RichText::new("none (the AI client opens one)").color(MUTED))),
                    None => drop(ui.label(RichText::new("—").color(MUTED))),
                });
            } else if let Some(root) = app.root() {
                ui.label(RichText::new("FILES IT MAY USE").small().color(MUTED));
                ui.horizontal(|ui| {
                    ui.label(crafthub::rpc::clip(&root, 30)).on_hover_text(&root);
                    if ui.add_enabled(!busy, egui::Button::new("Change…").small()).clicked() {
                        self.root_edit = Some((app.id.clone(), app.name.clone(), root.clone()));
                    }
                });
            }
        });

        // Port
        ui.vertical(|ui| {
            ui.label(RichText::new("PORT").small().color(MUTED));
            if app.port == 0 || !app.installed() {
                ui.label(RichText::new("—").color(MUTED));
            } else {
                let mut port = *self.ports.get(&app.id).unwrap_or(&app.port);
                let field = ui.add_enabled(!busy, egui::DragValue::new(&mut port).range(1024..=65535).speed(0.1));
                if field.has_focus() || field.dragged() {
                    self.ports.insert(app.id.clone(), port);
                } else {
                    self.ports.remove(&app.id);
                }
                if (field.lost_focus() || field.drag_stopped()) && port != app.port {
                    self.send(Job::SetPort(app.id.clone(), port));
                }
            }
        });

        // State
        ui.vertical(|ui| {
            ui.set_min_width(190.0);
            let color = match state.kind {
                StateKind::On => GREEN,
                StateKind::Starting | StateKind::New | StateKind::Other => YELLOW,
                StateKind::Error | StateKind::PortBusy => RED,
                _ => MUTED,
            };
            ui.horizontal(|ui| {
                // Painted, not a character: the built-in font has no round bullet.
                let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 14.0), egui::Sense::hover());
                if matches!(state.kind, StateKind::Off | StateKind::NotInstalled) {
                    ui.painter().circle_stroke(dot.center(), 4.0, egui::Stroke::new(1.5_f32, color));
                } else {
                    ui.painter().circle_filled(dot.center(), 4.5, color);
                }
                let label = ui.label(RichText::new(&state.label).strong().color(color));
                if !state.tip.is_empty() {
                    label.on_hover_text(&state.tip);
                }
            });
            match check.filter(|_| state.is_on()) {
                Some((ok, text)) => {
                    let verdict = if ok { "checked:" } else { "check failed:" };
                    ui.label(RichText::new(format!("{verdict} {}", crafthub::rpc::clip(&text, 30))).small().color(if ok { GREEN } else { RED }))
                        .on_hover_text(text);
                }
                None => {
                    let detail = if state.stale { "restart to apply settings" } else { state.detail.as_str() };
                    ui.label(RichText::new(detail).small().color(MUTED));
                }
            }
        });

        // Actions
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new("Diagnose")).clicked() {
                self.send(Job::Diagnose(app.id.clone()));
            }
            let (text, job) = match state.kind {
                StateKind::New => ("Add", Job::Add(app.id.clone())),
                StateKind::Off | StateKind::Other | StateKind::NotInstalled => ("Turn on", Job::TurnOn(app.id.clone())),
                _ => ("Turn off", Job::TurnOff(app.id.clone())),
            };
            let primary = matches!(state.kind, StateKind::New | StateKind::Off | StateKind::Other);
            let mut button = egui::Button::new(RichText::new(text).strong()).min_size(egui::vec2(76.0, 0.0));
            if primary {
                button = button.fill(Color32::from_rgb(52, 92, 168));
            }
            if ui.add_enabled(!busy && app.installed(), button).clicked() {
                self.send(job);
            }
            if state.stale && ui.add_enabled(!busy, egui::Button::new("Restart")).on_hover_text(&state.tip).clicked() {
                self.send(Job::TurnOn(app.id.clone()));
            }
            if app.origin == Origin::Custom && !state.is_on() && ui.add_enabled(!busy, egui::Button::new("Remove")).clicked() {
                self.send(Job::Remove(app.id.clone()));
            }
        });
        ui.end_row();
    }

    fn windows(&mut self, ctx: &egui::Context, busy: bool) {
        // Diagnosis
        let diagnosis = lock(&self.shared).diagnosis.as_ref().map(|d| (d.id.clone(), d.name.clone(), d.text.clone()));
        if let Some((id, name, text)) = diagnosis {
            let mut open = true;
            egui::Window::new(format!("Diagnosis · {name}")).id(egui::Id::new("diagnosis")).open(&mut open).default_size([860.0, 520.0]).show(ctx, |ui| {
                let shown = text.clone().unwrap_or_else(|| "Checking…".to_string());
                egui::ScrollArea::vertical().max_height(ui.available_height() - 44.0).show(ui, |ui| {
                    ui.add(egui::TextEdit::multiline(&mut shown.as_str()).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(!busy, egui::Button::new("Check again")).clicked() {
                        self.send(Job::Diagnose(id.clone()));
                    }
                    if ui.button("Copy").clicked() {
                        ctx.copy_text(shown.clone());
                    }
                });
            });
            if !open {
                lock(&self.shared).diagnosis = None;
            }
        }

        // What went wrong
        let error = lock(&self.shared).error.clone();
        if let Some(error) = error {
            egui::Window::new("Something went wrong").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.set_max_width(560.0);
                ui.label(&error);
                ui.add_space(8.0);
                if ui.button("OK").clicked() {
                    lock(&self.shared).error = None;
                }
            });
        }

        // The folder an engine may use
        let mut close_root = false;
        if let Some((id, name, dir)) = &mut self.root_edit {
            let mut open = true;
            egui::Window::new(format!("Files {name} may use")).open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
                ui.label("The one folder (and everything in it) the engine may read and write:");
                ui.add(egui::TextEdit::singleline(dir).desired_width(460.0));
                ui.add_space(6.0);
                if ui.add_enabled(!busy && !dir.trim().is_empty(), egui::Button::new("Use this folder")).clicked() {
                    let _ = self.jobs.send(Job::SetRoot(id.clone(), dir.trim().to_string()));
                    close_root = true;
                }
            });
            close_root |= !open;
        }
        if close_root {
            self.root_edit = None;
        }

        // A custom app
        let mut close_custom = false;
        if let Some(form) = &mut self.custom {
            // A file dropped on the window is the app's command-line tool.
            if let Some(dropped) = ctx.input(|i| i.raw.dropped_files.first().and_then(|f| f.path.clone())) {
                form.cli = dropped.to_string_lossy().into_owned();
                if form.id.is_empty() {
                    let stem = dropped.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
                    form.id = stem.trim_end_matches("-cli").replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "-");
                }
            }
            let mut open = true;
            egui::Window::new("Add a custom app").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
                ui.label("A fork or your own build of an Artcraft app. Its command-line tool must have an `mcp` command.");
                ui.add_space(6.0);
                egui::Grid::new("custom-form").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                    ui.label("Command-line tool");
                    ui.add(egui::TextEdit::singleline(&mut form.cli).hint_text("full path to …-cli — or drop the file on this window").desired_width(430.0));
                    ui.end_row();
                    ui.label("Id");
                    ui.add(egui::TextEdit::singleline(&mut form.id).hint_text("short name the AI client sees, e.g. chromaforge").desired_width(430.0));
                    ui.end_row();
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(&mut form.name).hint_text("optional").desired_width(430.0));
                    ui.end_row();
                    ui.label("Start it like");
                    egui::ComboBox::from_id_salt("like")
                        .selected_text(if form.like.is_empty() { "work it out from its --help" } else { form.like.as_str() })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut form.like, String::new(), "work it out from its --help");
                            for known in ["lightcraft", "vectorcraft", "photocraft", "figcraft"] {
                                ui.selectable_value(&mut form.like, known.to_string(), known);
                            }
                        });
                    ui.end_row();
                    ui.label("Settings folder");
                    ui.add(
                        egui::TextEdit::singleline(&mut form.settings).hint_text("optional: where a photo app lists its recent libraries").desired_width(430.0),
                    );
                    ui.end_row();
                });
                ui.add_space(8.0);
                let ready = !form.cli.trim().is_empty() && !form.id.trim().is_empty();
                if ui.add_enabled(!busy && ready, egui::Button::new(RichText::new("Add and turn on").strong())).clicked() {
                    let text = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
                    let options = AddOptions {
                        cli: text(&form.cli.replace('"', "")),
                        name: text(&form.name),
                        like: text(&form.like),
                        port: None,
                        settings: text(&form.settings).into_iter().collect(),
                    };
                    let _ = self.jobs.send(Job::AddCustom { id: form.id.trim().to_lowercase(), options });
                    close_custom = true;
                }
            });
            close_custom |= !open;
        }
        if close_custom {
            self.custom = None;
        }
    }

    /// Saves a picture of the window once it has something to show, then closes it.
    fn take_screenshot(&mut self, ctx: &egui::Context, ready: bool) {
        let Some(path) = self.screenshot.clone() else { return };
        self.frames += 1;
        if ready && self.frames > 40 && !self.shot_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            self.shot_requested = true;
        }
        let image = ctx.input(|i| {
            i.events.iter().find_map(|event| match event {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            if let Ok(file) = std::fs::File::create(&path) {
                let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), image.size[0] as u32, image.size[1] as u32);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                if let Ok(mut writer) = encoder.write_header() {
                    let _ = writer.write_image_data(image.as_raw());
                }
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(Duration::from_millis(50));
    }
}

impl eframe::App for Panel {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let (rows, message, busy_text, checks) = {
            let mut shared = lock(&self.shared);
            shared.checks.retain(|_, check| check.at.elapsed() < Duration::from_secs(90));
            let checks: HashMap<String, (bool, String)> = shared.checks.iter().map(|(id, c)| (id.clone(), (c.ok, c.text.clone()))).collect();
            (shared.rows.clone(), shared.message.clone(), shared.busy.clone(), checks)
        };
        let busy = busy_text.is_some();
        let new_apps = rows.iter().filter(|r| r.state.kind == StateKind::New).count();
        let (present, missing): (Vec<&Row>, Vec<&Row>) = rows.iter().partition(|r| r.app.installed() || r.app.configured);

        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.label(RichText::new("Craft MCP Hub").size(22.0).strong());
            ui.label(
                RichText::new(
                    "Each app that is on has one windowless server on its own port, shared by every AI client session. \
                     A session starts the server when it isn't running; closing this window stops nothing.",
                )
                .color(MUTED),
            );
            ui.add_space(6.0);
        });

        egui::TopBottomPanel::bottom("footer").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let set_up = egui::Button::new(RichText::new(format!("Set up new apps ({new_apps})")).strong()).fill(Color32::from_rgb(52, 92, 168));
                if ui.add_enabled(!busy && new_apps > 0, set_up).on_hover_text("Add and turn on every installed app that isn't in the hub yet").clicked() {
                    self.send(Job::SetUpNew);
                }
                if ui.add_enabled(!busy, egui::Button::new("Turn all on")).clicked() {
                    self.send(Job::AllOn);
                }
                if ui.add_enabled(!busy, egui::Button::new("Turn all off")).clicked() {
                    self.send(Job::AllOff);
                }
                if ui.add_enabled(!busy, egui::Button::new("Check all")).on_hover_text("A full MCP round trip through every server that is on").clicked() {
                    self.send(Job::CheckAll);
                }
                if ui.add_enabled(!busy, egui::Button::new("Add custom app…")).clicked() {
                    self.custom = Some(CustomForm::default());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(busy_text.as_deref().unwrap_or(&message)).color(MUTED));
                });
            });
            ui.label(
                RichText::new(
                    "Photo apps start with no library open: the AI client opens the one it needs (library_list, library_open). \
                     A library opens in one program at a time: Release it here before opening it in the desktop app.",
                )
                .small()
                .color(MUTED),
            );
            ui.add_space(6.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                if rows.is_empty() {
                    ui.add_space(20.0);
                    ui.label(RichText::new("Looking for the apps…").color(MUTED));
                    return;
                }
                egui::Grid::new("apps").num_columns(5).striped(true).spacing([22.0, 12.0]).min_row_height(46.0).show(ui, |ui| {
                    // One line per label: a wrapped status makes the rows uneven.
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    for row in &present {
                        self.app_row(ui, row, busy, checks.get(&row.app.id).cloned());
                    }
                });
                if !missing.is_empty() {
                    ui.add_space(12.0);
                    egui::CollapsingHeader::new(RichText::new(format!("Not installed ({})", missing.len())).color(MUTED)).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("They show up above once installed.").small().color(MUTED));
                            if ui
                                .add_enabled(!busy, egui::Button::new("Refresh the catalog").small())
                                .on_hover_text("Fetch the current app list from getartcraft.com")
                                .clicked()
                            {
                                self.send(Job::UpdateCatalog);
                            }
                        });
                        for row in &missing {
                            ui.label(format!("{}  ·  {}", row.app.name, row.app.tagline));
                        }
                    });
                }
            });
        });

        self.windows(ctx, busy);
        self.take_screenshot(ctx, !rows.is_empty() && !busy);
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("Craft MCP Hub").with_inner_size([1080.0, 780.0]).with_min_inner_size([900.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native("craft-mcp-hub", options, Box::new(|cc| Ok(Box::new(Panel::new(cc)))))
}
