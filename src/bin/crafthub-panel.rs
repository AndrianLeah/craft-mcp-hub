//! `crafthub-panel`: the control panel window.
//!
//! The window never waits on anything: a worker thread refreshes what the apps are doing every
//! couple of seconds and runs the actions the buttons ask for, one at a time.
//!
//! It is drawn in the icon's colours, Ink & Jade: solid fills, no shadows or gradients, and egui's
//! own fonts in their one weight, so strong text is ivory rather than heavier. Green means turning
//! something on and nothing else; ivory confirms in a window; red is only for uninstalling.

#![cfg_attr(windows, windows_subsystem = "windows")]

use crafthub::clients::Snapshot;
use crafthub::hub::{self, AddOptions, App, AppState, Hub, Origin, StateKind};
use crafthub::update_check::{self, Newer};
use crafthub::{catalog, clients, doctor, uninstall};
use eframe::egui::{self, pos2, vec2, Align, Color32, CornerRadius, FontId, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------- Ink & Jade

/// Fields and the footer (`extreme_bg_color`).
const INK: Color32 = Color32::from_rgb(0x0B, 0x0B, 0x0C);
/// The window and its panels (`panel_fill`, `window_fill`).
const WINDOW: Color32 = Color32::from_rgb(0x10, 0x13, 0x12);
/// A row of the list, and the summary (`faint_bg_color`).
const ROW: Color32 = Color32::from_rgb(0x16, 0x1A, 0x18);
/// Buttons (`widgets.inactive.weak_bg_fill`), and under the pointer.
const BUTTON: Color32 = Color32::from_rgb(0x1F, 0x24, 0x22);
const BUTTON_HOVER: Color32 = Color32::from_rgb(0x26, 0x2C, 0x29);
/// Separators and borders, and a border under the pointer.
const LINE: Color32 = Color32::from_rgb(0x23, 0x29, 0x26);
const LINE_HOVER: Color32 = Color32::from_rgb(0x34, 0x3C, 0x38);
/// Turning something on: Add, Set up, the On dot, with ink text on it (`selection.bg_fill`).
const JADE: Color32 = Color32::from_rgb(0x1B, 0xA5, 0x72);
const JADE_HOVER: Color32 = Color32::from_rgb(0x22, 0xB5, 0x7E);
/// Jade that reads on the dark: On, checks, Turn on.
const JADE_TEXT: Color32 = Color32::from_rgb(0x3C, 0xC4, 0x8F);
/// Behind what is on: the clients that are on, the update notice, and its border and buttons.
const JADE_WASH: Color32 = Color32::from_rgb(0x10, 0x29, 0x1F);
const JADE_WASH_HOVER: Color32 = Color32::from_rgb(0x14, 0x33, 0x27);
const JADE_LINE: Color32 = Color32::from_rgb(0x1A, 0x5C, 0x43);
const WASH_BUTTON: Color32 = Color32::from_rgb(0x18, 0x3A, 0x2C);
const WASH_BUTTON_HOVER: Color32 = Color32::from_rgb(0x1E, 0x46, 0x36);
/// Strong text: titles, app names. Also what confirms in a window.
const IVORY: Color32 = Color32::from_rgb(0xEF, 0xE9, 0xDC);
const IVORY_HOVER: Color32 = Color32::from_rgb(0xFA, 0xF6, 0xEE);
/// Body text and labels (`widgets.*.fg_stroke`).
const TEXT: Color32 = Color32::from_rgb(0xCF, 0xC9, 0xBC);
/// Taglines, details, Off.
const MUTED: Color32 = Color32::from_rgb(0x8C, 0x88, 0x7F);
/// The headings over the columns and the clients.
const CAPTION: Color32 = Color32::from_rgb(0x61, 0x5E, 0x58);
/// New, warnings.
const AMBER: Color32 = Color32::from_rgb(0xE3, 0xB3, 0x41);
/// Uninstall, errors, a failed check; dimmed for the outline that leads to uninstalling.
const RED: Color32 = Color32::from_rgb(0xD9, 0x66, 0x5F);
const RED_HOVER: Color32 = Color32::from_rgb(0xE2, 0x78, 0x71);
const RED_LINE: Color32 = Color32::from_rgb(0x5D, 0x2F, 0x2D);
const RED_WASH: Color32 = Color32::from_rgb(0x2A, 0x17, 0x16);

/// The type scale. egui's default fonts have one weight: Ubuntu Light, and Hack for numbers.
const TITLE_SIZE: f32 = 24.0;
const NAME_SIZE: f32 = 15.5;
const BODY_SIZE: f32 = 13.0;
const SMALL_SIZE: f32 = 11.5;
const CAPTION_SIZE: f32 = 10.5;

/// Puts Ink & Jade in place of egui's own dark look.
fn ink_and_jade(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        use egui::TextStyle;
        style.text_styles = [
            (TextStyle::Heading, FontId::proportional(TITLE_SIZE)),
            (TextStyle::Body, FontId::proportional(BODY_SIZE)),
            (TextStyle::Button, FontId::proportional(BODY_SIZE)),
            (TextStyle::Small, FontId::proportional(SMALL_SIZE)),
            (TextStyle::Monospace, FontId::monospace(BODY_SIZE)),
        ]
        .into();
        style.spacing.item_spacing = vec2(8.0, 6.0);
        style.spacing.button_padding = vec2(12.0, 6.0);
        style.spacing.interact_size.y = 26.0;
        style.spacing.menu_margin = Margin::same(6);
        // Scroll bars that take their own space, so they never cover a row's buttons.
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        style.visuals = visuals();
    });
}

fn visuals() -> egui::Visuals {
    let widget = |fill: Color32, border: Color32, text: Color32| egui::style::WidgetVisuals {
        bg_fill: fill,
        weak_bg_fill: fill,
        bg_stroke: Stroke::new(1.0_f32, border),
        corner_radius: CornerRadius::same(6),
        fg_stroke: Stroke::new(1.0_f32, text),
        // Flat: nothing grows under the pointer.
        expansion: 0.0,
    };
    let mut visuals = egui::Visuals::dark();
    visuals.widgets.noninteractive = widget(WINDOW, LINE, TEXT);
    visuals.widgets.inactive = widget(BUTTON, LINE, TEXT);
    visuals.widgets.hovered = widget(BUTTON_HOVER, LINE_HOVER, IVORY);
    // Strong text is the pressed widgets' colour: ivory.
    visuals.widgets.active = widget(BUTTON_HOVER, JADE_TEXT, IVORY);
    visuals.widgets.open = widget(BUTTON_HOVER, LINE, TEXT);
    visuals.selection = egui::style::Selection { bg_fill: JADE, stroke: Stroke::new(1.0_f32, JADE_TEXT) };
    visuals.hyperlink_color = JADE_TEXT;
    visuals.faint_bg_color = ROW;
    visuals.extreme_bg_color = INK;
    visuals.code_bg_color = BUTTON;
    visuals.warn_fg_color = AMBER;
    visuals.error_fg_color = RED;
    visuals.panel_fill = WINDOW;
    visuals.window_fill = WINDOW;
    visuals.window_stroke = Stroke::new(1.0_f32, LINE);
    visuals.window_corner_radius = CornerRadius::same(12);
    visuals.menu_corner_radius = CornerRadius::same(8);
    visuals.window_shadow = egui::Shadow::NONE;
    visuals.popup_shadow = egui::Shadow::NONE;
    visuals.window_highlight_topmost = false;
    visuals.indent_has_left_vline = false;
    visuals
}

/// A heading in small capitals, over the columns and before the clients.
fn caption(text: &str) -> RichText {
    RichText::new(text).size(CAPTION_SIZE).color(CAPTION).extra_letter_spacing(1.2)
}

fn line_height(ui: &Ui, size: f32) -> f32 {
    ui.fonts(|fonts| fonts.row_height(&FontId::proportional(size)))
}

/// How a button looks, by what it does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    /// The usual button: a flat fill a shade lighter than what it sits on.
    Plain,
    /// Turning something on, when it is the thing to do: solid jade.
    Primary,
    /// Turning something on: a jade outline.
    Outline,
    /// Confirming in a window: ivory.
    Confirm,
    /// Uninstalling, once asked: solid red.
    Danger,
    /// The way to uninstalling: a red outline.
    DangerOutline,
    /// On the jade of the update notice.
    OnWash,
    /// Text only, for putting something away.
    Quiet,
}

impl Look {
    /// The fill, the border and the text, as it is and under the pointer.
    fn colors(self, hovered: bool) -> (Color32, Color32, Color32) {
        let none = Color32::TRANSPARENT;
        match (self, hovered) {
            (Look::Plain, false) => (BUTTON, none, TEXT),
            (Look::Plain, true) => (BUTTON_HOVER, none, IVORY),
            (Look::Primary, false) => (JADE, none, INK),
            (Look::Primary, true) => (JADE_HOVER, none, INK),
            (Look::Outline, false) => (none, JADE, JADE_TEXT),
            (Look::Outline, true) => (JADE_WASH, JADE_TEXT, JADE_TEXT),
            (Look::Confirm, false) => (IVORY, none, INK),
            (Look::Confirm, true) => (IVORY_HOVER, none, INK),
            (Look::Danger, false) => (RED, none, INK),
            (Look::Danger, true) => (RED_HOVER, none, INK),
            (Look::DangerOutline, false) => (none, RED_LINE, RED),
            (Look::DangerOutline, true) => (RED_WASH, RED, RED),
            (Look::OnWash, false) => (WASH_BUTTON, none, TEXT),
            (Look::OnWash, true) => (WASH_BUTTON_HOVER, none, IVORY),
            (Look::Quiet, false) => (none, none, MUTED),
            (Look::Quiet, true) => (BUTTON, none, TEXT),
        }
    }
}

/// A flat button. Painted here rather than by egui's own, which keeps a set fill and text colour
/// the same under the pointer.
struct Flat<'a> {
    text: &'a str,
    look: Look,
    size: f32,
    height: f32,
    min_width: f32,
    padding: f32,
}

impl<'a> Flat<'a> {
    fn new(text: &'a str, look: Look) -> Flat<'a> {
        Flat { text, look, size: BODY_SIZE, height: 30.0, min_width: 0.0, padding: 12.0 }
    }

    /// The smaller button that goes with a value in a row: Change…, Release.
    fn small(self) -> Self {
        Flat { size: 12.0, height: 20.0, padding: 7.0, ..self }
    }

    fn height(self, height: f32) -> Self {
        Flat { height, ..self }
    }

    fn min_width(self, min_width: f32) -> Self {
        Flat { min_width, ..self }
    }

    fn padding(self, padding: f32) -> Self {
        Flat { padding, ..self }
    }
}

impl egui::Widget for Flat<'_> {
    fn ui(self, ui: &mut Ui) -> egui::Response {
        let galley = ui.painter().layout_no_wrap(self.text.to_string(), FontId::proportional(self.size), TEXT);
        let size = vec2((galley.size().x + 2.0 * self.padding).max(self.min_width), self.height);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let enabled = ui.is_enabled();
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, self.text));
        if ui.is_rect_visible(rect) {
            let (mut fill, mut border, mut text) = self.look.colors(enabled && (response.hovered() || response.has_focus()));
            if !enabled {
                (fill, border, text) = (fill.gamma_multiply(0.45), border.gamma_multiply(0.45), text.gamma_multiply(0.45));
            }
            let painter = ui.painter();
            painter.rect(rect, CornerRadius::same(6), fill, Stroke::new(1.0_f32, border), StrokeKind::Inside);
            painter.galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, text);
        }
        response
    }
}

/// A tick, drawn: egui's default fonts have none that thin.
fn paint_check(painter: &egui::Painter, center: Pos2, width: f32, color: Color32) {
    let (w, h) = (width, width * 0.72);
    let points = vec![center + vec2(-w / 2.0, 0.0), center + vec2(-w * 0.15, h / 2.0), center + vec2(w / 2.0, -h / 2.0)];
    painter.add(egui::Shape::line(points, Stroke::new(1.4_f32, color)));
}

/// A status dot: filled, or a ring for what is off.
fn dot(ui: &mut Ui, color: Color32, ring: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(10.0, 14.0), Sense::hover());
    if ring {
        ui.painter().circle_stroke(rect.center(), 3.8, Stroke::new(1.5_f32, color));
    } else {
        ui.painter().circle_filled(rect.center(), 4.5, color);
    }
}

/// The outline of a pill in dashes, for what isn't there.
fn dashed_pill(painter: &egui::Painter, rect: Rect, stroke: Stroke) {
    use std::f32::consts::{FRAC_PI_2, PI};
    let radius = rect.height() / 2.0;
    let mut points = vec![pos2(rect.left() + radius, rect.top())];
    // The right end from top to bottom, then the left end from bottom to top.
    for (center, from) in [(pos2(rect.right() - radius, rect.center().y), -FRAC_PI_2), (pos2(rect.left() + radius, rect.center().y), FRAC_PI_2)] {
        for step in 0..=12 {
            let angle = from + PI * step as f32 / 12.0;
            points.push(center + radius * vec2(angle.cos(), angle.sin()));
        }
    }
    points.push(points[0]);
    painter.extend(egui::Shape::dashed_line(&points, stroke, 3.0, 3.0));
}

/// One AI client at the top: jade with a tick when the apps are registered with it, dashed when it
/// isn't on this computer.
fn client_chip(ui: &mut Ui, client: &ClientRow) -> egui::Response {
    let painter = ui.painter().clone();
    let name = painter.layout_no_wrap(client.name.to_string(), FontId::proportional(BODY_SIZE), TEXT);
    let missing = !client.found && !client.enabled;
    let note = missing.then(|| painter.layout_no_wrap("not installed".to_string(), FontId::proportional(SMALL_SIZE), CAPTION));
    let tick = if client.enabled { 16.0 } else { 0.0 };
    let width = 12.0 + tick + name.size().x + note.as_ref().map_or(0.0, |note| 8.0 + note.size().x) + 12.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, 27.0), Sense::click());
    let enabled = ui.is_enabled();
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, client.enabled, client.name));
    let hovered = enabled && response.hovered();
    let fade = |color: Color32| if enabled || missing { color } else { color.gamma_multiply(0.6) };
    let radius = CornerRadius::same(14);
    if client.enabled {
        painter.rect(rect, radius, fade(if hovered { JADE_WASH_HOVER } else { JADE_WASH }), Stroke::new(1.0_f32, fade(JADE)), StrokeKind::Inside);
    } else if missing {
        dashed_pill(&painter, rect.shrink(0.5), Stroke::new(1.0_f32, LINE_HOVER));
    } else {
        painter.rect_filled(rect, radius, fade(if hovered { BUTTON_HOVER } else { BUTTON }));
    }
    let mut x = rect.left() + 12.0;
    if client.enabled {
        paint_check(&painter, pos2(x + 5.0, rect.center().y), 9.0, fade(JADE_TEXT));
        x += tick;
    }
    let name_width = name.size().x;
    painter.galley_with_override_text_color(pos2(x, rect.center().y - name.size().y / 2.0), name, fade(if missing { MUTED } else { TEXT }));
    if let Some(note) = note {
        let at = pos2(x + name_width + 8.0, rect.center().y - note.size().y / 2.0);
        painter.galley_with_override_text_color(at, note, CAPTION);
    }
    response
}

/// A label that takes what its text needs, up to `max_width`, and is cut with an ellipsis beyond.
fn label_up_to(ui: &mut Ui, max_width: f32, text: RichText) -> egui::Response {
    ui.scope(|ui| {
        ui.set_max_width(max_width.max(30.0));
        ui.label(text)
    })
    .inner
}

// ---------------------------------------------------------------- the list's measures

/// A row: its height, the space between rows, the margin inside it, and the app's colour bar.
const ROW_HEIGHT: f32 = 58.0;
/// A row in a narrow window, on two lines.
const COMPACT_ROW_HEIGHT: f32 = 92.0;
const ROW_GAP: f32 = 4.0;
const ROW_PAD: f32 = 12.0;
const BAR_WIDTH: f32 = 3.0;
/// The columns, and the space between them.
const APP_WIDTH: f32 = 208.0;
const FILES_WIDTH: f32 = 222.0;
const PORT_WIDTH: f32 = 76.0;
const STATE_WIDTH: f32 = 156.0;
const GAP: f32 = 12.0;
/// The row's buttons: ⋯, Diagnose and the main one.
const ACTIONS_WIDTH: f32 = 29.0 + 6.0 + 78.0 + 6.0 + 84.0;
/// In a narrow window the app and file columns give way down to this, then a row takes two lines.
const NARROW_WIDTH: f32 = 140.0;
/// On two lines, the port has its heading before it.
const PORT_HEADING_WIDTH: f32 = 44.0;

/// Where the columns are across a row: the same for the headings and every row.
#[derive(Clone, Copy)]
struct Columns {
    width: f32,
    /// Two lines a row: the name, the state and the buttons, then the files and the port under them.
    compact: bool,
    app: (f32, f32),
    files: (f32, f32),
    port: f32,
    state: f32,
}

impl Columns {
    fn new(available: f32) -> Columns {
        let lead = ROW_PAD + BAR_WIDTH + GAP;
        let fixed = lead + PORT_WIDTH + STATE_WIDTH + ACTIONS_WIDTH + 4.0 * GAP + ROW_PAD;
        if available < fixed + 2.0 * NARROW_WIDTH {
            // Below this the list scrolls sideways.
            let width = available.max(lead + NARROW_WIDTH + STATE_WIDTH + ACTIONS_WIDTH + 2.0 * GAP + ROW_PAD);
            let state = width - ROW_PAD - ACTIONS_WIDTH - GAP - STATE_WIDTH;
            let port = width - ROW_PAD - PORT_WIDTH;
            let files = port - PORT_HEADING_WIDTH - GAP - lead;
            return Columns { width, compact: true, app: (lead, state - GAP - lead), files: (lead, files), port, state };
        }
        let scale = ((available - fixed) / (APP_WIDTH + FILES_WIDTH)).min(1.0);
        let (app, files) = ((APP_WIDTH * scale).max(NARROW_WIDTH), (FILES_WIDTH * scale).max(NARROW_WIDTH));
        let files_x = lead + app + GAP;
        let port = files_x + files + GAP;
        Columns { width: available.max(fixed + app + files), compact: false, app: (lead, app), files: (files_x, files), port, state: port + PORT_WIDTH + GAP }
    }
}

/// A part of a row: from `x` across it, `width` wide, with content `height` high centred in it.
fn cell(ui: &mut Ui, row: Rect, x: f32, width: f32, height: f32, layout: Layout) -> Ui {
    let top = row.top() + ((row.height() - height) / 2.0).max(0.0);
    let rect = Rect::from_min_size(pos2(row.left() + x, top), vec2(width.max(0.0), height));
    let mut cell = ui.new_child(UiBuilder::new().max_rect(rect).layout(layout));
    // A label that doesn't fit its column is cut with an ellipsis instead of pushing the others.
    cell.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
    cell
}

// ---------------------------------------------------------------- what the worker shares

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

/// An AI client the hub can register apps with, for the chips at the top.
#[derive(Clone)]
struct ClientRow {
    id: &'static str,
    name: &'static str,
    found: bool,
    enabled: bool,
    needs_restart: bool,
}

#[derive(Default)]
struct Shared {
    rows: Vec<Row>,
    clients: Vec<ClientRow>,
    message: String,
    /// What the worker is doing right now; buttons are disabled meanwhile.
    busy: Option<String>,
    checks: HashMap<String, CheckResult>,
    diagnosis: Option<Diagnosis>,
    error: Option<String>,
    /// What uninstalling did, once it is done: the panel then only says so and closes.
    uninstalled: Option<String>,
    /// A newer Craft MCP Hub on GitHub, once the check has found one.
    newer: Option<Newer>,
}

enum Job {
    Add(String),
    AddCustom { id: String, options: AddOptions },
    Remove(String),
    TurnOn(String),
    TurnOff(String),
    Restart(String),
    SetPort(String, u16),
    SetRoots(String, Vec<String>),
    CloseLibrary(String),
    Diagnose(String),
    SetUpNew,
    AllOn,
    AllOff,
    CheckAll,
    UpdateCatalog,
    SetClient(&'static str, bool),
    Uninstall,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------------------------------------------------------------- worker

fn refresh(shared: &Mutex<Shared>) {
    let hub = Hub::load();
    let registered = Snapshot::take(&hub.config);
    // Clients rewrite their settings often: when none can be read this instant, keep what we have.
    if registered.unreadable() {
        return;
    }
    let rows: Vec<Row> = hub.apps().into_iter().map(|app| Row { state: hub::state(&app, &registered), app }).collect();
    let enabled: Vec<&str> = registered.clients().map(|client| client.id).collect();
    let client_rows: Vec<ClientRow> = clients::all()
        .iter()
        .map(|client| ClientRow {
            id: client.id,
            name: client.name,
            found: client.detected(),
            enabled: enabled.contains(&client.id),
            needs_restart: client.needs_restart,
        })
        .collect();
    let mut shared = lock(shared);
    shared.rows = rows;
    shared.clients = client_rows;
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

    if let Job::Uninstall = job {
        busy("Uninstalling Craft MCP Hub…".to_string());
        let mut report: Vec<String> = uninstall::run()
            .into_iter()
            .map(|(what, outcome)| match outcome {
                Ok(()) => format!("✔ {what}"),
                Err(why) => format!("✘ {why}"),
            })
            .collect();
        report.extend(uninstall::left_over());
        let mut state = lock(shared);
        state.busy = None;
        state.uninstalled = Some(report.join("\n"));
        drop(state);
        ctx.request_repaint();
        return;
    }

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
        Job::Restart(id) => {
            busy(format!("Restarting {}…", name(&id)));
            hub::restart(&id).map(|()| format!("{} restarted.", name(&id)))
        }
        Job::SetPort(id, port) => {
            busy(format!("Moving {} to port {port}…", name(&id)));
            hub::set_port(&id, port).map(|()| format!("{} is on port {port}.", name(&id)))
        }
        Job::SetRoots(id, dirs) => {
            busy(format!("Changing the files {} may use…", name(&id)));
            hub::set_roots(&id, &dirs).map(|()| match dirs.as_slice() {
                [] => format!("{} may now use any file you can.", name(&id)),
                dirs => format!("{} may now read and write only {}.", name(&id), dirs.join(", ")),
            })
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
        Job::Uninstall => unreachable!("handled above"),
        Job::SetClient(id, on) => {
            let client = clients::get(id);
            let client_name = client.map_or(id, |c| c.name);
            if on {
                busy(format!("Registering the apps with {client_name}…"));
                hub::enable_client(id).map(|apps| {
                    let restart = if client.is_some_and(|c| c.needs_restart) { format!(" Restart {client_name} to load them.") } else { String::new() };
                    match apps.len() {
                        0 => format!("{client_name} enabled. Turn an app on to register it there."),
                        n => format!("{client_name} enabled: {n} app(s) registered.{restart}"),
                    }
                })
            } else {
                busy(format!("Taking the apps out of {client_name}…"));
                hub::disable_client(id).map(|()| format!("{client_name} disabled: the hub's servers were taken out of it."))
            }
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

/// The window for choosing the folders an app's engine may use.
struct RootEdit {
    id: String,
    name: String,
    /// The folders being typed, one per line.
    folders: String,
    /// Whether the engine takes more than one folder.
    several: bool,
    /// Whether the engine needs a folder to have any file access.
    required: bool,
}

struct Panel {
    shared: Arc<Mutex<Shared>>,
    jobs: Sender<Job>,
    /// The hub's icon, for the header.
    icon: Option<egui::TextureHandle>,
    /// Where the columns were last drawn, for the headings over them.
    columns: Option<Columns>,
    /// Port fields being edited, by app: what is typed so far.
    ports: HashMap<String, String>,
    custom: Option<CustomForm>,
    /// The file-folder window: app id, app name, the folder being typed.
    root_edit: Option<RootEdit>,
    /// The uninstall confirmation, with what uninstalling would do.
    uninstall: Option<Vec<String>>,
    /// The new-version notice was put away until the panel is next opened.
    newer_dismissed: bool,
    /// For taking a picture of the panel: where to save it, and how far along we are.
    screenshot: Option<PathBuf>,
    frames: u32,
    shot_requested: bool,
}

impl Panel {
    fn new(cc: &eframe::CreationContext<'_>) -> Panel {
        ink_and_jade(&cc.egui_ctx);
        let icon = decode_png(include_bytes!("../../assets/icons/linux/hicolor/96x96/apps/craft-mcp-hub.png")).map(|(rgba, width, height)| {
            let image = egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba);
            let smooth = egui::TextureOptions { mipmap_mode: Some(egui::TextureFilter::Linear), ..egui::TextureOptions::LINEAR };
            cc.egui_ctx.load_texture("icon", image, smooth)
        });
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (jobs, queue) = mpsc::channel();
        // Made-up apps for the pictures: nothing runs and nothing is saved.
        if let Some(scene) = std::env::var("CRAFTHUB_PANEL_DEMO").ok().filter(|scene| !scene.is_empty()) {
            *lock(&shared) = demo::shared(&scene);
        } else {
            let (for_worker, ctx) = (shared.clone(), cc.egui_ctx.clone());
            thread::spawn(move || worker(for_worker, queue, ctx));
            // Whether a newer hub is out: asked in the background, at most once a day.
            let (for_check, ctx) = (shared.clone(), cc.egui_ctx.clone());
            thread::spawn(move || {
                if let Ok(Some(newer)) = update_check::newer(false) {
                    lock(&for_check).newer = Some(newer);
                    ctx.request_repaint();
                }
            });
        }
        // Used to take the pictures in the README: save one of the window and close, optionally
        // with an app's diagnosis open.
        let screenshot = std::env::var_os("CRAFTHUB_PANEL_SCREENSHOT").map(PathBuf::from);
        if let Some(id) = std::env::var("CRAFTHUB_PANEL_DIAGNOSE").ok().filter(|_| screenshot.is_some()) {
            let _ = jobs.send(Job::Diagnose(id));
        }
        Panel {
            shared,
            jobs,
            icon,
            columns: None,
            ports: HashMap::new(),
            custom: None,
            root_edit: None,
            uninstall: None,
            newer_dismissed: false,
            screenshot,
            frames: 0,
            shot_requested: false,
        }
    }

    fn send(&self, job: Job) {
        let _ = self.jobs.send(job);
    }

    /// The icon, the name and what the hub does, how many apps are on, and the AI clients.
    fn header(&mut self, ui: &mut Ui, rows: &[&Row], client_rows: &[ClientRow], newer: Option<&Newer>, busy: bool) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 15.0;
            if let Some(icon) = &self.icon {
                ui.add(egui::Image::new((icon.id(), vec2(46.0, 46.0))));
            }
            ui.vertical(|ui| {
                // Room for the summary at the right; the description wraps in a narrow window.
                ui.set_max_width((ui.available_width() - 230.0).max(160.0));
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.label(RichText::new("Craft MCP Hub").size(TITLE_SIZE).color(IVORY));
                ui.label(RichText::new("One windowless server per app, shared by every AI client session. Closing this window stops nothing.").color(MUTED));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| summary(ui, rows));
        });
        ui.add_space(10.0);
        // The AI clients the apps are registered with. One that isn't on this computer can't be
        // turned on, but can be turned off if it was on before.
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            ui.label(caption("AI CLIENTS"));
            for client in client_rows {
                let hover = match (client.enabled, client.needs_restart) {
                    (true, _) => format!("Take the hub's servers out of {}", client.name),
                    (false, true) => format!("Register the apps that are on with {} (restart it afterwards)", client.name),
                    (false, false) => format!("Register the apps that are on with {}", client.name),
                };
                let chip = ui
                    .add_enabled_ui(!busy && (client.found || client.enabled), |ui| client_chip(ui, client))
                    .inner
                    .on_hover_text(hover)
                    .on_disabled_hover_text(if client.found {
                        "Busy: wait for what the hub is doing".to_string()
                    } else {
                        format!("{} was not found on this computer", client.name)
                    });
                if chip.clicked() {
                    self.send(Job::SetClient(client.id, !client.enabled));
                }
            }
        });
        if let Some(newer) = newer {
            ui.add_space(10.0);
            self.newer_notice(ui, newer);
        }
    }

    /// Says that a newer version is out, with a way to its release page on GitHub.
    fn newer_notice(&mut self, ui: &mut Ui, newer: &Newer) {
        egui::Frame::new()
            .fill(JADE_WASH)
            .stroke(Stroke::new(1.0_f32, JADE_LINE))
            .corner_radius(8)
            .inner_margin(Margin { left: 15, right: 7, top: 7, bottom: 7 })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.label(RichText::new(format!("Craft MCP Hub {} is available", newer.version)).color(IVORY));
                    ui.label(RichText::new(format!("you have {}", crafthub::VERSION)).size(SMALL_SIZE).color(MUTED));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(Flat::new("Later", Look::Quiet).height(28.0)).on_hover_text("Hide this until the panel is opened again").clicked() {
                            self.newer_dismissed = true;
                        }
                        if ui.add(Flat::new("Copy the update command", Look::OnWash).height(28.0)).on_hover_text(update_check::how_to_update()).clicked() {
                            ui.ctx().copy_text(update_check::how_to_update().to_string());
                        }
                        if ui.add(Flat::new("See what's new", Look::OnWash).height(28.0)).on_hover_text(&newer.url).clicked() {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(&newer.url));
                        }
                    });
                });
            });
    }

    /// The headings over the columns.
    fn headings(&self, ui: &mut Ui, columns: &Columns) {
        // Two-line rows say what each value is next to it instead.
        if columns.compact {
            return;
        }
        let (rect, _) = ui.allocate_exact_size(vec2(columns.width, 14.0), Sense::hover());
        for (x, text) in [(columns.app.0, "APP"), (columns.files.0, "FILES IT MAY USE"), (columns.port, "PORT"), (columns.state, "STATE")] {
            let mut cell = cell(ui, rect, x, 200.0, rect.height(), Layout::left_to_right(Align::Center));
            cell.label(caption(text));
        }
    }

    /// One app: its name, the files it may use, its port, what it is doing, and its buttons.
    fn app_row(&mut self, ui: &mut Ui, columns: &Columns, row: &Row, busy: bool, check: Option<(bool, String)>) {
        let (app, state) = (&row.app, &row.state);
        let height = if columns.compact { COMPACT_ROW_HEIGHT } else { ROW_HEIGHT };
        let (rect, _) = ui.allocate_exact_size(vec2(columns.width, height), Sense::hover());
        if !ui.is_rect_visible(rect) {
            return;
        }
        // On two lines, the files and the port go on the second.
        let (first, second) = match columns.compact {
            true => (rect.with_max_y(rect.top() + 50.0), rect.with_min_y(rect.top() + 46.0)),
            false => (rect, rect),
        };
        ui.painter().rect_filled(rect, CornerRadius::same(8), ROW);
        let bar = Rect::from_center_size(pos2(rect.left() + ROW_PAD + BAR_WIDTH / 2.0, rect.center().y), vec2(BAR_WIDTH, rect.height() - 28.0));
        ui.painter().rect_filled(bar, CornerRadius::same(2), Color32::from_rgb(app.color[0], app.color[1], app.color[2]));

        let top_down = Layout::top_down(Align::Min);
        let inline = Layout::left_to_right(Align::Center);
        let two_lines = |ui: &Ui, first: f32| line_height(ui, first) + 2.0 + line_height(ui, SMALL_SIZE);

        // The name may run a little into the space after its column, as long words do.
        let height = two_lines(ui, NAME_SIZE);
        let mut name = cell(ui, first, columns.app.0, columns.app.1 + GAP / 2.0, height, top_down);
        name.spacing_mut().item_spacing.y = 2.0;
        name.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.label(RichText::new(&app.name).size(NAME_SIZE).color(IVORY));
            match app.origin {
                Origin::Custom => drop(ui.label(RichText::new("custom").size(SMALL_SIZE).color(MUTED))),
                Origin::Community => drop(ui.label(RichText::new("community").size(SMALL_SIZE).color(MUTED))),
                Origin::Official => {}
            }
        });
        let tagline = if app.tagline.is_empty() { app.id.as_str() } else { app.tagline.as_str() };
        name.label(RichText::new(tagline).size(SMALL_SIZE).color(MUTED)).on_hover_text(tagline);

        let mut files = cell(ui, second, columns.files.0, columns.files.1, second.height(), inline);
        if columns.compact && app.configured && (app.profile.library || app.confinable()) {
            files.label(caption(if app.profile.library { "LIBRARY" } else { "FILES IT MAY USE" }));
        }
        self.files_value(&mut files, app, state, busy);

        if columns.compact {
            let x = columns.port - PORT_HEADING_WIDTH;
            let mut port = cell(ui, second, x, PORT_WIDTH + PORT_HEADING_WIDTH, second.height(), Layout::right_to_left(Align::Center));
            self.port_field(&mut port, app, busy);
            port.label(caption("PORT"));
        } else {
            let mut port = cell(ui, rect, columns.port, PORT_WIDTH, ROW_HEIGHT, inline);
            self.port_field(&mut port, app, busy);
        }

        let detail = status_detail(state, check);
        let height = if detail.is_some() { two_lines(ui, BODY_SIZE) } else { line_height(ui, BODY_SIZE) };
        let mut status = cell(ui, first, columns.state, STATE_WIDTH, height, top_down);
        status.spacing_mut().item_spacing.y = 2.0;
        status.horizontal(|ui| status_head(ui, state));
        if let Some((text, color, tip)) = detail {
            let label = status.label(RichText::new(text).size(SMALL_SIZE).color(color));
            if !tip.is_empty() {
                label.on_hover_text(tip);
            }
        }

        let actions_x = columns.state + STATE_WIDTH + GAP;
        let mut actions = cell(ui, first, actions_x, columns.width - actions_x - ROW_PAD, first.height(), Layout::right_to_left(Align::Center));
        self.actions(&mut actions, app, state, busy);
    }

    /// What the engine works on: the open library with its Release button, or the folders it may
    /// use with their Change button.
    fn files_value(&mut self, ui: &mut Ui, app: &App, state: &AppState, busy: bool) {
        ui.spacing_mut().item_spacing.x = 8.0;
        let none = |ui: &mut Ui| drop(ui.label(RichText::new("—").color(MUTED)));
        if !app.configured {
            return none(ui);
        }
        // The text gives way to its button, which stays at the column's right edge.
        let button_room = |ui: &Ui, text: &str| ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(12.0), TEXT).size().x + 14.0 + 8.0;
        if app.profile.library {
            match &state.library {
                Some(library) => {
                    let name = std::path::Path::new(library).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| library.clone());
                    let room = ui.available_width() - button_room(ui, "Release");
                    label_up_to(ui, room, RichText::new(format!("Library: {name}")).color(TEXT)).on_hover_text(library);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add_enabled(!busy, Flat::new("Release", Look::Plain).small()).on_hover_text("Close it, so the desktop app can open it").clicked()
                        {
                            self.send(Job::CloseLibrary(app.id.clone()));
                        }
                    });
                }
                None => drop(ui.label(RichText::new("Library: none open").color(MUTED)).on_hover_text("The AI client opens the library it needs")),
            }
        } else if let Some(roots) = app.roots() {
            let room = ui.available_width() - button_room(ui, "Change…");
            match roots.as_slice() {
                [] => drop(
                    label_up_to(ui, room, RichText::new("Any file you can open").color(AMBER))
                        .on_hover_text("Not restricted: an AI client can have this engine open or save any file you can."),
                ),
                [only] => drop(label_up_to(ui, room, RichText::new(only).color(TEXT)).on_hover_text(only)),
                [first, more @ ..] => {
                    // The count has its own place, so it stays when the first folder is cut short.
                    let count = format!("+{}", more.len());
                    let count_width = ui.painter().layout_no_wrap(count.clone(), FontId::proportional(BODY_SIZE), TEXT).size().x + 8.0;
                    label_up_to(ui, room - count_width, RichText::new(first).color(TEXT)).on_hover_text(roots.join("\n"));
                    ui.label(RichText::new(count).color(TEXT)).on_hover_text(roots.join("\n"));
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add_enabled(!busy, Flat::new("Change…", Look::Plain).small()).clicked() {
                    self.root_edit = Some(RootEdit {
                        id: app.id.clone(),
                        name: app.name.clone(),
                        folders: roots.join("\n"),
                        several: app.profile.multi_root,
                        required: app.profile.root_required,
                    });
                }
            });
        } else {
            none(ui);
        }
    }

    /// The port, in a field: typed, then taken on Enter or when the field is left.
    fn port_field(&mut self, ui: &mut Ui, app: &App, busy: bool) {
        if app.port == 0 || !app.installed() {
            ui.label(RichText::new("—").color(MUTED));
            return;
        }
        let mut text = self.ports.get(&app.id).cloned().unwrap_or_else(|| app.port.to_string());
        let field = ui.add_enabled(
            !busy,
            egui::TextEdit::singleline(&mut text)
                .font(FontId::monospace(BODY_SIZE))
                .text_color(IVORY)
                .margin(Margin::symmetric(9, 5))
                .desired_width(45.0)
                .char_limit(5),
        );
        text.retain(|c| c.is_ascii_digit());
        if field.lost_focus() {
            self.ports.remove(&app.id);
            let cancelled = ui.input(|input| input.key_pressed(egui::Key::Escape));
            match text.parse::<u16>() {
                Ok(port) if port != app.port && !cancelled => self.send(Job::SetPort(app.id.clone(), port)),
                _ => {}
            }
        } else if field.has_focus() {
            self.ports.insert(app.id.clone(), text);
        } else {
            self.ports.remove(&app.id);
        }
    }

    /// The row's buttons, added from the right edge inwards so the main one lines up in every row.
    fn actions(&mut self, ui: &mut Ui, app: &App, state: &AppState, busy: bool) {
        ui.spacing_mut().item_spacing.x = 6.0;
        // Green means turning on: solid for adding, an outline for turning on again.
        let (text, look, job) = match state.kind {
            StateKind::New => ("Add", Look::Primary, Job::Add(app.id.clone())),
            StateKind::Off | StateKind::Other | StateKind::NotInstalled => ("Turn on", Look::Outline, Job::TurnOn(app.id.clone())),
            _ => ("Turn off", Look::Plain, Job::TurnOff(app.id.clone())),
        };
        if ui.add_enabled(!busy && app.available(), Flat::new(text, look).min_width(84.0)).clicked() {
            self.send(job);
        }
        if ui.add_enabled(!busy, Flat::new("Diagnose", Look::Plain)).clicked() {
            self.send(Job::Diagnose(app.id.clone()));
        }
        let can_restart = state.is_on();
        let can_remove = app.origin == Origin::Custom && !state.is_on();
        if !(can_restart || can_remove) {
            return;
        }
        let more = ui.add_enabled(!busy, Flat::new("···", Look::Plain).padding(8.0).min_width(29.0)).on_hover_text("More");
        let menu = egui::Id::new(("more", &app.id));
        if more.clicked() {
            ui.memory_mut(|memory| memory.toggle_popup(menu));
        }
        more_menu(ui, menu, &more, |ui| {
            if state.partial.is_some() && menu_item(ui, "Register in every client", TEXT).clicked() {
                self.send(Job::TurnOn(app.id.clone()));
            }
            if can_restart && menu_item(ui, "Restart the server", TEXT).clicked() {
                self.send(Job::Restart(app.id.clone()));
            }
            if can_remove && menu_item(ui, "Remove from the hub", RED).clicked() {
                self.send(Job::Remove(app.id.clone()));
            }
        });
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
                    if ui.add_enabled(!busy, Flat::new("Check again", Look::Plain)).clicked() {
                        self.send(Job::Diagnose(id.clone()));
                    }
                    if ui.add(Flat::new("Copy", Look::Plain)).clicked() {
                        ctx.copy_text(shown.clone());
                    }
                });
            });
            if !open {
                lock(&self.shared).diagnosis = None;
            }
        }

        // Uninstalling: first what it will do, then what it did.
        let uninstalled = lock(&self.shared).uninstalled.clone();
        if let Some(report) = uninstalled {
            egui::Window::new("Craft MCP Hub was uninstalled").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(
                ctx,
                |ui| {
                    ui.set_max_width(620.0);
                    ui.label(report);
                    ui.add_space(8.0);
                    if ui.add(Flat::new("Close", Look::Confirm)).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                },
            );
            return;
        }
        let mut close_uninstall = false;
        if let Some(plan) = &self.uninstall {
            egui::Window::new("Uninstall Craft MCP Hub?").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.set_max_width(620.0);
                if plan.is_empty() {
                    ui.label("There is nothing to remove.");
                } else {
                    ui.label("This will:");
                    for step in plan {
                        ui.label(format!("•  {step}"));
                    }
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new("The ArtCraft apps themselves are not touched. Open AI client sessions lose these servers.").size(SMALL_SIZE).color(MUTED),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(!busy && !plan.is_empty(), Flat::new("Uninstall", Look::Danger)).clicked() {
                        self.send(Job::Uninstall);
                        close_uninstall = true;
                    }
                    if ui.add(Flat::new("Cancel", Look::Plain)).clicked() {
                        close_uninstall = true;
                    }
                });
            });
        }
        if close_uninstall {
            self.uninstall = None;
        }

        // What went wrong
        let error = lock(&self.shared).error.clone();
        if let Some(error) = error {
            egui::Window::new("Something went wrong").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.set_max_width(560.0);
                ui.label(&error);
                ui.add_space(8.0);
                if ui.add(Flat::new("OK", Look::Confirm)).clicked() {
                    lock(&self.shared).error = None;
                }
            });
        }

        // The folders an engine may use
        let mut close_root = false;
        if let Some(edit) = &mut self.root_edit {
            let mut open = true;
            egui::Window::new(format!("Files {} may use", edit.name)).open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
                ui.set_max_width(480.0);
                if edit.several {
                    ui.label("The folders (and everything in them) the engine may read and write, one per line:");
                    ui.add(egui::TextEdit::multiline(&mut edit.folders).desired_rows(4).desired_width(460.0));
                } else {
                    ui.label("The one folder (and everything in it) the engine may read and write:");
                    ui.add(egui::TextEdit::singleline(&mut edit.folders).desired_width(460.0));
                }
                ui.label(RichText::new("An AI client can't have this engine open or save anything outside.").size(SMALL_SIZE).color(MUTED));
                ui.add_space(6.0);
                let folders: Vec<String> = edit.folders.lines().map(|line| line.trim().trim_matches('"').to_string()).filter(|line| !line.is_empty()).collect();
                ui.horizontal(|ui| {
                    let label = if edit.several { "Use these folders" } else { "Use this folder" };
                    if ui.add_enabled(!busy && !folders.is_empty(), Flat::new(label, Look::Confirm)).clicked() {
                        let _ = self.jobs.send(Job::SetRoots(edit.id.clone(), folders.clone()));
                        close_root = true;
                    }
                    // Not offered for an engine that has no file access at all without a folder.
                    if !edit.required
                        && ui
                            .add_enabled(!busy, Flat::new("Allow any file", Look::Plain))
                            .on_hover_text("Lift the restriction: it may use any file you can")
                            .clicked()
                    {
                        let _ = self.jobs.send(Job::SetRoots(edit.id.clone(), Vec::new()));
                        close_root = true;
                    }
                });
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
                ui.label("A fork or your own build of an ArtCraft app. Its command-line tool must have an `mcp` command.");
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
                if ui.add_enabled(!busy && ready, Flat::new("Add and turn on", Look::Primary)).clicked() {
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

/// How many apps are on and how many sessions use them, in a pill at the top right.
fn summary(ui: &mut Ui, rows: &[&Row]) {
    let on = rows.iter().filter(|row| row.state.is_on()).count();
    let sessions: usize = rows.iter().filter_map(|row| row.state.status.as_ref()).map(|status| status.sessions).sum();
    let mut text = format!("{on} of {} apps on", rows.len());
    match sessions {
        0 => {}
        1 => text.push_str(" · 1 session"),
        n => text.push_str(&format!(" · {n} sessions")),
    }
    pill(ui, if on > 0 { JADE } else { MUTED }, &text);
}

/// A dot and a few words in a pill, for the summary at the top right.
fn pill(ui: &mut Ui, color: Color32, text: &str) {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(BODY_SIZE), TEXT);
    let (rect, _) = ui.allocate_exact_size(vec2(12.0 + 9.0 + 8.0 + galley.size().x + 14.0, 27.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(14), ROW);
    painter.circle_filled(pos2(rect.left() + 12.0 + 4.5, rect.center().y), 4.5, color);
    painter.galley(pos2(rect.left() + 29.0, rect.center().y - galley.size().y / 2.0), galley, TEXT);
}

/// The state, as a dot and a word in its colour.
fn status_head(ui: &mut Ui, state: &AppState) {
    ui.spacing_mut().item_spacing.x = 6.0;
    let (color, ring) = match state.kind {
        StateKind::On => (JADE, false),
        // Registered, and it starts with the next session.
        StateKind::Waiting => (JADE, true),
        StateKind::Starting | StateKind::New | StateKind::Other => (AMBER, false),
        StateKind::Error | StateKind::PortBusy => (RED, false),
        StateKind::Off | StateKind::NotInstalled => (MUTED, true),
    };
    dot(ui, color, ring);
    let text = match color {
        JADE => JADE_TEXT,
        other => other,
    };
    let label = ui.label(RichText::new(&state.label).color(text));
    if !state.tip.is_empty() {
        label.on_hover_text(&state.tip);
    }
}

/// The line under the state, if there is one: what a check found, what needs attention, or the
/// plain detail. With its colour and its tooltip.
fn status_detail(state: &AppState, check: Option<(bool, String)>) -> Option<(String, Color32, String)> {
    if let Some((ok, text)) = check.filter(|_| state.is_on()) {
        let verdict = if ok { "checked:" } else { "check failed:" };
        return Some((format!("{verdict} {text}"), if ok { JADE_TEXT } else { RED }, text));
    }
    // What needs attention comes before the plain detail.
    let (detail, color) = match (&state.partial, state.stale) {
        (Some(missing), _) => (missing.clone(), AMBER),
        (None, true) => ("restart to apply settings".to_string(), AMBER),
        (None, false) => (state.detail.clone(), MUTED),
    };
    (!detail.is_empty()).then(|| (detail, color, state.tip.clone()))
}

/// The menu under a row's ⋯, opened at the button's right edge.
fn more_menu(ui: &Ui, id: egui::Id, button: &egui::Response, add: impl FnOnce(&mut Ui)) {
    if !ui.memory(|memory| memory.is_popup_open(id)) {
        return;
    }
    let area = egui::Area::new(id)
        .kind(egui::UiKind::Popup)
        .order(egui::Order::Foreground)
        .fixed_pos(button.rect.right_bottom() + vec2(0.0, 4.0))
        .pivot(egui::Align2::RIGHT_TOP);
    let shown = area.show(ui.ctx(), |ui| {
        egui::Frame::new().fill(ROW).stroke(Stroke::new(1.0_f32, LINE)).corner_radius(8).inner_margin(Margin::same(4)).show(ui, |ui| {
            ui.set_min_width(220.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui);
        });
    });
    let clicked_inside = shown.response.contains_pointer() && ui.input(|input| input.pointer.any_click());
    if ui.input(|input| input.key_pressed(egui::Key::Escape)) || button.clicked_elsewhere() || clicked_inside {
        ui.memory_mut(|memory| memory.close_popup());
    }
}

/// A line of a menu, lit up under the pointer.
fn menu_item(ui: &mut Ui, text: &str, color: Color32) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(BODY_SIZE), color);
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 31.0), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, text));
    if response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(6), BUTTON);
    }
    ui.painter().galley(pos2(rect.left() + 12.0, rect.center().y - galley.size().y / 2.0), galley, color);
    response
}

impl eframe::App for Panel {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let (rows, client_rows, message, busy_text, checks) = {
            let mut shared = lock(&self.shared);
            shared.checks.retain(|_, check| check.at.elapsed() < Duration::from_secs(90));
            let checks: HashMap<String, (bool, String)> = shared.checks.iter().map(|(id, c)| (id.clone(), (c.ok, c.text.clone()))).collect();
            (shared.rows.clone(), shared.clients.clone(), shared.message.clone(), shared.busy.clone(), checks)
        };
        let busy = busy_text.is_some();
        let new_apps = rows.iter().filter(|r| r.state.kind == StateKind::New).count();
        let (present, missing): (Vec<&Row>, Vec<&Row>) = rows.iter().partition(|r| r.app.available() || r.app.configured);

        let newer = if self.newer_dismissed { None } else { lock(&self.shared).newer.clone() };
        let header = egui::Frame::new().fill(WINDOW).inner_margin(Margin { left: 24, right: 24, top: 20, bottom: 15 });
        egui::TopBottomPanel::top("header").frame(header).show(ctx, |ui| self.header(ui, &present, &client_rows, newer.as_ref(), busy));

        let footer = egui::Frame::new().fill(INK).inner_margin(Margin { left: 24, right: 24, top: 13, bottom: 14 });
        egui::TopBottomPanel::bottom("footer").frame(footer).show(ctx, |ui| {
            let status = busy_text.as_deref().unwrap_or(&message);
            if !status.is_empty() {
                ui.label(RichText::new(status).color(TEXT));
                ui.add_space(4.0);
            }
            ui.horizontal_top(|ui| {
                // In a narrow window the buttons go onto a second line, and Uninstall… stays at the right.
                let left = (ui.available_width() - 104.0).max(0.0);
                ui.allocate_ui_with_layout(vec2(left, 29.0), Layout::left_to_right(Align::Center).with_main_wrap(true), |ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                    fn button(text: &str, look: Look) -> Flat<'_> {
                        Flat::new(text, look).height(29.0).padding(13.0)
                    }
                    if new_apps > 0 {
                        let set_up = format!("Set up new apps ({new_apps})");
                        if ui
                            .add_enabled(!busy, button(&set_up, Look::Primary))
                            .on_hover_text("Add and turn on every installed app that isn't in the hub yet")
                            .clicked()
                        {
                            self.send(Job::SetUpNew);
                        }
                    }
                    if ui.add_enabled(!busy, button("Turn all on", Look::Plain)).clicked() {
                        self.send(Job::AllOn);
                    }
                    if ui.add_enabled(!busy, button("Turn all off", Look::Plain)).clicked() {
                        self.send(Job::AllOff);
                    }
                    if ui.add_enabled(!busy, button("Check all", Look::Plain)).on_hover_text("A full MCP round trip through every server that is on").clicked()
                    {
                        self.send(Job::CheckAll);
                    }
                    if ui.add_enabled(!busy, button("Add custom app…", Look::Plain)).clicked() {
                        self.custom = Some(CustomForm::default());
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    let uninstall = Flat::new("Uninstall…", Look::DangerOutline).height(29.0).padding(13.0);
                    if ui.add_enabled(!busy, uninstall).on_hover_text("Take Craft MCP Hub off this computer").clicked() {
                        self.uninstall = Some(uninstall::plan());
                    }
                });
            });
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Photo apps start with no library open: the AI client opens the one it needs. \
                     A library opens in one program at a time, so release it here before opening it in the desktop app.",
                )
                .size(SMALL_SIZE)
                .color(MUTED),
            );
        });

        let list = egui::Frame::new().fill(WINDOW).inner_margin(Margin { left: 16, right: 16, top: 12, bottom: 8 });
        egui::CentralPanel::default().frame(list).show(ctx, |ui| {
            if rows.is_empty() {
                ui.add_space(20.0);
                ui.label(RichText::new("Looking for the apps…").color(MUTED));
                return;
            }
            // The headings line up with the rows as they were last drawn: inside the list, whose
            // scroll bar may take some of the width.
            let columns = self.columns.unwrap_or_else(|| Columns::new(ui.available_width()));
            self.headings(ui, &columns);
            ui.add_space(1.0);
            // Both ways: in a window narrower than the table, its buttons must stay reachable.
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                let columns = Columns::new(ui.available_width());
                self.columns = Some(columns);
                ui.spacing_mut().item_spacing.y = ROW_GAP;
                for row in &present {
                    self.app_row(ui, &columns, row, busy, checks.get(&row.app.id).cloned());
                }
                if !missing.is_empty() {
                    ui.add_space(12.0);
                    egui::CollapsingHeader::new(RichText::new(format!("Not installed ({})", missing.len())).color(MUTED)).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("They show up above once installed.").size(SMALL_SIZE).color(MUTED));
                            if ui
                                .add_enabled(!busy, Flat::new("Refresh the catalog", Look::Plain).small())
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

// ---------------------------------------------------------------- pictures

/// Made-up apps for the pictures in the README: `CRAFTHUB_PANEL_DEMO=main` fills the panel with
/// them instead of this computer's. Nothing is run and nothing is saved meanwhile.
mod demo {
    use super::*;
    use crafthub::config::{AppConfig, Config};
    use crafthub::control::Status;

    /// An app as the scene has it: id, port, the folders it may use, and its state.
    struct Made {
        id: &'static str,
        port: u16,
        roots: &'static [&'static str],
        kind: StateKind,
        detail: &'static str,
        sessions: usize,
    }

    const fn made(id: &'static str, port: u16, roots: &'static [&'static str], kind: StateKind, detail: &'static str, sessions: usize) -> Made {
        Made { id, port, roots, kind, detail, sessions }
    }

    const PICTURES: &[&str] = &["C:\\Users\\Public\\Pictures"];
    const DOCUMENTS: &[&str] = &["C:\\Users\\Public\\Documents"];
    const DESKTOP: &[&str] = &["C:\\Users\\Public\\Desktop\\Designs"];

    const MAIN: &[Made] = &[
        made("photocraft", 7801, PICTURES, StateKind::On, "2 sessions", 2),
        made("vectorcraft", 7802, DOCUMENTS, StateKind::On, "no sessions", 0),
        made("lightcraft", 7803, &[], StateKind::Off, "", 0),
        made("pdfcraft", 7804, DOCUMENTS, StateKind::On, "1 session", 1),
        made("figcraft", 7805, DESKTOP, StateKind::Off, "", 0),
        made("wordcraft", 7806, DOCUMENTS, StateKind::On, "no sessions", 0),
        made("gridcraft", 7808, &[], StateKind::New, "installed, not added yet", 0),
        made("shapecraft", 7821, &[], StateKind::New, "installed, not added yet", 0),
    ];

    /// The panel as the scene shows it: `main` is the main window, `readme` the same without the
    /// update notice, whose version numbers would soon be out of date.
    pub(super) fn shared(scene: &str) -> Shared {
        // The scene's apps, the apps in the catalog its made-up computer doesn't have, and whether
        // a newer hub is out.
        let (made, absent, newer): (&[Made], &[&str], bool) = match scene {
            "readme" => (MAIN, &[], false),
            _ => (MAIN, &[], true),
        };
        // The apps' names, taglines and colours come from the catalog; the rest is made up here.
        let program = std::env::current_exe().unwrap_or_default().to_string_lossy().into_owned();
        let mut config = Config::default();
        for app in made {
            let roots = (!app.roots.is_empty()).then(|| app.roots.iter().map(|root| root.to_string()).collect());
            config.apps.insert(app.id.to_string(), AppConfig { port: app.port, cli: vec![program.clone()], roots, ..AppConfig::default() });
        }
        let hub = Hub { config, catalog: catalog::load() };
        let mut rows: Vec<Row> = made
            .iter()
            .filter_map(|made| {
                let mut app = hub.app(made.id)?;
                app.configured = made.kind != StateKind::New;
                let label = match made.kind {
                    StateKind::On => "On",
                    StateKind::Off => "Off",
                    StateKind::New => "New",
                    _ => "",
                };
                let status =
                    (made.kind == StateKind::On).then(|| Status { app: made.id.to_string(), port: made.port, sessions: made.sessions, ..Status::default() });
                let state = AppState {
                    kind: made.kind,
                    label: label.to_string(),
                    detail: made.detail.to_string(),
                    tip: String::new(),
                    library: None,
                    status,
                    stale: false,
                    partial: None,
                };
                Some(Row { app, state })
            })
            .collect();
        for id in absent {
            if let Some(mut app) = hub.app(id).or_else(|| Hub { config: Config::default(), catalog: catalog::load() }.app(id)) {
                (app.cli, app.fetchable, app.configured) = (None, false, false);
                let state = AppState {
                    kind: StateKind::NotInstalled,
                    label: "Not installed".to_string(),
                    detail: String::new(),
                    tip: String::new(),
                    library: None,
                    status: None,
                    stale: false,
                    partial: None,
                };
                rows.push(Row { app, state });
            }
        }
        let client = |id, name, found, enabled, needs_restart| ClientRow { id, name, found, enabled, needs_restart };
        Shared {
            rows,
            clients: vec![
                client("claude-code", "Claude Code", true, true, false),
                client("claude-desktop", "Claude Desktop", true, true, true),
                client("cursor", "Cursor", false, false, false),
                client("vscode", "VS Code", true, false, false),
                client("codex", "Codex", true, true, true),
            ],
            newer: newer.then(|| Newer { version: "0.7.0".to_string(), url: update_check::RELEASES_URL.to_string() }),
            ..Shared::default()
        }
    }
}

/// A PNG built into the program, as RGBA pixels with its width and height.
fn decode_png(picture: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let mut reader = png::Decoder::new(picture).read_info().ok()?;
    let mut rgba = vec![0; reader.output_buffer_size()];
    let frame = reader.next_frame(&mut rgba).ok()?;
    rgba.truncate(frame.buffer_size());
    let plain = frame.color_type == png::ColorType::Rgba && frame.bit_depth == png::BitDepth::Eight;
    plain.then_some((rgba, frame.width, frame.height))
}

/// The hub's icon, from the picture built into the program.
fn window_icon() -> Option<egui::IconData> {
    // On macOS the icon sits on the system's icon grid, with a margin, so it is the size of the
    // other apps' icons in the Dock. Elsewhere it is the rounded square Windows and Linux use (the
    // same picture as the largest size in the Windows icon file).
    #[cfg(target_os = "macos")]
    let picture = &include_bytes!("../../assets/icons/macos/craft-mcp-hub-512.png")[..];
    #[cfg(not(target_os = "macos"))]
    let picture = &include_bytes!("../../assets/icons/linux/hicolor/256x256/apps/craft-mcp-hub.png")[..];
    let (rgba, width, height) = decode_png(picture)?;
    Some(egui::IconData { rgba, width, height })
}

fn main() -> eframe::Result {
    // `CRAFTHUB_PANEL_SIZE=800x500` opens the window at that size, to take a picture of it small.
    let size = std::env::var("CRAFTHUB_PANEL_SIZE").ok().and_then(|size| {
        let (width, height) = size.split_once('x')?;
        Some([width.parse::<f32>().ok()?, height.parse::<f32>().ok()?])
    });
    // The app id (Wayland) and window class (X11) are the name of the desktop file `crafthub
    // install-app` makes on Linux, so the desktop gives the window that entry's name and icon.
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Craft MCP Hub")
        .with_app_id(crafthub::linuxapp::APP_ID)
        .with_inner_size(size.unwrap_or([1000.0, 804.0]))
        .with_min_inner_size([480.0, 320.0]);
    // The window's own icon: its title bar, the taskbar, the dock.
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(crafthub::linuxapp::APP_ID, options, Box::new(|cc| Ok(Box::new(Panel::new(cc)))))
}
