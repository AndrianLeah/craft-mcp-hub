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
use crafthub::config::Config;
use crafthub::doctor::{self, Check, Fix, Mark, Topic};
use crafthub::hub::{self, valid_id, AddOptions, App, AppState, Hub, Origin, StateKind};
use crafthub::update_check::{self, Newer};
use crafthub::{catalog, clients, profiles, uninstall};
use eframe::egui::{self, pos2, vec2, Align, Color32, CornerRadius, FontId, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

#[path = "../panel/lang.rs"]
mod lang;

/// The panel's own text in its language: the English given, translated when the language has it,
/// with the words in braces filled in from the names given after it.
macro_rules! tr {
    ($text:literal) => {
        lang::text($text)
    };
    ($text:literal, $($name:ident = $value:expr),+ $(,)?) => {
        lang::fill(&lang::text($text), &[$((stringify!($name), &$value as &dyn std::fmt::Display)),+])
    };
}

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

/// Where the ArtCraft launcher is.
const LAUNCHER_URL: &str = "https://getartcraft.com";

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
struct Flat {
    text: String,
    look: Look,
    size: f32,
    height: f32,
    min_width: f32,
    padding: f32,
}

impl Flat {
    fn new(text: impl Into<String>, look: Look) -> Flat {
        Flat { text: text.into(), look, size: BODY_SIZE, height: 30.0, min_width: 0.0, padding: 12.0 }
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

impl egui::Widget for Flat {
    fn ui(self, ui: &mut Ui) -> egui::Response {
        let galley = ui.painter().layout_no_wrap(self.text.clone(), FontId::proportional(self.size), TEXT);
        let size = vec2((galley.size().x + 2.0 * self.padding).max(self.min_width), self.height);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let enabled = ui.is_enabled();
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &self.text));
        if ui.is_rect_visible(rect) {
            let (mut fill, mut border, mut text) = self.look.colors(enabled && (response.hovered() || response.has_focus()));
            if !enabled {
                // Ink text would vanish into its dimmed fill: it takes the fill's own colour instead.
                let faded = if text == INK { fill.gamma_multiply(0.8) } else { text.gamma_multiply(0.45) };
                (fill, border, text) = (fill.gamma_multiply(0.45), border.gamma_multiply(0.45), faded);
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

/// A faint ring with a jade arc going round it: something is being done.
fn spinner(ui: &mut Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    paint_spinner(ui, rect.center(), size / 2.0 - 1.5, (size / 10.0).max(1.5));
}

fn paint_spinner(ui: &Ui, center: Pos2, radius: f32, width: f32) {
    ui.ctx().request_repaint();
    ui.painter().circle_stroke(center, radius, Stroke::new(width, LINE_HOVER));
    let start = ui.input(|input| input.time) as f32 * std::f32::consts::TAU * 0.8;
    let arc = (0..=16).map(|step| start + step as f32 / 16.0 * 1.7).map(|angle| center + radius * vec2(angle.cos(), angle.sin())).collect();
    ui.painter().add(egui::Shape::line(arc, Stroke::new(width, JADE_TEXT)));
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

/// One AI client at the top: jade with a tick when the apps are registered with it, and saying so
/// when it is no longer on this computer.
fn client_chip(ui: &mut Ui, client: &ClientRow) -> egui::Response {
    let painter = ui.painter().clone();
    let name = painter.layout_no_wrap(client.name.to_string(), FontId::proportional(BODY_SIZE), TEXT);
    let note = (!client.found).then(|| painter.layout_no_wrap(tr!("not installed"), FontId::proportional(SMALL_SIZE), CAPTION));
    let tick = if client.enabled { 16.0 } else { 0.0 };
    let width = 12.0 + tick + name.size().x + note.as_ref().map_or(0.0, |note| 8.0 + note.size().x) + 12.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, 27.0), Sense::click());
    let enabled = ui.is_enabled();
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, client.enabled, client.name));
    let hovered = enabled && response.hovered();
    let fade = |color: Color32| if enabled { color } else { color.gamma_multiply(0.6) };
    let radius = CornerRadius::same(14);
    if client.enabled {
        painter.rect(rect, radius, fade(if hovered { JADE_WASH_HOVER } else { JADE_WASH }), Stroke::new(1.0_f32, fade(JADE)), StrokeKind::Inside);
    } else {
        painter.rect_filled(rect, radius, fade(if hovered { BUTTON_HOVER } else { BUTTON }));
    }
    let mut x = rect.left() + 12.0;
    if client.enabled {
        paint_check(&painter, pos2(x + 5.0, rect.center().y), 9.0, fade(JADE_TEXT));
        x += tick;
    }
    let name_width = name.size().x;
    painter.galley_with_override_text_color(pos2(x, rect.center().y - name.size().y / 2.0), name, fade(TEXT));
    if let Some(note) = note {
        let at = pos2(x + name_width + 8.0, rect.center().y - note.size().y / 2.0);
        painter.galley_with_override_text_color(at, note, fade(MUTED));
    }
    response
}

/// The chip after the clients: an outline with a plus, lit while its list is open.
fn add_more_chip(ui: &mut Ui, label: &str, open: bool) -> egui::Response {
    let painter = ui.painter().clone();
    let plus = painter.layout_no_wrap("+".to_string(), FontId::proportional(15.0), MUTED);
    let text = painter.layout_no_wrap(label.to_string(), FontId::proportional(BODY_SIZE), TEXT);
    let width = 10.0 + plus.size().x + 5.0 + text.size().x + 12.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, 27.0), Sense::click());
    let enabled = ui.is_enabled();
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    let lit = enabled && (open || response.hovered());
    let fade = |color: Color32| if enabled { color } else { color.gamma_multiply(0.6) };
    let fill = if lit { BUTTON_HOVER } else { Color32::TRANSPARENT };
    painter.rect(rect, CornerRadius::same(14), fill, Stroke::new(1.0_f32, fade(LINE_HOVER)), StrokeKind::Inside);
    let x = rect.left() + 10.0;
    let plus_width = plus.size().x;
    painter.galley_with_override_text_color(pos2(x, rect.center().y - plus.size().y / 2.0), plus, fade(if lit { TEXT } else { MUTED }));
    painter.galley_with_override_text_color(pos2(x + plus_width + 5.0, rect.center().y - text.size().y / 2.0), text, fade(if lit { IVORY } else { TEXT }));
    response
}

/// A path with the home folder written `~`, and cut at the front, where it matters least, when it
/// is longer than `max` characters.
fn home_as_tilde(path: &std::path::Path) -> String {
    let home = crafthub::paths::home_dir();
    let text = match path.strip_prefix(&home) {
        Ok(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        Err(_) => path.display().to_string(),
    };
    const MAX: usize = 54;
    let count = text.chars().count();
    if count <= MAX {
        return text;
    }
    let tail: String = text.chars().skip(count - (MAX - 1)).collect();
    format!("…{tail}")
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
    /// For an app whose port another program has: a free one to move it to, as the hub picks them.
    free_port: Option<u16>,
}

/// What a check found: a few words for the row, and the whole answer for its tooltip.
#[derive(Clone)]
struct CheckResult {
    ok: bool,
    short: String,
    text: String,
    at: Instant,
}

/// An app's diagnosis in its window: the checks as they come in, and the full report once done.
#[derive(Clone)]
struct DiagnosisView {
    id: String,
    name: String,
    /// Whether the app is on: one that is off is expected to be in no client.
    on: bool,
    /// The checks to come, by topic and name, shown as waiting.
    planned: Vec<(Topic, String)>,
    checks: Vec<Check>,
    /// `None` while the checks run.
    report: Option<String>,
}

/// What went wrong, for its window.
#[derive(Clone)]
struct Failure {
    /// "PhotoCraft couldn't turn on".
    title: String,
    /// Why, and what is free when the fix is another port.
    text: String,
    /// The app it is about, for Diagnose.
    app: Option<String>,
    /// The fix, as the window's main button: its words, and the port it moves the app to.
    fix: Option<(String, u16)>,
}

impl Failure {
    /// For an app that couldn't turn on because something has its port: says so, and offers a
    /// free one.
    fn offer_free_port(&mut self, id: &str) {
        let config = Config::load();
        let Some(app) = Hub::load().app(id) else { return };
        if let Some(conflict) = hub::port_conflict(&config, id, app.port).filter(|conflict| conflict.contains("already")) {
            let free = hub::free_port(&config, None);
            let why = lang::any(conflict.trim_end_matches('.'));
            self.text = tr!("{why}, so its server can't start there. Port {free} is free.", why = why, free = free);
            self.fix = Some((tr!("Use port {port} and turn on", port = free), free));
        }
    }
}

/// Uninstalling, as its window shows it.
#[derive(Clone)]
struct Uninstalling {
    /// How many apps it turns off.
    apps: usize,
    /// The parts it has to do, and the one under way.
    stages: Vec<uninstall::Stage>,
    stage: Option<uninstall::Stage>,
    /// What was done, step by step, once it is over.
    done: Vec<(uninstall::Stage, String, Result<(), String>)>,
    /// What is left to do by hand.
    left_over: Option<String>,
    finished: bool,
}

/// An AI client the hub can register apps with, for the chips at the top.
#[derive(Clone)]
struct ClientRow {
    id: &'static str,
    name: &'static str,
    found: bool,
    enabled: bool,
    needs_restart: bool,
    /// Its settings file, with the home folder as `~`: where turning it on writes.
    file: String,
}

/// How the status line starts: a tick for what is done, a dot for a problem, or for news that is
/// neither.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tone {
    Done,
    Problem,
    Plain,
}

/// Where the status line's link leads.
#[derive(Clone)]
enum Link {
    /// An app's diagnosis.
    Diagnose(String),
    /// Back to the window that said what went wrong.
    Error,
}

/// What the last action did, in the status line until the next one: a sentence for what happened,
/// one for what to do next, and a link when there is a problem.
#[derive(Clone)]
struct Note {
    tone: Tone,
    text: String,
    next: String,
    link: Option<(String, Link)>,
}

impl Note {
    fn done(text: impl Into<String>, next: impl Into<String>) -> Note {
        Note { tone: Tone::Done, text: text.into(), next: next.into(), link: None }
    }

    fn plain(text: impl Into<String>, next: impl Into<String>) -> Note {
        Note { tone: Tone::Plain, ..Note::done(text, next) }
    }

    fn problem(text: impl Into<String>, link: &str, to: Link) -> Note {
        Note { tone: Tone::Problem, text: text.into(), next: String::new(), link: Some((link.to_string(), to)) }
    }
}

/// What the worker is doing.
#[derive(Clone)]
struct Busy {
    /// For the status line: "Turning on PhotoCraft…".
    text: String,
    /// For the app's row, next to a spinner: "Turning on…".
    doing: String,
    /// The app it works on, whose row waits meanwhile.
    app: Option<String>,
    /// How far along an action on several apps is: this one of so many.
    step: Option<(usize, usize)>,
}

#[derive(Default)]
struct Shared {
    rows: Vec<Row>,
    /// Whether the apps have been looked for: until then the panel says it is looking.
    loaded: bool,
    clients: Vec<ClientRow>,
    /// What the last action did.
    note: Option<Note>,
    /// What the worker is doing right now. The row it works on waits, and so do the buttons that
    /// act on every app.
    busy: Option<Busy>,
    checks: HashMap<String, CheckResult>,
    diagnosis: Option<DiagnosisView>,
    /// What went wrong, for its window. The last such stays, for the status line's link back to it.
    failure: Option<Failure>,
    last_failure: Option<Failure>,
    /// Uninstalling, once it has begun: the panel then only says how far along it is, and closes.
    uninstalling: Option<Uninstalling>,
    /// A newer Craft MCP Hub on GitHub, once the check has found one.
    newer: Option<Newer>,
}

enum Job {
    Add(String),
    AddCustom {
        id: String,
        options: AddOptions,
    },
    Remove(String),
    TurnOn(String),
    TurnOff(String),
    Restart(String),
    SetPort(String, u16),
    SetRoots(String, Vec<String>),
    CloseLibrary(String),
    Diagnose(String),
    /// Moves an app to a free port, then turns it on.
    MoveAndTurnOn(String, u16),
    /// Keeps the panel's language, by its code, in the hub's settings.
    SetLanguage(String),
    SetUpNew,
    AllOn,
    AllOff,
    CheckAll,
    UpdateCatalog,
    /// Looks for the apps again, with the launcher's newest list of them.
    LookAgain,
    SetClient(&'static str, bool),
    Uninstall,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Names in a sentence: "Codex", "Codex and Cursor", "Codex, Cursor and VS Code".
fn and_list(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => tr!("{rest} and {last}", rest = rest.join(", "), last = last),
    }
}

/// "1 app", "2 apps".
fn apps(n: usize) -> String {
    if n == 1 {
        tr!("1 app")
    } else {
        tr!("{n} apps", n = n)
    }
}

/// A failed check in a few words, for the row. The whole answer goes in its tooltip.
fn short_failure(why: &str, took: Duration) -> String {
    if why.contains("no answer") || why.contains("does not answer") {
        tr!("no answer in {seconds} s", seconds = took.as_secs().max(1))
    } else if why.contains("no server is running") {
        tr!("not running")
    } else if why.contains("refused") {
        tr!("refused the hub's token")
    } else {
        why.to_string()
    }
}

// ---------------------------------------------------------------- worker

fn refresh(shared: &Mutex<Shared>) {
    let hub = Hub::load();
    let registered = Snapshot::take(&hub.config);
    // Clients rewrite their settings often: when none can be read this instant, keep what we have.
    if registered.unreadable() {
        return;
    }
    let rows: Vec<Row> = hub
        .apps()
        .into_iter()
        .map(|app| {
            let state = hub::state(&app, &registered);
            let free_port = (state.kind == StateKind::PortBusy).then(|| hub::free_port(&hub.config, None));
            Row { app, state, free_port }
        })
        .collect();
    let enabled: Vec<&str> = registered.clients().map(|client| client.id).collect();
    let client_rows: Vec<ClientRow> = clients::all()
        .iter()
        .map(|client| ClientRow {
            id: client.id,
            name: client.name,
            found: client.detected(),
            enabled: enabled.contains(&client.id),
            needs_restart: client.needs_restart,
            file: home_as_tilde(&client.settings_file()),
        })
        .collect();
    let mut shared = lock(shared);
    shared.rows = rows;
    shared.clients = client_rows;
    shared.loaded = true;
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
    let name = |id: &str| lock(shared).rows.iter().find(|r| r.app.id == id).map_or(id.to_string(), |r| r.app.name.clone());
    // Says what is going on: in the status line, and next to a spinner in the app's row.
    let busy = |app: Option<&str>, text: String, doing: &str, step: Option<(usize, usize)>| {
        lock(shared).busy = Some(Busy { text, doing: doing.to_string(), app: app.map(str::to_string), step });
        ctx.request_repaint();
    };
    // Runs one action on several apps, one after the other, saying what it does with each (and in
    // its row); returns how many, or what went wrong, app by app.
    let on_each = |ids: &[String], doing: &str, says: &dyn Fn(&str) -> String, action: &dyn Fn(&str) -> Result<(), String>| {
        let mut errors = Vec::new();
        for (index, id) in ids.iter().enumerate() {
            busy(Some(id), says(&name(id)), doing, (ids.len() > 1).then_some((index + 1, ids.len())));
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
    let use_it = tr!("Open a new AI session to use it.");
    let use_them = |n: usize| if n == 1 { tr!("Open a new AI session to use it.") } else { tr!("Open a new AI session to use them.") };
    let turning_on = |name: &str| tr!("Turning on {name}…", name = name);
    let turning_off = |name: &str| tr!("Turning off {name}…", name = name);

    if let Job::Uninstall = job {
        busy(None, tr!("Uninstalling Craft MCP Hub…"), "", None);
        let done = uninstall::run_with(|stage| {
            if let Some(uninstalling) = lock(shared).uninstalling.as_mut() {
                uninstalling.stage = Some(stage);
            }
            ctx.request_repaint();
        });
        let left_over = uninstall::left_over();
        let mut state = lock(shared);
        state.busy = None;
        if let Some(uninstalling) = state.uninstalling.as_mut() {
            (uninstalling.done, uninstalling.left_over, uninstalling.stage, uninstalling.finished) = (done, left_over, None, true);
        }
        drop(state);
        ctx.request_repaint();
        return;
    }

    // The app the job is about, and whether it turns it on: for the window that says what went wrong.
    let (about, turns_on) = match &job {
        Job::Add(id) | Job::TurnOn(id) | Job::MoveAndTurnOn(id, _) => (Some(id.clone()), true),
        Job::Remove(id) | Job::TurnOff(id) | Job::Restart(id) | Job::SetPort(id, _) | Job::SetRoots(id, _) | Job::CloseLibrary(id) => (Some(id.clone()), false),
        _ => (None, false),
    };

    // What happened, for the status line; or why it failed, with the sentence that says what did.
    let (outcome, failed): (Result<Option<Note>, String>, String) = match job {
        Job::Add(id) | Job::TurnOn(id) => {
            let name = name(&id);
            busy(Some(&id), turning_on(&name), &tr!("Turning on…"), None);
            (hub::turn_on(&id).map(|()| Some(Note::done(tr!("{name} is on.", name = name), use_it))), tr!("{name} couldn't turn on.", name = name))
        }
        Job::AddCustom { id, options } => {
            busy(None, tr!("Adding {name}…", name = id), "", None);
            let added = hub::add(&id, options).and_then(|app| hub::turn_on(&app.id).map(|()| app));
            let note = |app: App| Note::done(tr!("{name} is on, on port {port}.", name = app.name, port = app.port), use_it);
            (added.map(|app| Some(note(app))), tr!("{name} couldn't be added.", name = id))
        }
        Job::Remove(id) => {
            let name = name(&id);
            busy(Some(&id), tr!("Removing {name}…", name = name), &tr!("Removing…"), None);
            (hub::remove(&id).map(|()| Some(Note::done(tr!("{name} is out of the hub.", name = name), ""))), tr!("{name} couldn't be removed.", name = name))
        }
        Job::TurnOff(id) => {
            let name = name(&id);
            busy(Some(&id), turning_off(&name), &tr!("Turning off…"), None);
            (hub::turn_off(&id).map(|()| Some(Note::done(tr!("{name} is off.", name = name), ""))), tr!("{name} couldn't turn off.", name = name))
        }
        Job::Restart(id) => {
            let name = name(&id);
            busy(Some(&id), tr!("Restarting {name}…", name = name), &tr!("Restarting…"), None);
            (hub::restart(&id).map(|()| Some(Note::done(tr!("{name} restarted.", name = name), ""))), tr!("{name} couldn't restart.", name = name))
        }
        Job::SetPort(id, port) => {
            let name = name(&id);
            busy(Some(&id), tr!("Moving {name} to port {port}…", name = name, port = port), &tr!("Moving…"), None);
            let note = Note::done(tr!("{name} is on port {port} now.", name = name, port = port), tr!("Open sessions follow it by themselves."));
            (hub::set_port(&id, port).map(|()| Some(note)), tr!("{name} couldn't move to port {port}.", name = name, port = port))
        }
        Job::SetRoots(id, dirs) => {
            let name = name(&id);
            busy(Some(&id), tr!("Changing the files {name} may use…", name = name), &tr!("Changing…"), None);
            let text = match dirs.as_slice() {
                [] => tr!("{name} may use any file you can open.", name = name),
                dirs => tr!("{name} may use only {folders}.", name = name, folders = and_list(&dirs.iter().map(String::as_str).collect::<Vec<_>>())),
            };
            (hub::set_roots(&id, &dirs).map(|()| Some(Note::done(text, ""))), tr!("The files {name} may use couldn't be changed.", name = name))
        }
        Job::CloseLibrary(id) => {
            let name = name(&id);
            busy(Some(&id), tr!("Releasing the library of {name}…", name = name), &tr!("Releasing…"), None);
            let note = Note::done(tr!("{name} released its library.", name = name), tr!("The desktop app can open it now."));
            (hub::close_library(&id).map(|()| Some(note)), tr!("{name} couldn't release its library.", name = name))
        }
        Job::Diagnose(id) => {
            // The window was closed before its turn came: nothing to check for.
            if lock(shared).diagnosis.as_ref().is_none_or(|view| view.id != id) {
                return;
            }
            let name = name(&id);
            // Opened before the apps were loaded, the window had only the app's id.
            let on = lock(shared).rows.iter().any(|row| row.app.id == id && row.state.is_on());
            if let Some(view) = lock(shared).diagnosis.as_mut() {
                (view.name, view.on) = (name.clone(), on);
            }
            busy(Some(&id), tr!("Checking {name}…", name = name), &tr!("Checking…"), None);
            let found = doctor::diagnose(&id, |checks| {
                if let Some(view) = lock(shared).diagnosis.as_mut().filter(|view| view.id == id) {
                    view.checks = checks.to_vec();
                }
                ctx.request_repaint();
            });
            if let Some(view) = lock(shared).diagnosis.as_mut().filter(|view| view.id == id) {
                (view.checks, view.report) = (found.checks, Some(found.report));
            }
            (Ok(None), String::new())
        }
        Job::MoveAndTurnOn(id, port) => {
            let name = name(&id);
            busy(Some(&id), tr!("Moving {name} to port {port}…", name = name, port = port), &tr!("Moving…"), None);
            let moved = hub::set_port(&id, port).and_then(|()| {
                busy(Some(&id), turning_on(&name), &tr!("Turning on…"), None);
                hub::turn_on(&id)
            });
            let note = Note::done(tr!("{name} is on, on port {port}.", name = name, port = port), use_it);
            (moved.map(|()| Some(note)), tr!("{name} couldn't turn on.", name = name))
        }
        Job::SetUpNew => {
            let ids = ids_where(&|r| r.state.kind == StateKind::New);
            let note = |n: usize| Note::done(if n == 1 { tr!("1 new app is on.") } else { tr!("{n} new apps are on.", n = n) }, use_them(n));
            let done = on_each(&ids, &tr!("Turning on…"), &turning_on, &|id| hub::turn_on(id));
            (done.map(|n| Some(note(n))), tr!("Not every new app could be set up."))
        }
        Job::AllOn => {
            let ids = ids_where(&|r| r.app.configured && r.app.installed());
            let note = |n: usize| match n {
                0 => Note::plain(tr!("There is no app to turn on yet."), tr!("Add one first.")),
                1 => Note::done(tr!("1 app is on."), use_them(1)),
                n => Note::done(tr!("{n} apps are on.", n = n), use_them(n)),
            };
            let done = on_each(&ids, &tr!("Turning on…"), &turning_on, &|id| hub::turn_on(id));
            (done.map(|n| Some(note(n))), tr!("Not every app could turn on."))
        }
        Job::AllOff => {
            let ids = ids_where(&|r| r.state.is_on());
            let note = |n: usize| match n {
                0 => Note::plain(tr!("No app was on."), ""),
                1 => Note::done(tr!("1 app is off."), ""),
                n => Note::done(tr!("{n} apps are off.", n = n), ""),
            };
            let done = on_each(&ids, &tr!("Turning off…"), &turning_off, &|id| hub::turn_off(id));
            (done.map(|n| Some(note(n))), tr!("Not every app could turn off."))
        }
        Job::CheckAll => {
            let ids = ids_where(&|r| r.state.is_on());
            let mut failing = Vec::new();
            for (index, id) in ids.iter().enumerate() {
                busy(Some(id), tr!("Checking {name}…", name = name(id)), &tr!("Checking…"), (ids.len() > 1).then_some((index + 1, ids.len())));
                let started = Instant::now();
                let result = match hub::check(id) {
                    Ok(ok) => {
                        let short = tr!("{tools} tools, {ms} ms", tools = ok.tools, ms = ok.millis);
                        let text = tr!("{tools} tools in {ms} ms", tools = ok.tools, ms = ok.millis);
                        CheckResult { ok: true, short, text, at: Instant::now() }
                    }
                    Err(why) => {
                        failing.push(id.clone());
                        CheckResult { ok: false, short: short_failure(&why, started.elapsed()), text: why, at: Instant::now() }
                    }
                };
                lock(shared).checks.insert(id.clone(), result);
            }
            let see = tr!("See what's wrong");
            let note = match (ids.len(), failing.as_slice()) {
                (0, _) => Note::plain(tr!("No app is on."), tr!("Turn one on first.")),
                (1, []) => Note::done(tr!("The app that is on answers."), ""),
                (n, []) => Note::done(tr!("All {n} apps that are on answer.", n = n), ""),
                (_, [one]) => Note::problem(tr!("{name} doesn't answer.", name = name(one)), &see, Link::Diagnose(one.clone())),
                (_, [first, ..]) => Note::problem(tr!("{n} apps don't answer.", n = failing.len()), &see, Link::Diagnose(first.clone())),
            };
            (Ok(Some(note)), String::new())
        }
        Job::UpdateCatalog => {
            busy(None, tr!("Fetching the list of apps…"), "", None);
            let note = |n: usize| Note::done(tr!("The list of apps is up to date."), tr!("It has {n} apps.", n = n));
            (catalog::update().map(|n| Some(note(n))), tr!("The list of apps couldn't be fetched."))
        }
        Job::LookAgain => {
            busy(None, tr!("Looking for the apps…"), "", None);
            // The launcher's newest list first, in case an app joined it; offline, the one there is.
            let _ = catalog::update();
            refresh(shared);
            let found = lock(shared).rows.iter().filter(|r| r.app.available() || r.app.configured).count();
            let note = match found {
                0 => Note::plain(tr!("No ArtCraft app found yet."), tr!("Install one with the ArtCraft launcher, then look again.")),
                n => Note::done(tr!("Found {apps}.", apps = apps(n)), ""),
            };
            (Ok(Some(note)), String::new())
        }
        Job::SetLanguage(code) => {
            let mut config = Config::load();
            config.language = Some(code);
            (config.save().map(|()| None), tr!("The language couldn't be kept for next time."))
        }
        Job::Uninstall => unreachable!("handled above"),
        Job::SetClient(id, on) => {
            let client = clients::get(id);
            let client_name = client.map_or(id, |c| c.name);
            if on {
                busy(None, tr!("Adding your apps to {client}…", client = client_name), "", None);
                let restart = client.is_some_and(|c| c.needs_restart);
                let note = |apps: Vec<String>| match (apps.len(), restart) {
                    (0, _) => Note::done(tr!("{client} is on.", client = client_name), tr!("Turn an app on to add it there.")),
                    (1, true) => Note::done(tr!("{client} has your app now.", client = client_name), tr!("Restart {client} to load it.", client = client_name)),
                    (1, false) => Note::done(
                        tr!("{client} has your app now.", client = client_name),
                        tr!("Open a new session in {client} to use it.", client = client_name),
                    ),
                    (n, true) => Note::done(
                        tr!("{client} has your {n} apps now.", client = client_name, n = n),
                        tr!("Restart {client} to load them.", client = client_name),
                    ),
                    (n, false) => Note::done(
                        tr!("{client} has your {n} apps now.", client = client_name, n = n),
                        tr!("Open a new session in {client} to use them.", client = client_name),
                    ),
                };
                (hub::enable_client(id).map(|apps| Some(note(apps))), tr!("Your apps couldn't be added to {client}.", client = client_name))
            } else {
                busy(None, tr!("Taking your apps out of {client}…", client = client_name), "", None);
                let note = Note::done(tr!("{client} no longer has your apps.", client = client_name), "");
                (hub::disable_client(id).map(|()| Some(note)), tr!("Your apps couldn't be taken out of {client}.", client = client_name))
            }
        }
    };

    refresh(shared);
    // What went wrong, with the fix when it is a port something else has.
    let failure = outcome.as_ref().err().map(|why| {
        let mut failure = Failure { title: failed.trim_end_matches('.').to_string(), text: why.clone(), app: about.clone(), fix: None };
        if let (true, Some(id)) = (turns_on, &about) {
            failure.offer_free_port(id);
        }
        failure
    });
    let mut state = lock(shared);
    state.busy = None;
    match (outcome, failure) {
        (Ok(Some(note)), _) => state.note = Some(note),
        (Err(_), Some(failure)) => {
            state.note = Some(Note::problem(failed, &tr!("See why"), Link::Error));
            state.last_failure = Some(failure.clone());
            state.failure = Some(failure);
        }
        _ => {}
    }
    drop(state);
    ctx.request_repaint();
}

// ---------------------------------------------------------------- window

/// The custom-app window: the tool, and the names filled in from it.
#[derive(Default)]
struct CustomForm {
    cli: String,
    id: String,
    name: String,
    like: String,
    settings: String,
    /// More options are unfolded.
    more: bool,
    /// Whether the tool has an `mcp` command; `None` while that is being found out.
    recognized: Arc<Mutex<Option<bool>>>,
}

impl CustomForm {
    /// Takes the tool at `path`, names the app after it, and finds out in the background whether
    /// the hub can run it.
    fn choose(&mut self, path: &std::path::Path) {
        self.cli = path.to_string_lossy().into_owned();
        let stem = path.file_stem().map(|stem| stem.to_string_lossy().to_lowercase()).unwrap_or_default();
        self.id = stem.trim_end_matches("-cli").replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "-");
        self.name = self.id.split('-').filter(|word| !word.is_empty()).map(sentence).collect::<Vec<_>>().join(" ");
        let recognized = Arc::new(Mutex::new(None));
        self.recognized = recognized.clone();
        let cli = vec![self.cli.clone()];
        thread::spawn(move || *lock(&recognized) = Some(profiles::has_mcp(&cli)));
    }

    /// Asks for the tool with the system's own file dialog.
    fn pick(&mut self) {
        if let Some(file) = rfd::FileDialog::new().set_title(tr!("The app's command-line tool")).pick_file() {
            self.choose(&file);
        }
    }
}

/// The window for choosing the files an app's engine may use.
struct RootEdit {
    id: String,
    name: String,
    folders: Vec<String>,
    /// Any file, rather than only the folders.
    any: bool,
    /// Whether the engine takes more than one folder.
    several: bool,
    /// Whether the engine has no file access at all without a folder.
    required: bool,
    /// Whether the app is on, so it restarts to apply the change.
    running: bool,
}

impl RootEdit {
    /// Adds a folder, or puts it in place of the one folder an engine can have.
    fn add(&mut self, folder: String) {
        if !self.several {
            self.folders.clear();
        }
        if !self.folders.contains(&folder) {
            self.folders.push(folder);
        }
        self.any = false;
    }
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
    /// The uninstall confirmation: what uninstalling would do, in short and in full.
    uninstall: Option<(uninstall::Summary, Vec<String>)>,
    /// The diagnosis shows its full report.
    show_report: bool,
    /// The languages there are, as the language menu last found them.
    languages: Vec<lang::Language>,
    /// The new-version notice was put away until the panel is next opened.
    newer_dismissed: bool,
    /// The first run: apps found, none added yet. One button then sets them all up, and the rows'
    /// Add buttons step back to an outline.
    first_run: bool,
    /// The apps that aren't installed are unfolded, and all of them are listed.
    absent_open: bool,
    absent_all: bool,
    /// The panel shows made-up apps, for pictures.
    demo: bool,
    /// For taking a picture of the panel: where to save it, and how far along we are.
    screenshot: Option<PathBuf>,
    frames: u32,
    shot_requested: bool,
}

impl Panel {
    fn new(cc: &eframe::CreationContext<'_>) -> Panel {
        lang::choose(&lang::initial());
        ink_and_jade(&cc.egui_ctx);
        let icon = decode_png(include_bytes!("../../assets/icons/linux/hicolor/96x96/apps/craft-mcp-hub.png")).map(|(rgba, width, height)| {
            let image = egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba);
            let smooth = egui::TextureOptions { mipmap_mode: Some(egui::TextureFilter::Linear), ..egui::TextureOptions::LINEAR };
            cc.egui_ctx.load_texture("icon", image, smooth)
        });
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (jobs, queue) = mpsc::channel();
        // Made-up apps for the pictures: nothing runs and nothing is saved.
        let scene = std::env::var("CRAFTHUB_PANEL_DEMO").ok().filter(|scene| !scene.is_empty());
        if let Some(scene) = &scene {
            *lock(&shared) = demo::shared(scene);
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
        let diagnose = std::env::var("CRAFTHUB_PANEL_DIAGNOSE").ok().filter(|_| screenshot.is_some());
        let mut panel = Panel {
            shared,
            jobs,
            icon,
            columns: None,
            ports: HashMap::new(),
            custom: None,
            root_edit: None,
            uninstall: None,
            show_report: false,
            languages: lang::available(),
            newer_dismissed: false,
            first_run: false,
            absent_open: false,
            absent_all: false,
            demo: scene.is_some(),
            screenshot,
            frames: 0,
            shot_requested: false,
        };
        if let Some(scene) = &scene {
            demo::arrange(&mut panel, &cc.egui_ctx, scene);
        }
        if let Some(id) = diagnose {
            panel.diagnose(&id);
        }
        panel
    }

    fn send(&self, job: Job) {
        let _ = self.jobs.send(job);
    }

    /// The icon, the name and what the hub does, how many apps are on, and the AI clients. Under
    /// them, on the first run, the button that sets every app up, and the update notice.
    #[allow(clippy::too_many_arguments)]
    fn header(&mut self, ui: &mut Ui, rows: &[&Row], loaded: bool, new_apps: usize, client_rows: &[ClientRow], newer: Option<&Newer>, busy: bool) {
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
                ui.label(
                    RichText::new(tr!("One windowless server per app, shared by every AI client session. Closing this window stops nothing.")).color(MUTED),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| match (loaded, rows.is_empty()) {
                (false, _) => pill(ui, MUTED, &tr!("Looking for apps…")),
                (true, true) => pill(ui, MUTED, &tr!("No apps yet")),
                (true, false) => summary(ui, rows),
            });
        });
        ui.add_space(10.0);
        // The AI clients: the ones on this computer, and the ones turned on, as chips that turn them
        // on and off; the others the hub knows in the list under + Add more.
        let hover = |client: &ClientRow| match (client.enabled, client.needs_restart) {
            (true, _) => tr!("Take the hub's servers out of {client}", client = client.name),
            (false, true) => tr!("Register the apps that are on with {client} (restart it afterwards)", client = client.name),
            (false, false) => tr!("Register the apps that are on with {client}", client = client.name),
        };
        let (shown, others): (Vec<&ClientRow>, Vec<&ClientRow>) = client_rows.iter().partition(|client| client.found || client.enabled);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            ui.label(caption(&tr!("AI CLIENTS")));
            if loaded && shown.is_empty() {
                ui.label(RichText::new(tr!("none found on this computer")).color(CAPTION));
            }
            for client in &shown {
                let chip = ui
                    .add_enabled_ui(!busy, |ui| client_chip(ui, client))
                    .inner
                    .on_hover_text(hover(client))
                    .on_disabled_hover_text(tr!("Busy: wait for what the hub is doing"));
                if chip.clicked() {
                    self.send(Job::SetClient(client.id, !client.enabled));
                }
            }
            let id = egui::Id::new("more clients");
            let open = ui.memory(|memory| memory.is_popup_open(id));
            let label = if shown.is_empty() { tr!("Add a client") } else { tr!("Add more") };
            let more = ui
                .add_enabled_ui(!busy, |ui| add_more_chip(ui, &label, open))
                .inner
                .on_hover_text(tr!("The other AI clients the hub knows, and the way to add one it doesn't"))
                .on_disabled_hover_text(tr!("Busy: wait for what the hub is doing"));
            if more.clicked() {
                ui.memory_mut(|memory| memory.toggle_popup(id));
            }
            popup(ui, id, &more, more.rect.left_bottom() + vec2(0.0, 6.0), egui::Align2::LEFT_TOP, 340.0, |ui| {
                if !others.is_empty() {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.label(caption(&tr!("NOT FOUND ON THIS COMPUTER")));
                    });
                    ui.add_space(4.0);
                    // Turning one on writes its settings file, so the note says which.
                    for client in &others {
                        if menu_item(ui, client.name, TEXT, Some(&client.file), true).on_hover_text(hover(client)).clicked() {
                            self.send(Job::SetClient(client.id, true));
                        }
                    }
                    ui.separator();
                }
                let note = tr!("copies what to ask your AI, so it sets itself up");
                if menu_item(ui, &tr!("Another client…"), TEXT, Some(&note), true).clicked() {
                    ui.ctx().copy_text(tr!("Run `crafthub guide` and set Craft MCP Hub up for yourself."));
                    lock(&self.shared).note = Some(Note::done(
                        tr!("Copied what to ask your AI."),
                        tr!("Paste it into a session of the AI client you want: it reads the hub's guide and sets itself up."),
                    ));
                }
            });
        });
        if self.first_run {
            ui.add_space(10.0);
            self.first_run_card(ui, new_apps, busy);
        }
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
                    ui.label(RichText::new(tr!("Craft MCP Hub {version} is available", version = newer.version)).color(IVORY));
                    ui.label(RichText::new(tr!("you have {version}", version = crafthub::VERSION)).size(SMALL_SIZE).color(MUTED));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(Flat::new(tr!("Later"), Look::Quiet).height(28.0)).on_hover_text(tr!("Hide this until the panel is opened again")).clicked() {
                            self.newer_dismissed = true;
                        }
                        if ui.add(Flat::new(tr!("Copy the update command"), Look::OnWash).height(28.0)).on_hover_text(update_check::how_to_update()).clicked() {
                            ui.ctx().copy_text(update_check::how_to_update().to_string());
                            lock(&self.shared).note = Some(Note::done(tr!("Update command copied."), tr!("Paste it into a terminal, then come back here.")));
                        }
                        if ui.add(Flat::new(tr!("See what's new"), Look::OnWash).height(28.0)).on_hover_text(&newer.url).clicked() {
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
        for (x, text) in [(columns.app.0, tr!("APP")), (columns.files.0, tr!("FILES IT MAY USE")), (columns.port, tr!("PORT")), (columns.state, tr!("STATE"))] {
            let mut cell = cell(ui, rect, x, 200.0, rect.height(), Layout::left_to_right(Align::Center));
            cell.label(caption(&text));
        }
    }

    /// One app: its name, the files it may use, its port, what it is doing, and its buttons.
    fn app_row(&mut self, ui: &mut Ui, columns: &Columns, row: &Row, busy: Option<&Busy>, check: Option<&CheckResult>) {
        let (app, state) = (&row.app, &row.state);
        let height = if columns.compact { COMPACT_ROW_HEIGHT } else { ROW_HEIGHT };
        let (rect, _) = ui.allocate_exact_size(vec2(columns.width, height), Sense::hover());
        if !ui.is_rect_visible(rect) {
            return;
        }
        // While the worker is on this app, its row shows a spinner and its buttons wait.
        let working = busy.filter(|busy| busy.app.as_deref() == Some(app.id.as_str()));
        let wait = working.is_some();
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
                Origin::Custom => drop(ui.label(RichText::new(tr!("custom")).size(SMALL_SIZE).color(MUTED))),
                Origin::Community => drop(ui.label(RichText::new(tr!("community")).size(SMALL_SIZE).color(MUTED))),
                Origin::Official => {}
            }
        });
        let tagline = if app.tagline.is_empty() { app.id.as_str() } else { app.tagline.as_str() };
        name.label(RichText::new(tagline).size(SMALL_SIZE).color(MUTED)).on_hover_text(tagline);

        let mut files = cell(ui, second, columns.files.0, columns.files.1, second.height(), inline);
        if columns.compact && app.configured && (app.profile.library || app.confinable()) {
            files.label(caption(&if app.profile.library { tr!("LIBRARY") } else { tr!("FILES IT MAY USE") }));
        }
        self.files_value(&mut files, app, state, wait);

        if columns.compact {
            let x = columns.port - PORT_HEADING_WIDTH;
            let mut port = cell(ui, second, x, PORT_WIDTH + PORT_HEADING_WIDTH, second.height(), Layout::right_to_left(Align::Center));
            self.port_field(&mut port, app, wait);
            port.label(caption(&tr!("PORT")));
        } else {
            let mut port = cell(ui, rect, columns.port, PORT_WIDTH, ROW_HEIGHT, inline);
            self.port_field(&mut port, app, wait);
        }

        if let Some(busy) = working {
            let mut status = cell(ui, first, columns.state, STATE_WIDTH, line_height(ui, BODY_SIZE), inline);
            status.spacing_mut().item_spacing.x = 8.0;
            spinner(&mut status, 12.0);
            status.label(RichText::new(&busy.doing).color(TEXT));
        } else {
            let detail = row_detail(row, check);
            let height = if detail.is_some() { two_lines(ui, BODY_SIZE) } else { line_height(ui, BODY_SIZE) };
            let mut status = cell(ui, first, columns.state, STATE_WIDTH, height, top_down);
            status.spacing_mut().item_spacing.y = 2.0;
            status.horizontal(|ui| status_head(ui, state));
            match detail {
                Some(Detail::Text(text, tip)) => {
                    let label = status.label(RichText::new(text).size(SMALL_SIZE).color(MUTED));
                    if !tip.is_empty() {
                        label.on_hover_text(tip);
                    }
                }
                Some(Detail::Check { ok, short, text }) => {
                    status.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;
                        if ok {
                            let (rect, _) = ui.allocate_exact_size(vec2(9.0, 12.0), Sense::hover());
                            paint_check(ui.painter(), rect.center(), 8.0, JADE_TEXT);
                        } else {
                            ui.label(RichText::new("×").size(SMALL_SIZE).color(RED));
                        }
                        ui.label(RichText::new(short).size(SMALL_SIZE).color(if ok { JADE_TEXT } else { RED })).on_hover_text(text);
                    });
                }
                Some(Detail::Fix { label, job, tip }) => {
                    let link = link(&mut status, &label, SMALL_SIZE);
                    let link = if tip.is_empty() { link } else { link.on_hover_text(tip) };
                    if link.clicked() {
                        self.send(job);
                    }
                }
                None => {}
            }
        }

        let actions_x = columns.state + STATE_WIDTH + GAP;
        let mut actions = cell(ui, first, actions_x, columns.width - actions_x - ROW_PAD, first.height(), Layout::right_to_left(Align::Center));
        self.actions(&mut actions, app, state, wait);
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
                    let room = ui.available_width() - button_room(ui, &tr!("Release"));
                    label_up_to(ui, room, RichText::new(tr!("Library: {name}", name = name)).color(TEXT)).on_hover_text(library);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add_enabled(!busy, Flat::new(tr!("Release"), Look::Plain).small())
                            .on_hover_text(tr!("Close it, so the desktop app can open it"))
                            .clicked()
                        {
                            self.send(Job::CloseLibrary(app.id.clone()));
                        }
                    });
                }
                None => drop(ui.label(RichText::new(tr!("Library: none open")).color(MUTED)).on_hover_text(tr!("The AI client opens the library it needs"))),
            }
        } else if let Some(roots) = app.roots() {
            let room = ui.available_width() - button_room(ui, &tr!("Change…"));
            match roots.as_slice() {
                [] => drop(
                    label_up_to(ui, room, RichText::new(tr!("Any file you can open")).color(AMBER))
                        .on_hover_text(tr!("Not restricted: an AI client can have this engine open or save any file you can.")),
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
                if ui.add_enabled(!busy, Flat::new(tr!("Change…"), Look::Plain).small()).clicked() {
                    self.root_edit = Some(RootEdit {
                        id: app.id.clone(),
                        name: app.name.clone(),
                        folders: roots.clone(),
                        any: roots.is_empty(),
                        several: app.profile.multi_root,
                        required: app.profile.root_required,
                        running: state.is_on(),
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

    /// The row's buttons, added from the right edge inwards so the main one lines up in every row:
    /// ⋯, Diagnose, and the one that turns the app on or off.
    fn actions(&mut self, ui: &mut Ui, app: &App, state: &AppState, wait: bool) {
        ui.spacing_mut().item_spacing.x = 6.0;
        // Green means turning on: solid for adding, an outline for turning on again. On the first
        // run, Set up everything is the one solid button, and each row's Add steps back to an outline.
        let add = if self.first_run { Look::Outline } else { Look::Primary };
        let (text, look, job) = match state.kind {
            StateKind::New => (tr!("Add"), add, Job::Add(app.id.clone())),
            StateKind::Off | StateKind::Other | StateKind::NotInstalled => (tr!("Turn on"), Look::Outline, Job::TurnOn(app.id.clone())),
            _ => (tr!("Turn off"), Look::Plain, Job::TurnOff(app.id.clone())),
        };
        if ui.add_enabled(!wait && app.available(), Flat::new(text, look).min_width(84.0)).clicked() {
            self.send(job);
        }
        if ui.add_enabled(!wait, Flat::new(tr!("Diagnose"), Look::Plain)).clicked() {
            self.diagnose(&app.id);
        }
        let on = state.is_on();
        let custom = app.origin == Origin::Custom;
        if !(on || custom) {
            return;
        }
        let more = ui.add_enabled(!wait, Flat::new("···", Look::Plain).padding(8.0).min_width(29.0)).on_hover_text(tr!("More"));
        let menu = egui::Id::new(("more", &app.id));
        if more.clicked() {
            ui.memory_mut(|memory| memory.toggle_popup(menu));
        }
        more_menu(ui, menu, &more, |ui| {
            if on {
                if menu_item(ui, &tr!("Restart the server"), TEXT, None, true).clicked() {
                    self.send(Job::Restart(app.id.clone()));
                }
                if menu_item(ui, &tr!("Add to every AI client"), TEXT, None, true).on_hover_text(tr!("Register it with every AI client that is on")).clicked() {
                    self.send(Job::TurnOn(app.id.clone()));
                }
                ui.separator();
            }
            // Only an app added by hand can be taken out: the others come back from the catalog.
            if menu_item(ui, &tr!("Remove from the hub"), RED, (!custom).then(|| tr!("custom apps only")).as_deref(), custom).clicked() {
                self.send(Job::Remove(app.id.clone()));
            }
        });
    }

    /// The panel's language, at the footer's right: a menu of the languages there are, and the way
    /// to the folder where another one's file goes.
    fn language_menu(&mut self, ui: &mut Ui) {
        let current = lang::current();
        let name = self.languages.iter().find(|language| language.code == current).map_or_else(|| current.clone(), |language| language.name.clone());
        let button = ui.add(Flat::new(name, Look::Quiet).small()).on_hover_text(tr!("The panel's language"));
        let id = egui::Id::new("languages");
        if button.clicked() {
            self.languages = lang::available();
            ui.memory_mut(|memory| memory.toggle_popup(id));
        }
        let languages = self.languages.clone();
        menu(ui, id, &button, true, |ui| {
            for language in &languages {
                let color = if language.code == current { JADE_TEXT } else { TEXT };
                if menu_item(ui, &language.name, color, None, true).clicked() {
                    lang::choose(&language.code);
                    self.send(Job::SetLanguage(language.code.clone()));
                }
            }
            ui.separator();
            let add = menu_item(ui, &tr!("Add a language…"), MUTED, None, true)
                .on_hover_text(tr!("Opens the folder for language files. The README says how to write one."));
            if add.clicked() {
                let folder = lang::folder();
                let _ = std::fs::create_dir_all(&folder);
                ui.ctx().open_url(egui::OpenUrl::new_tab(format!("file:///{}", folder.display().to_string().replace('\\', "/"))));
            }
        });
    }

    /// The windows over the panel, each an `egui::Modal` over the dimmed panel. The last one shown
    /// is on top: what went wrong comes last.
    fn windows(&mut self, ctx: &egui::Context, busy: bool) {
        let diagnosis = lock(&self.shared).diagnosis.clone();
        match diagnosis {
            Some(view) => self.diagnosis_window(ctx, &view, busy),
            None => self.show_report = false,
        }
        self.files_window(ctx, busy);
        self.custom_window(ctx, busy);
        self.uninstall_windows(ctx, busy);
        let failure = lock(&self.shared).failure.clone();
        if let Some(failure) = failure {
            self.failure_window(ctx, &failure);
        }
    }

    /// Opens an app's diagnosis at once, with its checks waiting, and has the worker run them.
    fn diagnose(&mut self, id: &str) {
        let row = lock(&self.shared).rows.iter().find(|row| row.app.id == id).map(|row| (row.app.name.clone(), row.state.is_on()));
        let (name, on) = row.unwrap_or_else(|| (id.to_string(), false));
        lock(&self.shared).diagnosis = Some(DiagnosisView { id: id.to_string(), name, on, planned: doctor::planned(id), checks: Vec::new(), report: None });
        self.send(Job::Diagnose(id.to_string()));
    }

    /// Diagnose: the checks as a list, the fix for what failed in its line, and the full report
    /// one click away.
    fn diagnosis_window(&mut self, ctx: &egui::Context, view: &DiagnosisView, busy: bool) {
        let name = &view.name;
        let running = view.report.is_none();
        // What is wrong: what failed, and what has a fix. Not being in a client is no problem for an
        // app that is off.
        let problem = |check: &Check| check.mark == Mark::Fail || (check.fix.is_some() && (view.on || check.topic != Topic::Client));
        let problems: Vec<&Check> = view.checks.iter().filter(|check| problem(check)).collect();
        let server_up = view.checks.iter().any(|check| check.topic == Topic::Server && check.mark == Mark::Ok);
        let (badge, title, about) = if running {
            (Some(Badge::Working), tr!("Checking {name}…", name = name), tr!("This takes a few seconds."))
        } else if let Some(first) = problems.first() {
            let title = match first.topic {
                Topic::Program => tr!("{name} can't be found", name = name),
                Topic::Client => tr!("{name} is missing from an AI client", name = name),
                Topic::Settings => tr!("{name} runs with old settings", name = name),
                _ => tr!("{name} doesn't answer", name = name),
            };
            let about = if problems.len() == 1 { tr!("One thing to fix.") } else { tr!("{n} things to fix.", n = problems.len()) };
            (Some(Badge::Problem), title, about)
        } else if !view.on {
            (None, tr!("{name} is off", name = name), tr!("Turn it on to use it in your AI clients."))
        } else if server_up {
            (Some(Badge::Good), tr!("{name} is working", name = name), tr!("AI sessions you open now connect to it."))
        } else {
            (Some(Badge::Good), tr!("{name} is ready", name = name), tr!("It starts with the next AI session."))
        };

        let mut close = false;
        let mut fix_clicked: Option<Job> = None;
        let shown = modal(ctx, "diagnosis", DIAGNOSE_WIDTH, |ui| {
            close |= modal_head(ui, badge, &title, &about, true);
            list_box(ui, |ui| {
                for check in &view.checks {
                    let fix = check.fix.as_ref().filter(|_| problem(check)).map(|fix| match fix {
                        Fix::Port(port) => (tr!("Use port {port}", port = port), Job::SetPort(view.id.clone(), *port)),
                        Fix::Restart => (tr!("Restart it"), Job::Restart(view.id.clone())),
                        Fix::TurnOn => (tr!("Add it"), Job::TurnOn(view.id.clone())),
                    });
                    let mark = match check.mark {
                        Mark::Ok => LineMark::Done,
                        Mark::Fail => LineMark::Failed,
                        Mark::Warn | Mark::Unknown => LineMark::Warned,
                        Mark::Off => LineMark::Skipped,
                        Mark::Pending => LineMark::Current,
                    };
                    // What the doctor found comes from the hub, in English: translated as shown.
                    let (name, found) = (lang::any(&check.name), lang::any(&check.found));
                    if check_line(ui, mark, &name, &found, fix.as_ref().map(|(label, _)| label.as_str()), busy) {
                        fix_clicked = fix.map(|(_, job)| job);
                    }
                }
                // While the checks run, the ones still to come: the next is being checked.
                if running {
                    let pending = view.planned.iter().filter(|(topic, name)| !view.checks.iter().any(|check| check.topic == *topic && &check.name == name));
                    for (index, (_, name)) in pending.enumerate() {
                        let (mark, found) = if index == 0 { (LineMark::Current, tr!("checking…")) } else { (LineMark::Waiting, tr!("waiting")) };
                        check_line(ui, mark, &lang::any(name), &found, None, true);
                    }
                }
            });
            // What the fixes do, and what else to know, in a sentence or two.
            if !running && !problems.is_empty() {
                let mut advice: Vec<String> = Vec::new();
                for check in &problems {
                    let line = match (&check.fix, check.topic) {
                        (Some(Fix::Port(_)), _) => tr!("Move {name} to a free port and it starts by itself.", name = name),
                        (Some(Fix::Restart), Topic::Settings) => tr!("Restart it to apply the current settings."),
                        (Some(Fix::Restart), _) => tr!("Restart it to try again: the full report has the engine's last lines."),
                        (Some(Fix::TurnOn), _) => tr!("Add it to every AI client that is on."),
                        (None, Topic::Program) => tr!("Install or update {name} with the ArtCraft launcher.", name = name),
                        (None, _) => continue,
                    };
                    if !advice.contains(&line) {
                        advice.push(line);
                    }
                }
                if problems.iter().any(|check| matches!(check.topic, Topic::Port | Topic::Server | Topic::Engine)) {
                    advice.push(tr!("AI sessions that were open while it wasn't working need to be restarted to reconnect."));
                }
                if !advice.is_empty() {
                    ui.add_space(4.0);
                    ui.label(RichText::new(advice.join(" ")).color(TEXT));
                }
            }
            if self.show_report {
                if let Some(report) = &view.report {
                    ui.add_space(4.0);
                    egui::Frame::new().fill(INK).corner_radius(8).inner_margin(Margin::same(10)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        egui::ScrollArea::both().max_height(220.0).show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(report).monospace().size(11.5).color(TEXT)).extend());
                        });
                    });
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let toggle = if self.show_report { tr!("Hide the full report") } else { tr!("Show the full report") };
                if ui.add_enabled_ui(!running, |ui| quiet_link(ui, &toggle)).inner.clicked() {
                    self.show_report = !self.show_report;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if !running && problems.is_empty() {
                        close |= ui.add(Flat::new(tr!("Close"), Look::Plain)).clicked();
                    } else if ui.add_enabled(!running && !busy, Flat::new(tr!("Check again"), Look::Plain)).clicked() {
                        self.diagnose(&view.id);
                    }
                    if ui.add_enabled(!running, Flat::new(tr!("Copy report"), Look::Plain)).clicked() {
                        ctx.copy_text(view.report.clone().unwrap_or_default());
                        lock(&self.shared).note = Some(Note::done(tr!("Report copied."), tr!("Paste it wherever you ask for help.")));
                    }
                });
            });
        });
        if let Some(job) = fix_clicked {
            // Fixed, then checked again.
            self.send(job);
            self.diagnose(&view.id);
        } else if close || shown.should_close() {
            lock(&self.shared).diagnosis = None;
        }
    }

    /// Files it may use: only some folders, which can be added and taken away, or any file.
    fn files_window(&mut self, ctx: &egui::Context, busy: bool) {
        let Some(edit) = &mut self.root_edit else { return };
        // A folder dropped on the window is added to the list (or takes the place of the one
        // folder an engine can have).
        for dropped in ctx.input(|input| input.raw.dropped_files.iter().filter_map(|file| file.path.clone()).collect::<Vec<_>>()) {
            if dropped.is_dir() {
                edit.add(dropped.to_string_lossy().into_owned());
            }
        }
        let (mut close, mut save) = (false, false);
        let shown = modal(ctx, "files", WINDOW_WIDTH, |ui| {
            let (title, about) = (tr!("Files {name} may use", name = edit.name), tr!("AI clients can open and save files only where you allow it."));
            close |= modal_head(ui, None, &title, &about, true);
            let only = choice(ui, !edit.any, |ui, selected| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tr!("Only these folders")).color(TEXT));
                    let everything = if edit.several { tr!("and everything in them") } else { tr!("and everything in it") };
                    ui.label(RichText::new(everything).size(SMALL_SIZE).color(MUTED));
                });
                if !selected {
                    return;
                }
                ui.add_space(4.0);
                let mut remove = None;
                for (index, folder) in edit.folders.iter().enumerate() {
                    if folder_line(ui, folder) {
                        remove = Some(index);
                    }
                }
                if let Some(index) = remove {
                    edit.folders.remove(index);
                }
                let add = if edit.several || edit.folders.is_empty() { tr!("+ Add a folder…") } else { tr!("Choose another folder…") };
                if ui.add(Flat::new(add, Look::Plain).small()).on_hover_text(tr!("Or drop a folder on this window")).clicked() {
                    if let Some(folder) = rfd::FileDialog::new().set_title(tr!("A folder the app may use")).pick_folder() {
                        edit.add(folder.to_string_lossy().into_owned());
                    }
                }
            });
            if only {
                edit.any = false;
            }
            // An engine with no file access at all without a folder can't be given any file.
            if !edit.required {
                let any = choice(ui, edit.any, |ui, _| {
                    ui.label(RichText::new(tr!("Any file I can open")).color(TEXT));
                    ui.label(
                        RichText::new(tr!("An AI client could open or overwrite any of your files with {name}.", name = edit.name))
                            .size(SMALL_SIZE)
                            .color(AMBER),
                    );
                });
                if any {
                    edit.any = true;
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if edit.running {
                    ui.label(RichText::new(tr!("{name} restarts to apply it.", name = edit.name)).size(SMALL_SIZE).color(MUTED));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let ready = edit.any || !edit.folders.is_empty();
                    save = ui.add_enabled(!busy && ready, Flat::new(tr!("Save"), Look::Confirm)).clicked();
                    close |= ui.add(Flat::new(tr!("Cancel"), Look::Plain)).clicked();
                });
            });
        });
        if save {
            let folders = if edit.any { Vec::new() } else { edit.folders.clone() };
            let _ = self.jobs.send(Job::SetRoots(edit.id.clone(), folders));
        }
        if save || close || shown.should_close() {
            self.root_edit = None;
        }
    }

    /// Add a custom app: its command-line tool, dropped or chosen, then its names, filled in from
    /// the file. How to start it is folded away under More options.
    fn custom_window(&mut self, ctx: &egui::Context, busy: bool) {
        let Some(form) = &mut self.custom else { return };
        // A file dropped on the window is the app's command-line tool.
        if let Some(dropped) = ctx.input(|input| input.raw.dropped_files.first().and_then(|file| file.path.clone())) {
            form.choose(&dropped);
        }
        let hovering = ctx.input(|input| !input.raw.hovered_files.is_empty());
        let recognized = *lock(&form.recognized);
        let (mut close, mut add) = (false, false);
        let shown = modal(ctx, "custom", WINDOW_WIDTH, |ui| {
            close |= modal_head(ui, None, &tr!("Add a custom app"), &tr!("A fork or your own build of an ArtCraft app."), true);
            if form.cli.is_empty() {
                if drop_zone(ui, hovering) {
                    form.pick();
                }
                ui.label(RichText::new(tr!("It's the file whose name ends in -cli, next to the app's main program.")).size(SMALL_SIZE).color(MUTED));
            } else {
                if chosen_file(ui, &form.cli, recognized) {
                    form.pick();
                }
                ui.add_space(4.0);
                ui.columns(2, |columns| {
                    columns[0].label(RichText::new(tr!("Name")).size(SMALL_SIZE).color(MUTED));
                    columns[0].add(egui::TextEdit::singleline(&mut form.name).margin(Margin::symmetric(10, 7)).desired_width(f32::INFINITY));
                    columns[1].label(RichText::new(tr!("Name the AI clients see")).size(SMALL_SIZE).color(MUTED));
                    columns[1].add(
                        egui::TextEdit::singleline(&mut form.id)
                            .font(FontId::monospace(BODY_SIZE))
                            .margin(Margin::symmetric(10, 7))
                            .desired_width(f32::INFINITY),
                    );
                    let color = if form.id.is_empty() || valid_id(&form.id) { CAPTION } else { RED };
                    columns[1].label(RichText::new(tr!("lowercase letters, numbers and dashes")).size(SMALL_SIZE).color(color));
                });
                if fold(ui, form.more, &tr!("More options"), &tr!("how to start it, where it lists recent libraries")) {
                    form.more = !form.more;
                }
                if form.more {
                    egui::Grid::new("more-options").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                        ui.label(RichText::new(tr!("Start it like")).color(TEXT));
                        egui::ComboBox::from_id_salt("like")
                            .width(260.0)
                            .selected_text(if form.like.is_empty() { tr!("work it out from its --help") } else { form.like.clone() })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut form.like, String::new(), tr!("work it out from its --help"));
                                for known in ["lightcraft", "vectorcraft", "photocraft", "figcraft"] {
                                    ui.selectable_value(&mut form.like, known.to_string(), known);
                                }
                            });
                        ui.end_row();
                        ui.label(RichText::new(tr!("Settings folder")).color(TEXT));
                        ui.add(
                            egui::TextEdit::singleline(&mut form.settings)
                                .hint_text(tr!("where a photo app lists its recent libraries"))
                                .margin(Margin::symmetric(10, 7))
                                .desired_width(260.0),
                        );
                        ui.end_row();
                    });
                }
            }
            ui.add_space(8.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let ready = !form.cli.is_empty() && valid_id(&form.id) && recognized != Some(false);
                add = ui.add_enabled(!busy && ready, Flat::new(tr!("Add and turn on"), Look::Primary)).clicked();
                close |= ui.add(Flat::new(tr!("Cancel"), Look::Plain)).clicked();
            });
        });
        if add {
            let text = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
            let options = AddOptions {
                cli: text(&form.cli.replace('"', "")),
                name: text(&form.name),
                like: text(&form.like),
                port: None,
                settings: text(&form.settings).into_iter().collect(),
            };
            let _ = self.jobs.send(Job::AddCustom { id: form.id.trim().to_string(), options });
        }
        if add || close || shown.should_close() {
            self.custom = None;
        }
    }

    /// What went wrong, with the fix as the main button when there is one.
    fn failure_window(&mut self, ctx: &egui::Context, failure: &Failure) {
        let (mut close, mut fix, mut diagnose) = (false, false, false);
        let shown = modal(ctx, "failure", FAILURE_WIDTH, |ui| {
            close |= modal_head(ui, Some(Badge::Problem), &failure.title, "", true);
            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                // Why comes from the hub, in English: translated as shown.
                ui.label(RichText::new(lang::any(&failure.text)).color(TEXT));
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if failure.app.is_some() {
                    diagnose = quiet_link(ui, &tr!("Diagnose")).clicked();
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if let Some((label, _)) = &failure.fix {
                        fix = ui.add(Flat::new(label, Look::Primary)).clicked();
                    }
                    // Without a fix, closing is what there is to do.
                    let look = if failure.fix.is_some() { Look::Plain } else { Look::Confirm };
                    close |= ui.add(Flat::new(tr!("Close"), look)).clicked();
                });
            });
        });
        if fix || diagnose || close || shown.should_close() {
            lock(&self.shared).failure = None;
        }
        if let (true, Some(app), Some((_, port))) = (fix, &failure.app, &failure.fix) {
            self.send(Job::MoveAndTurnOn(app.clone(), *port));
        }
        if let (true, Some(app)) = (diagnose, &failure.app) {
            self.diagnose(app);
        }
    }

    /// Uninstalling: first what it will do, then how far along it is, then what it did.
    fn uninstall_windows(&mut self, ctx: &egui::Context, busy: bool) {
        let progress = lock(&self.shared).uninstalling.clone();
        if let Some(progress) = progress {
            return uninstall_progress(ctx, &progress);
        }
        let Some((summary, plan)) = &self.uninstall else { return };
        let (mut close, mut confirm) = (false, false);
        let steps = uninstall_steps(summary);
        let shown = modal(ctx, "uninstall", WINDOW_WIDTH, |ui| {
            close |= modal_head(ui, Some(Badge::Problem), &tr!("Uninstall Craft MCP Hub?"), &tr!("Here is what happens:"), true);
            if steps.is_empty() {
                ui.label(RichText::new(tr!("There is nothing to remove.")).color(TEXT));
            } else {
                list_box(ui, |ui| {
                    for (index, (_, step)) in steps.iter().enumerate() {
                        step_line(ui, LineMark::Number(index + 1), step, TEXT);
                    }
                })
                .on_hover_text(plan.join("\n"));
            }
            step_line(ui, LineMark::Done, &tr!("Your ArtCraft apps and their files stay as they are."), TEXT);
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr!("Open AI sessions lose these servers.")).size(SMALL_SIZE).color(MUTED));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    confirm = ui.add_enabled(!busy && !steps.is_empty(), Flat::new(tr!("Uninstall"), Look::Danger)).clicked();
                    close |= ui.add(Flat::new(tr!("Cancel"), Look::Plain)).clicked();
                });
            });
        });
        if confirm {
            let stages = steps.iter().map(|(stage, _)| *stage).collect();
            lock(&self.shared).uninstalling =
                Some(Uninstalling { apps: summary.apps, stages, stage: None, done: Vec::new(), left_over: None, finished: false });
            self.send(Job::Uninstall);
        }
        if confirm || close || shown.should_close() {
            self.uninstall = None;
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

// ---------------------------------------------------------------- windows

/// A window's width, by what it holds.
const DIAGNOSE_WIDTH: f32 = 580.0;
const WINDOW_WIDTH: f32 = 540.0;
const FAILURE_WIDTH: f32 = 500.0;
/// Behind a line that failed, and the round mark of a problem.
const RED_ROW: Color32 = Color32::from_rgb(0x22, 0x18, 0x17);
const RED_BADGE: Color32 = Color32::from_rgb(0x2E, 0x1E, 0x1C);

/// The round mark at the top of a window.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Badge {
    Working,
    Good,
    Problem,
}

/// A window over the dimmed panel, `width` wide.
fn modal<R>(ctx: &egui::Context, id: &str, width: f32, add: impl FnOnce(&mut Ui) -> R) -> egui::ModalResponse<R> {
    let frame = egui::Frame::new().fill(ROW).stroke(Stroke::new(1.0_f32, LINE)).corner_radius(12).inner_margin(Margin::same(24));
    egui::Modal::new(egui::Id::new(id)).frame(frame).backdrop_color(Color32::from_black_alpha(130)).show(ctx, |ui| {
        ui.set_width(width - 48.0);
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        add(ui)
    })
}

/// The top of a window: its round mark if it has one, its title and a line about it, and × to
/// close it. Returns whether × was clicked.
fn modal_head(ui: &mut Ui, badge: Option<Badge>, title: &str, about: &str, closable: bool) -> bool {
    let mut close = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        if let Some(badge) = badge {
            let (rect, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
            let center = rect.center();
            match badge {
                Badge::Working => {
                    ui.painter().circle_filled(center, 20.0, BUTTON);
                    paint_spinner(ui, center, 9.0, 2.0);
                }
                Badge::Good => {
                    ui.painter().circle_filled(center, 20.0, JADE_WASH);
                    paint_check(ui.painter(), center, 13.0, JADE_TEXT);
                }
                Badge::Problem => {
                    ui.painter().circle_filled(center, 20.0, RED_BADGE);
                    ui.painter().text(center, egui::Align2::CENTER_CENTER, "!", FontId::proportional(18.0), RED);
                }
            }
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            // A title alone sits level with the mark.
            if about.is_empty() && badge.is_some() {
                ui.add_space(8.0);
            }
            ui.label(RichText::new(title).size(19.0).color(IVORY));
            if !about.is_empty() {
                ui.label(RichText::new(about).size(12.5).color(MUTED));
            }
        });
        if closable {
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                let cross = egui::Label::new(RichText::new("×").size(20.0).color(MUTED)).sense(Sense::click());
                close = ui.add(cross).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tr!("Close")).clicked();
            });
        }
    });
    ui.add_space(6.0);
    close
}

/// The box a window lists its lines in.
fn list_box(ui: &mut Ui, add: impl FnOnce(&mut Ui)) -> egui::Response {
    egui::Frame::new()
        .fill(WINDOW)
        .corner_radius(8)
        .inner_margin(Margin::symmetric(0, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui);
        })
        .response
}

/// How a line of a window's list is marked.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LineMark {
    Done,
    Failed,
    Warned,
    /// Not there or not running, which may be as it should.
    Skipped,
    /// Being done now.
    Current,
    Waiting,
    /// A step to come, by its number.
    Number(usize),
    /// Something left to do by hand.
    Arrow,
}

fn paint_mark(ui: &Ui, center: Pos2, mark: LineMark) {
    let glyph = |text: &str, size: f32, color: Color32| {
        ui.painter().text(center, egui::Align2::CENTER_CENTER, text, FontId::proportional(size), color);
    };
    match mark {
        LineMark::Done => paint_check(ui.painter(), center, 10.0, JADE_TEXT),
        LineMark::Failed => glyph("×", 16.0, RED),
        LineMark::Warned => glyph("!", 14.0, AMBER),
        LineMark::Skipped => glyph("–", 13.0, MUTED),
        LineMark::Current => glyph("…", 13.0, TEXT),
        LineMark::Waiting => glyph("·", 15.0, MUTED),
        LineMark::Number(n) => glyph(&n.to_string(), 13.0, MUTED),
        // Drawn: egui's default fonts have no arrows.
        LineMark::Arrow => {
            let stroke = Stroke::new(1.3_f32, AMBER);
            ui.painter().line_segment([center - vec2(5.0, 0.0), center + vec2(5.0, 0.0)], stroke);
            ui.painter().line_segment([center + vec2(1.5, -3.5), center + vec2(5.0, 0.0)], stroke);
            ui.painter().line_segment([center + vec2(1.5, 3.5), center + vec2(5.0, 0.0)], stroke);
        }
    }
}

/// A line of a checklist: its mark, its name, what was found, and the button for its fix, with
/// the line in red when it failed. Returns whether the button was clicked.
fn check_line(ui: &mut Ui, mark: LineMark, name: &str, found: &str, fix: Option<&str>, busy: bool) -> bool {
    let height = if fix.is_some() { 44.0 } else { 33.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let failed = mark == LineMark::Failed;
    if failed {
        ui.painter().rect_filled(rect, CornerRadius::ZERO, RED_ROW);
    }
    paint_mark(ui, pos2(rect.left() + 20.0, rect.center().y), mark);
    let (name_color, found_color) = match mark {
        LineMark::Failed => (RED, RED),
        LineMark::Warned => (TEXT, AMBER),
        LineMark::Done => (TEXT, TEXT),
        _ => (TEXT, MUTED),
    };
    let body = line_height(ui, BODY_SIZE);
    let mut label = cell(ui, rect, 38.0, 116.0, body, Layout::left_to_right(Align::Center));
    label.label(RichText::new(name).color(name_color));
    let mut clicked = false;
    let button = fix.map_or(0.0, |fix| ui.painter().layout_no_wrap(fix.to_string(), FontId::proportional(BODY_SIZE), TEXT).size().x + 24.0 + 22.0);
    let mut value = cell(ui, rect, 160.0, rect.width() - 160.0 - button - 8.0, body, Layout::left_to_right(Align::Center));
    value.label(RichText::new(found).color(found_color)).on_hover_text(found);
    if let Some(fix) = fix {
        let mut action = cell(ui, rect, rect.width() - button, button - 11.0, height, Layout::right_to_left(Align::Center));
        clicked = action.add_enabled(!busy, Flat::new(fix, Look::Outline).height(28.0)).clicked();
    }
    clicked
}

/// A line that says a step in a sentence, after its mark.
fn step_line(ui: &mut Ui, mark: LineMark, text: &str, color: Color32) {
    egui::Frame::new().inner_margin(Margin { left: 12, right: 12, top: 9, bottom: 9 }).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            let (rect, _) = ui.allocate_exact_size(vec2(16.0, line_height(ui, BODY_SIZE)), Sense::hover());
            paint_mark(ui, rect.center(), mark);
            ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
        });
    });
}

/// An underlined link in muted text, for what a window also offers.
fn quiet_link(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(egui::Label::new(RichText::new(text).color(MUTED).underline()).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A choice in a card: a round button, and what `add` says about it. The chosen card is jade.
/// Returns whether the card was clicked.
fn choice(ui: &mut Ui, selected: bool, add: impl FnOnce(&mut Ui, bool)) -> bool {
    let (fill, border) = if selected { (JADE_WASH, JADE) } else { (Color32::TRANSPARENT, LINE_HOVER) };
    let card = egui::Frame::new().fill(fill).stroke(Stroke::new(1.0_f32, border)).corner_radius(8).inner_margin(Margin::same(14)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let (rect, _) = ui.allocate_exact_size(vec2(16.0, line_height(ui, BODY_SIZE)), Sense::hover());
            if selected {
                ui.painter().circle_stroke(rect.center(), 7.0, Stroke::new(1.5_f32, JADE));
                ui.painter().circle_filled(rect.center(), 4.0, JADE);
            } else {
                ui.painter().circle_stroke(rect.center(), 7.0, Stroke::new(1.5_f32, MUTED));
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                add(ui, selected);
            });
        });
    });
    card.response.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// A folder in the list of those an app may use, with × to take it away. Returns whether × was
/// clicked.
fn folder_line(ui: &mut Ui, folder: &str) -> bool {
    let mut remove = false;
    egui::Frame::new().fill(INK).corner_radius(6).inner_margin(Margin::symmetric(12, 1)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let room = ui.available_width() - 24.0;
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            label_up_to(ui, room, RichText::new(folder).color(TEXT)).on_hover_text(folder);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let cross = egui::Label::new(RichText::new("×").size(16.0).color(MUTED)).sense(Sense::click());
                remove = ui.add(cross).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tr!("Take it away")).clicked();
            });
        });
    });
    remove
}

/// A heading that folds what is under it, with a word about what that is. Returns whether it was
/// clicked.
fn fold(ui: &mut Ui, open: bool, heading: &str, about: &str) -> bool {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let galley = ui.painter().layout_no_wrap(heading.to_string(), FontId::proportional(BODY_SIZE), TEXT);
        let (rect, response) = ui.allocate_exact_size(vec2(16.0 + galley.size().x, 22.0), Sense::click());
        response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::CollapsingHeader, true, open, heading));
        let color = if response.hovered() { IVORY } else { TEXT };
        paint_fold(ui.painter(), pos2(rect.left() + 4.0, rect.center().y), open, color);
        ui.painter().galley_with_override_text_color(pos2(rect.left() + 16.0, rect.center().y - galley.size().y / 2.0), galley, color);
        ui.label(RichText::new(about).size(SMALL_SIZE).color(MUTED));
        response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
    })
    .inner
}

/// Where to drop an app's command-line tool, with a button to choose it instead. Jade while a file
/// is dragged over the window. Returns whether the button was clicked.
fn drop_zone(ui: &mut Ui, hovering: bool) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 148.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(8), if hovering { JADE_WASH } else { WINDOW });
    dashed_rect(ui.painter(), rect.shrink(0.5), 8.0, Stroke::new(1.0_f32, if hovering { JADE } else { LINE_HOVER }));
    // An arrow pointing down, into the box.
    let arrow = pos2(rect.center().x, rect.top() + 46.0);
    let stroke = Stroke::new(1.3_f32, MUTED);
    ui.painter().line_segment([arrow - vec2(0.0, 8.0), arrow + vec2(0.0, 8.0)], stroke);
    ui.painter().line_segment([arrow + vec2(-5.0, 3.0), arrow + vec2(0.0, 8.0)], stroke);
    ui.painter().line_segment([arrow + vec2(5.0, 3.0), arrow + vec2(0.0, 8.0)], stroke);
    let below = rect.with_min_y(rect.top() + 62.0).with_max_y(rect.bottom() - 18.0);
    let mut inside = cell(ui, below, 0.0, rect.width(), below.height(), Layout::top_down(Align::Center));
    inside.label(RichText::new(tr!("Drop the app's command-line tool here")).color(TEXT));
    inside.add_space(2.0);
    inside.add(Flat::new(tr!("or choose the file…"), Look::Plain).small()).clicked()
}

/// The tool that was dropped or chosen: its name, whether the hub can run it, and where it is,
/// with a button to choose another. Returns whether that was clicked.
fn chosen_file(ui: &mut Ui, cli: &str, recognized: Option<bool>) -> bool {
    let path = std::path::Path::new(cli);
    let file = path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_else(|| cli.to_string());
    egui::Frame::new()
        .fill(WINDOW)
        .corner_radius(8)
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                match recognized {
                    Some(true) => paint_check(ui.painter(), rect.center(), 10.0, JADE_TEXT),
                    Some(false) => drop(ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "×", FontId::proportional(16.0), RED)),
                    None => paint_spinner(ui, rect.center(), 6.0, 1.5),
                }
                let mut change = false;
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.set_max_width(ui.available_width() - 80.0);
                    ui.label(RichText::new(&file).color(IVORY));
                    let (verdict, color) = match recognized {
                        Some(true) => (tr!("It has an mcp command, so the hub can run it."), JADE_TEXT),
                        Some(false) => (tr!("It has no mcp command: the hub can't run it."), RED),
                        None => (tr!("Looking at it…"), MUTED),
                    };
                    ui.label(RichText::new(verdict).size(SMALL_SIZE).color(color));
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    ui.label(RichText::new(cli).size(SMALL_SIZE).color(CAPTION)).on_hover_text(cli);
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    change = ui.add(Flat::new(tr!("Change…"), Look::Plain).small()).clicked();
                });
                change
            })
            .inner
        })
        .inner
}

/// A sentence from a message that may start in lower case.
fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// The outline of a rounded box in dashes.
fn dashed_rect(painter: &egui::Painter, rect: Rect, radius: f32, stroke: Stroke) {
    use std::f32::consts::{FRAC_PI_2, PI};
    let corners = [
        (pos2(rect.right() - radius, rect.top() + radius), -FRAC_PI_2),
        (pos2(rect.right() - radius, rect.bottom() - radius), 0.0),
        (pos2(rect.left() + radius, rect.bottom() - radius), FRAC_PI_2),
        (pos2(rect.left() + radius, rect.top() + radius), PI),
    ];
    let mut points = vec![pos2(rect.left() + radius, rect.top())];
    for (center, from) in corners {
        for step in 0..=6 {
            let angle = from + FRAC_PI_2 * step as f32 / 6.0;
            points.push(center + radius * vec2(angle.cos(), angle.sin()));
        }
    }
    points.push(points[0]);
    painter.extend(egui::Shape::dashed_line(&points, stroke, 5.0, 4.0));
}

/// What uninstalling will do, in the three parts it does it in, as they read before it begins.
fn uninstall_steps(summary: &uninstall::Summary) -> Vec<(uninstall::Stage, String)> {
    let mut steps = Vec::new();
    if summary.apps > 0 {
        let step = match (summary.apps, summary.clients.as_slice()) {
            (1, []) => tr!("Turn off your app"),
            (n, []) => tr!("Turn off your {n} apps", n = n),
            (1, clients) => tr!("Turn off your app and take it out of {clients}", clients = and_list(clients)),
            (n, clients) => tr!("Turn off your {n} apps and take them out of {clients}", n = n, clients = and_list(clients)),
        };
        steps.push((uninstall::Stage::Apps, step));
    }
    if summary.programs {
        steps.push((uninstall::Stage::Programs, tr!("Delete the Craft MCP Hub app and its programs")));
    }
    if summary.data {
        steps.push((uninstall::Stage::Data, tr!("Delete its settings, logs and downloaded tools")));
    }
    steps
}

/// A part of uninstalling as it reads while it is under way, once done, and before.
fn uninstall_stage(stage: uninstall::Stage, apps: usize) -> [String; 3] {
    match (stage, apps) {
        (uninstall::Stage::Apps, 1) => [
            tr!("Turning off your app and taking it out of your AI clients…"),
            tr!("Turned off your app and took it out of your AI clients"),
            tr!("Turn off your app and take it out of your AI clients"),
        ],
        (uninstall::Stage::Apps, n) => [
            tr!("Turning off your {n} apps and taking them out of your AI clients…", n = n),
            tr!("Turned off your {n} apps and took them out of your AI clients", n = n),
            tr!("Turn off your {n} apps and take them out of your AI clients", n = n),
        ],
        (uninstall::Stage::Programs, _) => {
            [tr!("Deleting the app and its programs…"), tr!("Deleted the app and its programs"), tr!("Delete the app and its programs")]
        }
        (uninstall::Stage::Data, _) => [
            tr!("Deleting its settings, logs and downloaded tools…"),
            tr!("Deleted its settings, logs and downloaded tools"),
            tr!("Delete its settings, logs and downloaded tools"),
        ],
    }
}

/// Uninstalling under way, then what it did.
fn uninstall_progress(ctx: &egui::Context, progress: &Uninstalling) {
    let failures: Vec<&String> = progress.done.iter().filter_map(|(_, _, outcome)| outcome.as_ref().err()).collect();
    let (badge, title, about) = match (progress.finished, failures.is_empty()) {
        (false, _) => (Badge::Working, tr!("Uninstalling Craft MCP Hub…"), tr!("This takes a few seconds.")),
        (true, true) => (Badge::Good, tr!("Craft MCP Hub is uninstalled"), tr!("Thanks for trying it.")),
        (true, false) => (Badge::Problem, tr!("Not everything could be deleted"), tr!("What is left is below.")),
    };
    let mut close = false;
    modal(ctx, "uninstalling", WINDOW_WIDTH, |ui| {
        modal_head(ui, Some(badge), &title, &about, false);
        list_box(ui, |ui| {
            for &stage in &progress.stages {
                let [doing, done, to_do] = uninstall_stage(stage, progress.apps);
                let failed: Vec<&String> =
                    progress.done.iter().filter(|(part, _, _)| *part == stage).filter_map(|(_, _, outcome)| outcome.as_ref().err()).collect();
                if !failed.is_empty() {
                    for why in failed {
                        step_line(ui, LineMark::Failed, &lang::any(&sentence(why)), RED);
                    }
                } else if progress.finished || progress.stage.is_some_and(|current| current > stage) {
                    step_line(ui, LineMark::Done, &done, TEXT);
                } else if progress.stage == Some(stage) {
                    step_line(ui, LineMark::Current, &doing, TEXT);
                } else {
                    step_line(ui, LineMark::Waiting, &to_do, MUTED);
                }
            }
            if let Some(left) = &progress.left_over {
                // The fonts have no arrow for "Settings → …".
                step_line(ui, LineMark::Arrow, &lang::any(&left.replace(" → ", ", then ")), AMBER);
            }
        });
        if progress.finished {
            step_line(ui, LineMark::Done, &tr!("Your ArtCraft apps are still here."), TEXT);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                close = ui.add(Flat::new(tr!("Close"), Look::Confirm)).clicked();
            });
        } else {
            ui.label(RichText::new(tr!("Keep this window open until it's done.")).size(SMALL_SIZE).color(MUTED));
        }
    });
    if close {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

/// How many apps are on and how many sessions use them, in a pill at the top right.
fn summary(ui: &mut Ui, rows: &[&Row]) {
    let on = rows.iter().filter(|row| row.state.is_on()).count();
    let sessions: usize = rows.iter().filter_map(|row| row.state.status.as_ref()).map(|status| status.sessions).sum();
    let text = match sessions {
        0 => tr!("{on} of {total} apps on", on = on, total = rows.len()),
        1 => tr!("{on} of {total} apps on · 1 session", on = on, total = rows.len()),
        n => tr!("{on} of {total} apps on · {sessions} sessions", on = on, total = rows.len(), sessions = n),
    };
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
        StateKind::Ready => (JADE, true),
        StateKind::Starting | StateKind::New | StateKind::Other => (AMBER, false),
        StateKind::Error | StateKind::PortBusy => (RED, false),
        StateKind::Off | StateKind::NotInstalled => (MUTED, true),
    };
    dot(ui, color, ring);
    let text = match color {
        JADE => JADE_TEXT,
        other => other,
    };
    // The state comes from the hub, in English: translated as shown.
    let word = lang::any(&state.label);
    let word = if state.kind == StateKind::Starting { format!("{word}…") } else { word };
    let label = ui.label(RichText::new(word).color(text));
    if !state.tip.is_empty() {
        label.on_hover_text(lang::any(&state.tip));
    }
}

/// What goes under the state.
enum Detail {
    /// The plain detail, with its tooltip.
    Text(String, String),
    /// What a check found: a few words, and the whole answer.
    Check { ok: bool, short: String, text: String },
    /// The one-click fix for a problem, with why it is needed.
    Fix { label: String, job: Job, tip: String },
}

/// The line under the state: a problem's fix comes first, then what a check found, then the
/// plain detail.
fn row_detail(row: &Row, check: Option<&CheckResult>) -> Option<Detail> {
    let (app, state) = (&row.app, &row.state);
    let fix = |label: String, job: Job, tip: &str| Some(Detail::Fix { label, job, tip: tip.to_string() });
    match (state.kind, row.free_port) {
        // The reason is in the tooltip, and in Diagnose.
        (StateKind::Error, _) => return fix(tr!("Restart it"), Job::Restart(app.id.clone()), &lang::any(&state.tip)),
        (StateKind::PortBusy, Some(port)) => return fix(tr!("Use free port {port}", port = port), Job::SetPort(app.id.clone(), port), &lang::any(&state.tip)),
        _ => {}
    }
    if state.stale && state.is_on() {
        let tip = tr!("It runs with other settings, or was started by another version of the hub.");
        return fix(tr!("Restart to apply changes"), Job::Restart(app.id.clone()), &tip);
    }
    if !state.missing_from.is_empty() {
        let label = tr!("Add it to {clients} too", clients = and_list(&state.missing_from));
        return fix(label, Job::TurnOn(app.id.clone()), &tr!("Register it with every AI client that is on"));
    }
    if let Some(check) = check.filter(|_| state.is_on()) {
        return Some(Detail::Check { ok: check.ok, short: check.short.clone(), text: check.text.clone() });
    }
    // Ready reads the same whether its server waits for a first session or has stopped its idle
    // engine to free memory: the tooltip says which. The detail comes from the hub, in English.
    let detail = if state.kind == StateKind::Ready { tr!("starts with the next session") } else { lang::any(&state.detail) };
    (!detail.is_empty()).then(|| Detail::Text(detail, lang::any(&state.tip)))
}

/// An underlined jade link: a one-click fix, or the way to what is wrong.
fn link(ui: &mut Ui, text: &str, size: f32) -> egui::Response {
    ui.add(egui::Label::new(RichText::new(text).size(size).color(JADE_TEXT).underline()).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The menu under a row's ⋯, opened at the button's right edge.
fn more_menu(ui: &Ui, id: egui::Id, button: &egui::Response, add: impl FnOnce(&mut Ui)) {
    menu(ui, id, button, false, add);
}

/// A menu at a button's right edge, under it or, at the bottom of the window, over it.
fn menu(ui: &Ui, id: egui::Id, button: &egui::Response, above: bool, add: impl FnOnce(&mut Ui)) {
    let (at, pivot) = match above {
        true => (button.rect.right_top() - vec2(0.0, 4.0), egui::Align2::RIGHT_BOTTOM),
        false => (button.rect.right_bottom() + vec2(0.0, 4.0), egui::Align2::RIGHT_TOP),
    };
    popup(ui, id, button, at, pivot, 228.0, add);
}

/// A menu `width` wide, whose `pivot` corner is `at`, while `id` is the open popup. A click in it,
/// outside it, or Escape closes it.
fn popup(ui: &Ui, id: egui::Id, button: &egui::Response, at: Pos2, pivot: egui::Align2, width: f32, add: impl FnOnce(&mut Ui)) {
    if !ui.memory(|memory| memory.is_popup_open(id)) {
        return;
    }
    let area = egui::Area::new(id).kind(egui::UiKind::Popup).order(egui::Order::Foreground).fixed_pos(at).pivot(pivot);
    let shown = area.show(ui.ctx(), |ui| {
        egui::Frame::new().fill(ROW).stroke(Stroke::new(1.0_f32, LINE)).corner_radius(8).inner_margin(Margin::same(4)).show(ui, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui);
        });
    });
    let clicked_inside = shown.response.contains_pointer() && ui.input(|input| input.pointer.any_click());
    if ui.input(|input| input.key_pressed(egui::Key::Escape)) || button.clicked_elsewhere() || clicked_inside {
        ui.memory_mut(|memory| memory.close_popup());
    }
}

/// A line of a menu, lit up under the pointer, with a note under it when it can't be used.
fn menu_item(ui: &mut Ui, text: &str, color: Color32, note: Option<&str>, enabled: bool) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(BODY_SIZE), color);
    let note = note.map(|note| ui.painter().layout_no_wrap(note.to_string(), FontId::proportional(10.5), CAPTION));
    let height = if note.is_some() { 42.0 } else { 31.0 };
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), height), if enabled { Sense::click() } else { Sense::hover() });
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, text));
    if enabled && response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(6), BUTTON);
    }
    let color = if enabled { color } else { color.gamma_multiply(0.75) };
    match note {
        Some(note) => {
            ui.painter().galley_with_override_text_color(pos2(rect.left() + 12.0, rect.top() + 6.0), galley, color);
            ui.painter().galley(pos2(rect.left() + 12.0, rect.bottom() - 6.0 - note.size().y), note, CAPTION);
        }
        None => ui.painter().galley_with_override_text_color(pos2(rect.left() + 12.0, rect.center().y - galley.size().y / 2.0), galley, color),
    }
    response
}

/// A small triangle: pointing down when what it heads is unfolded, right when it is folded.
fn paint_fold(painter: &egui::Painter, center: Pos2, open: bool, color: Color32) {
    let points = if open {
        vec![center + vec2(-4.0, -2.0), center + vec2(4.0, -2.0), center + vec2(0.0, 3.0)]
    } else {
        vec![center + vec2(-2.0, -4.0), center + vec2(3.0, 0.0), center + vec2(-2.0, 4.0)]
    };
    painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
}

/// Puts what `add` draws, about `height` high, in the middle of what is left of the window.
fn in_the_middle(ui: &mut Ui, height: f32, add: impl FnOnce(&mut Ui)) {
    ui.add_space(((ui.available_height() - height) / 2.0).max(0.0));
    ui.vertical_centered(add);
}

/// The panel before the apps have been looked for.
fn looking(ui: &mut Ui) {
    in_the_middle(ui, 90.0, |ui| {
        spinner(ui, 28.0);
        ui.add_space(14.0);
        ui.label(RichText::new(tr!("Looking for the ArtCraft apps on this computer…")).size(15.0).color(TEXT));
        ui.add_space(4.0);
        ui.label(RichText::new(tr!("This takes a moment.")).size(SMALL_SIZE).color(MUTED));
    });
}

impl Panel {
    /// The panel when no ArtCraft app is installed: where to get one.
    fn no_apps(&mut self, ui: &mut Ui, working: bool) {
        in_the_middle(ui, 230.0, |ui| {
            if let Some(icon) = &self.icon {
                ui.add(egui::Image::new((icon.id(), vec2(64.0, 64.0))));
            }
            ui.add_space(18.0);
            ui.label(RichText::new(tr!("No ArtCraft apps on this computer yet")).size(17.0).color(IVORY));
            ui.add_space(6.0);
            ui.label(RichText::new(tr!("Install one with the ArtCraft launcher. It shows up here by itself, ready to set up.")).color(MUTED));
            ui.add_space(20.0);
            let get = Flat::new(tr!("Get the ArtCraft launcher"), Look::Primary);
            let again = Flat::new(tr!("Look again"), Look::Plain);
            let width = [&get.text, &again.text]
                .iter()
                .map(|text| ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(BODY_SIZE), TEXT).size().x + 24.0)
                .sum::<f32>()
                + 8.0;
            ui.allocate_ui_with_layout(vec2(width, 30.0), Layout::left_to_right(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if ui.add(get).on_hover_text(LAUNCHER_URL).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(LAUNCHER_URL));
                }
                if ui.add_enabled(!working, again).clicked() {
                    self.send(Job::LookAgain);
                }
            });
            ui.add_space(16.0);
            ui.label(RichText::new(tr!("Using a fork or your own build? Add it with “Add custom app…” below.")).size(SMALL_SIZE).color(CAPTION));
        });
    }

    /// On the first run, in place of a solid Add in every row: one button that sets them all up.
    fn first_run_card(&mut self, ui: &mut Ui, found: usize, working: bool) {
        egui::Frame::new()
            .fill(JADE_WASH)
            .stroke(Stroke::new(1.0_f32, JADE_LINE))
            .corner_radius(8)
            .inner_margin(Margin { left: 16, right: 10, top: 10, bottom: 10 })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    ui.label(RichText::new(tr!("Found {n} ArtCraft apps on this computer.", n = found)).color(IVORY));
                    ui.label(RichText::new(tr!("Set them all up in one go, or add them one by one below.")).size(SMALL_SIZE).color(MUTED));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let set_up = Flat::new(tr!("Set up everything"), Look::Primary).height(28.0);
                        if ui.add_enabled(!working, set_up).on_hover_text(tr!("Add and turn on every app found")).clicked() {
                            self.send(Job::SetUpNew);
                        }
                    });
                });
            });
    }

    /// What is going on, or what the last action did and what to do next: above the footer's
    /// buttons, until the next action.
    fn status_line(&mut self, ui: &mut Ui, busy: Option<&Busy>, note: Option<&Note>) {
        if busy.is_none() && note.is_none() {
            return;
        }
        ui.horizontal(|ui| {
            ui.set_min_height(22.0);
            ui.spacing_mut().item_spacing.x = 8.0;
            if let Some(busy) = busy {
                spinner(ui, 12.0);
                ui.label(RichText::new(&busy.text).color(TEXT));
                if let Some((step, of)) = busy.step {
                    ui.label(RichText::new(tr!("{step} of {total}", step = step, total = of)).color(MUTED));
                }
            } else if let Some(note) = note {
                match note.tone {
                    Tone::Done => {
                        let (rect, _) = ui.allocate_exact_size(vec2(12.0, 14.0), Sense::hover());
                        paint_check(ui.painter(), rect.center(), 10.0, JADE_TEXT);
                    }
                    Tone::Problem => dot(ui, AMBER, false),
                    Tone::Plain => dot(ui, MUTED, false),
                }
                ui.label(RichText::new(&note.text).color(TEXT));
                if !note.next.is_empty() {
                    ui.label(RichText::new(&note.next).color(MUTED));
                }
                if let Some((label, to)) = &note.link {
                    if link(ui, label, BODY_SIZE).clicked() {
                        match to {
                            Link::Diagnose(id) => self.diagnose(id),
                            Link::Error => {
                                let mut shared = lock(&self.shared);
                                shared.failure = shared.last_failure.clone();
                            }
                        }
                    }
                }
            }
        });
        ui.add_space(8.0);
    }

    /// The apps that aren't on this computer, folded away at the end of the list.
    fn not_installed(&mut self, ui: &mut Ui, width: f32, missing: &[&Row], working: bool) {
        egui::Frame::new().stroke(Stroke::new(1.0_f32, LINE)).corner_radius(8).inner_margin(Margin { left: 16, right: 12, top: 9, bottom: 9 }).show(ui, |ui| {
            ui.set_width(width - 30.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let heading = ui.painter().layout_no_wrap(tr!("Not installed ({n})", n = missing.len()), FontId::proportional(BODY_SIZE), TEXT);
                let (rect, fold) = ui.allocate_exact_size(vec2(16.0 + heading.size().x, 22.0), Sense::click());
                fold.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::CollapsingHeader, true, self.absent_open, "Not installed"));
                let color = if fold.hovered() { IVORY } else { TEXT };
                paint_fold(ui.painter(), pos2(rect.left() + 4.0, rect.center().y), self.absent_open, color);
                ui.painter().galley_with_override_text_color(pos2(rect.left() + 16.0, rect.center().y - heading.size().y / 2.0), heading, color);
                if fold.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.absent_open = !self.absent_open;
                }
                ui.label(RichText::new(tr!("They show up above as soon as you install them.")).size(SMALL_SIZE).color(MUTED));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let refresh = Flat::new(tr!("Refresh the list"), Look::Plain).small();
                    if ui.add_enabled(!working, refresh).on_hover_text(tr!("Fetch the launcher's current list of apps from getartcraft.com")).clicked() {
                        self.send(Job::UpdateCatalog);
                    }
                });
            });
            if self.absent_open {
                ui.add_space(4.0);
                ui.spacing_mut().item_spacing.y = 5.0;
                let shown = if self.absent_all { missing.len() } else { missing.len().min(3) };
                for row in &missing[..shown] {
                    ui.label(RichText::new(format!("{} · {}", row.app.name, row.app.tagline)).size(12.5).color(TEXT));
                }
                if shown < missing.len() {
                    let more =
                        ui.add(egui::Label::new(RichText::new(tr!("and {n} more", n = missing.len() - shown)).size(12.5).color(MUTED)).sense(Sense::click()));
                    if more.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.absent_all = true;
                    }
                }
            }
        });
    }
}

impl eframe::App for Panel {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let (rows, loaded, client_rows, note, busy, checks) = {
            let mut shared = lock(&self.shared);
            shared.checks.retain(|_, check| check.at.elapsed() < Duration::from_secs(90));
            (shared.rows.clone(), shared.loaded, shared.clients.clone(), shared.note.clone(), shared.busy.clone(), shared.checks.clone())
        };
        let working = busy.is_some();
        let (present, missing): (Vec<&Row>, Vec<&Row>) = rows.iter().partition(|r| r.app.available() || r.app.configured);
        let new_apps = present.iter().filter(|r| r.state.kind == StateKind::New).count();
        // The first run: apps found, and none added yet.
        self.first_run = loaded && new_apps >= 2 && present.iter().all(|row| !row.app.configured);

        let newer = if self.newer_dismissed { None } else { lock(&self.shared).newer.clone() };
        let header = egui::Frame::new().fill(WINDOW).inner_margin(Margin { left: 24, right: 24, top: 20, bottom: 15 });
        egui::TopBottomPanel::top("header").frame(header).show(ctx, |ui| self.header(ui, &present, loaded, new_apps, &client_rows, newer.as_ref(), working));

        let footer = egui::Frame::new().fill(INK).inner_margin(Margin { left: 24, right: 24, top: 13, bottom: 14 });
        egui::TopBottomPanel::bottom("footer").frame(footer).show(ctx, |ui| {
            self.status_line(ui, busy.as_ref(), note.as_ref());
            ui.horizontal_top(|ui| {
                // In a narrow window the buttons go onto a second line, and Uninstall… stays at the right.
                let left = (ui.available_width() - 104.0).max(0.0);
                ui.allocate_ui_with_layout(vec2(left, 29.0), Layout::left_to_right(Align::Center).with_main_wrap(true), |ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                    fn button(text: String, look: Look) -> Flat {
                        Flat::new(text, look).height(29.0).padding(13.0)
                    }
                    // On the first run the card at the top sets them up.
                    if new_apps > 0 && !self.first_run {
                        let set_up = tr!("Set up new apps ({n})", n = new_apps);
                        if ui
                            .add_enabled(!working, button(set_up, Look::Primary))
                            .on_hover_text(tr!("Add and turn on every installed app that isn't in the hub yet"))
                            .clicked()
                        {
                            self.send(Job::SetUpNew);
                        }
                    }
                    // Each for when there is something for it to do.
                    let (any_added, any_on) = (present.iter().any(|row| row.app.configured), present.iter().any(|row| row.state.is_on()));
                    if ui.add_enabled(!working && any_added, button(tr!("Turn all on"), Look::Plain)).clicked() {
                        self.send(Job::AllOn);
                    }
                    if ui.add_enabled(!working && any_on, button(tr!("Turn all off"), Look::Plain)).clicked() {
                        self.send(Job::AllOff);
                    }
                    if ui
                        .add_enabled(!working && any_on, button(tr!("Check all"), Look::Plain))
                        .on_hover_text(tr!("A full MCP round trip through every server that is on"))
                        .clicked()
                    {
                        self.send(Job::CheckAll);
                    }
                    if ui.add_enabled(!working && loaded, button(tr!("Add custom app…"), Look::Plain)).clicked() {
                        self.custom = Some(CustomForm::default());
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    let uninstall = Flat::new(tr!("Uninstall…"), Look::DangerOutline).height(29.0).padding(13.0);
                    if ui.add_enabled(!working, uninstall).on_hover_text(tr!("Take Craft MCP Hub off this computer")).clicked() {
                        self.uninstall = Some((uninstall::summary(), uninstall::plan()));
                    }
                });
            });
            ui.add_space(6.0);
            ui.horizontal_top(|ui| {
                let width = (ui.available_width() - 110.0).max(120.0);
                ui.vertical(|ui| {
                    ui.set_max_width(width);
                    let note = tr!("Photo apps start with no library open: the AI client opens the one it needs. A library opens in one program at a time, so release it here before opening it in the desktop app.");
                    ui.label(RichText::new(note).size(SMALL_SIZE).color(MUTED));
                });
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| self.language_menu(ui));
            });
        });

        let list = egui::Frame::new().fill(WINDOW).inner_margin(Margin { left: 16, right: 16, top: 12, bottom: 8 });
        egui::CentralPanel::default().frame(list).show(ctx, |ui| {
            if !loaded {
                return looking(ui);
            }
            if present.is_empty() {
                return self.no_apps(ui, working);
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
                    self.app_row(ui, &columns, row, busy.as_ref(), checks.get(&row.app.id));
                }
                if !missing.is_empty() {
                    ui.add_space(12.0);
                    self.not_installed(ui, columns.width, &missing, working);
                }
            });
        });

        self.windows(ctx, working);
        self.take_screenshot(ctx, self.demo || (loaded && !working));
    }
}

// ---------------------------------------------------------------- pictures

/// Made-up apps for pictures of the panel: `CRAFTHUB_PANEL_DEMO=<scene>` fills it with them instead
/// of this computer's. Nothing is run and nothing is saved meanwhile.
///
/// The scenes: `main` (the main window), `readme` (the same without the update notice, whose
/// version numbers would soon be out of date), `rows-1` and `rows-2` (every state a row can be in),
/// `first-run-looking`, `first-run-none` and `first-run-found`, `feedback-working`,
/// `feedback-done`, `feedback-client`, `feedback-checked`, `feedback-problem` and
/// `feedback-copied` (the status line), and the windows: `diagnose-checking`, `diagnose-problem`,
/// `diagnose-good`, `files`, `custom-start`, `custom-chosen`, `error`, `uninstall-confirm`,
/// `uninstall-working` and `uninstall-done`, `languages` (the language menu open), and the AI
/// clients: `clients-more` (the list under + Add more open), `clients-gone` (a client turned on
/// that isn't found any more) and `clients-none` (none found).
mod demo {
    use super::*;
    use crafthub::config::{AppConfig, Config};
    use crafthub::control::Status;

    /// An app as a scene has it.
    #[derive(Clone, Copy)]
    struct Made {
        id: &'static str,
        /// Another id, for a second row of the same app.
        alias: Option<&'static str>,
        port: u16,
        /// The folders it may use: none chosen, or these (none at all for any file).
        roots: Option<&'static [&'static str]>,
        /// Give it folders even though its profile has none.
        confinable: bool,
        kind: StateKind,
        detail: &'static str,
        tip: &'static str,
        sessions: usize,
        library: Option<&'static str>,
        stale: bool,
        missing_from: &'static [&'static str],
        free_port: Option<u16>,
        /// What a check found: whether it answered, in a few words.
        check: Option<(bool, &'static str)>,
    }

    const APP: Made = Made {
        id: "",
        alias: None,
        port: 0,
        roots: None,
        confinable: false,
        kind: StateKind::On,
        detail: "",
        tip: "",
        sessions: 0,
        library: None,
        stale: false,
        missing_from: &[],
        free_port: None,
        check: None,
    };

    const PICTURES: Option<&[&str]> = Some(&["C:\\Users\\Public\\Pictures"]);
    const DOCUMENTS: Option<&[&str]> = Some(&["C:\\Users\\Public\\Documents"]);
    const DESKTOP: Option<&[&str]> = Some(&["C:\\Users\\Public\\Desktop\\Designs"]);
    const MUSIC: Option<&[&str]> = Some(&["C:\\Users\\Public\\Music"]);
    const NEW: Made = Made { kind: StateKind::New, detail: "installed, not added yet", ..APP };

    const MAIN: &[Made] = &[
        Made { id: "photocraft", port: 7801, roots: PICTURES, detail: "2 sessions", sessions: 2, ..APP },
        Made { id: "vectorcraft", port: 7802, roots: DOCUMENTS, detail: "no sessions", ..APP },
        Made { id: "lightcraft", port: 7803, kind: StateKind::Off, ..APP },
        Made { id: "pdfcraft", port: 7804, roots: DOCUMENTS, detail: "1 session", sessions: 1, ..APP },
        Made { id: "figcraft", port: 7805, roots: DESKTOP, kind: StateKind::Off, ..APP },
        Made { id: "wordcraft", port: 7806, roots: DOCUMENTS, detail: "no sessions", ..APP },
        Made { id: "gridcraft", port: 7808, ..NEW },
        Made { id: "shapecraft", port: 7821, ..NEW },
    ];

    const ROWS_1: &[Made] = &[
        Made { id: "photocraft", port: 7801, roots: PICTURES, detail: "2 sessions", sessions: 2, check: Some((true, "24 tools, 180 ms")), ..APP },
        Made { id: "vectorcraft", port: 7802, roots: DOCUMENTS, detail: "no sessions", check: Some((false, "no answer in 10 s")), ..APP },
        Made { id: "pdfcraft", port: 7804, roots: DOCUMENTS, kind: StateKind::Ready, detail: "starts with the next session", ..APP },
        Made { id: "wordcraft", port: 7806, roots: DOCUMENTS, kind: StateKind::Starting, detail: "the engine is getting ready", ..APP },
        Made { id: "figcraft", port: 7805, roots: DESKTOP, kind: StateKind::Error, tip: "The engine exits at once: see Diagnose.", ..APP },
        Made { id: "lightcraft", port: 7803, kind: StateKind::PortBusy, tip: "Port 7803 is used by another program.", free_port: Some(7811), ..APP },
    ];

    const ROWS_2: &[Made] = &[
        Made { id: "gridcraft", port: 7808, detail: "no sessions", stale: true, ..APP },
        Made { id: "deckcraft", port: 7807, roots: DOCUMENTS, detail: "no sessions", missing_from: &["Codex"], ..APP },
        Made {
            id: "lightcraft",
            alias: Some("lightcraft-holidays"),
            port: 7803,
            detail: "1 session",
            sessions: 1,
            library: Some("D:\\Photos\\Holidays 2026"),
            ..APP
        },
        Made { id: "designcraft", port: 7809, roots: Some(&[]), confinable: true, kind: StateKind::Off, ..APP },
        Made { id: "soundcraft", port: 7810, roots: MUSIC, confinable: true, kind: StateKind::Off, ..APP },
        Made { id: "photocraft", alias: Some("photocraft-menu"), port: 7801, roots: PICTURES, detail: "2 sessions", sessions: 2, ..APP },
    ];

    const FOUND: &[Made] = &[
        Made { id: "photocraft", port: 7801, ..NEW },
        Made { id: "vectorcraft", port: 7802, ..NEW },
        Made { id: "lightcraft", port: 7803, ..NEW },
        Made { id: "pdfcraft", port: 7804, ..NEW },
        Made { id: "figcraft", port: 7805, ..NEW },
        Made { id: "wordcraft", port: 7806, ..NEW },
        Made { id: "gridcraft", port: 7808, ..NEW },
        Made { id: "shapecraft", port: 7821, ..NEW },
    ];

    /// Apps in the catalog that the made-up computer doesn't have.
    const ABSENT: &[&str] = &[
        "filmcraft",
        "effectcraft",
        "anicraft",
        "cadcraft",
        "texturecraft",
        "materialcraft",
        "solidcraft",
        "mailcraft",
        "craft-anything",
        "figcraft",
        "wordcraft",
    ];

    fn label(kind: StateKind) -> &'static str {
        match kind {
            StateKind::NotInstalled => "Not installed",
            StateKind::New => "New",
            StateKind::Off => "Off",
            StateKind::Other => "Other setup",
            StateKind::Ready => "Ready",
            StateKind::Starting => "Starting",
            StateKind::On => "On",
            StateKind::Error => "Error",
            StateKind::PortBusy => "Port in use",
        }
    }

    fn state(kind: StateKind) -> AppState {
        AppState {
            kind,
            label: label(kind).to_string(),
            detail: String::new(),
            tip: String::new(),
            library: None,
            status: None,
            stale: false,
            partial: None,
            missing_from: Vec::new(),
        }
    }

    /// The rows of the made-up apps, and of the ones the computer doesn't have.
    fn rows(made: &[Made], absent: &[&str]) -> Vec<Row> {
        // The apps' names, taglines and colours come from the catalog; the rest is made up here.
        let program = std::env::current_exe().unwrap_or_default().to_string_lossy().into_owned();
        let mut config = Config::default();
        for app in made {
            let saved = AppConfig {
                port: app.port,
                cli: vec![program.clone()],
                roots: app.roots.map(|roots| roots.iter().map(|root| root.to_string()).collect()),
                root_flags: app.confinable.then(|| vec!["--read-root".to_string(), "--write-root".to_string()]),
                ..AppConfig::default()
            };
            config.apps.insert(app.id.to_string(), saved);
        }
        let hub = Hub { config, catalog: catalog::load() };
        let mut rows: Vec<Row> = made
            .iter()
            .filter_map(|made| {
                let mut app = hub.app(made.id)?;
                app.configured = made.kind != StateKind::New;
                if let Some(alias) = made.alias {
                    app.id = alias.to_string();
                }
                let mut state = state(made.kind);
                state.detail = made.detail.to_string();
                state.tip = made.tip.to_string();
                state.library = made.library.map(str::to_string);
                state.stale = made.stale;
                state.missing_from = made.missing_from.to_vec();
                if made.kind == StateKind::On {
                    state.status = Some(Status { app: made.id.to_string(), port: made.port, sessions: made.sessions, ..Status::default() });
                }
                Some(Row { app, state, free_port: made.free_port })
            })
            .collect();
        let plain = Hub { config: Config::default(), catalog: catalog::load() };
        for id in absent {
            if let Some(mut app) = plain.app(id) {
                (app.cli, app.fetchable, app.configured) = (None, false, false);
                rows.push(Row { app, state: state(StateKind::NotInstalled), free_port: None });
            }
        }
        rows
    }

    /// The panel as the scene shows it.
    pub(super) fn shared(scene: &str) -> Shared {
        let (made, absent): (&[Made], &[&str]) = match scene {
            "rows-1" => (ROWS_1, &[]),
            "rows-2" => (ROWS_2, ABSENT),
            "first-run-looking" => (&[], &[]),
            "first-run-none" => (&[], ABSENT),
            "first-run-found" => (FOUND, &[]),
            _ => (MAIN, &[]),
        };
        let checks = made
            .iter()
            .filter_map(|made| {
                let (ok, short) = made.check?;
                let short = lang::any(short);
                Some((made.id.to_string(), CheckResult { ok, short: short.clone(), text: short, at: Instant::now() }))
            })
            .collect();
        let busy = match scene {
            "rows-2" => {
                Some(Busy {
                    text: tr!("Turning on {name}…", name = "SoundCraft"), doing: tr!("Turning on…"), app: Some("soundcraft".to_string()), step: None
                })
            }
            "feedback-working" => Some(Busy {
                text: tr!("Turning on {name}…", name = "GridCraft"),
                doing: tr!("Turning on…"),
                app: Some("gridcraft".to_string()),
                step: Some((1, 2)),
            }),
            _ => None,
        };
        let note = match scene {
            "feedback-done" => Some(Note::done(tr!("{n} new apps are on.", n = 2), tr!("Open a new AI session to use them."))),
            "feedback-client" => Some(Note::done(
                tr!("{client} has your {n} apps now.", client = "Claude Desktop", n = 4),
                tr!("Restart {client} to load them.", client = "Claude Desktop"),
            )),
            "feedback-checked" => Some(Note::done(tr!("All {n} apps that are on answer.", n = 4), "")),
            "feedback-problem" => {
                Some(Note::problem(tr!("{name} doesn't answer.", name = "VectorCraft"), &tr!("See what's wrong"), Link::Diagnose("vectorcraft".to_string())))
            }
            "feedback-copied" => Some(Note::done(tr!("Update command copied."), tr!("Paste it into a terminal, then come back here."))),
            _ => None,
        };
        let newer = !matches!(scene, "readme" | "first-run-looking" | "first-run-none" | "first-run-found");
        // The built-in clients, a few of them found and turned on, or none for "clients-none".
        let (found, on): (&[&str], &[&str]) = match scene {
            "clients-none" => (&[], &[]),
            "clients-gone" => (&["claude-code"], &["claude-code", "cursor"]),
            _ => (&["claude-code", "claude-desktop", "vscode", "codex"], &["claude-code", "claude-desktop", "codex"]),
        };
        let clients = clients::all_in(&Config::default())
            .into_iter()
            .map(|client| ClientRow {
                id: client.id,
                name: client.name,
                found: found.contains(&client.id),
                enabled: on.contains(&client.id),
                needs_restart: client.needs_restart,
                file: home_as_tilde(&client.settings_file()),
            })
            .collect();
        Shared {
            rows: rows(made, absent),
            loaded: scene != "first-run-looking",
            clients,
            note,
            busy,
            checks,
            diagnosis: diagnosis(scene),
            failure: (scene == "error").then(|| Failure {
                title: tr!("{name} couldn't turn on.", name = "PhotoCraft").trim_end_matches('.').to_string(),
                text: tr!(
                    "{why}, so its server can't start there. Port {free} is free.",
                    why = lang::any("Port 7801 is already used by another program"),
                    free = 7811
                ),
                app: Some("photocraft".to_string()),
                fix: Some((tr!("Use port {port} and turn on", port = 7811), 7811)),
            }),
            uninstalling: uninstalling(scene),
            newer: newer.then(|| Newer { version: "0.7.0".to_string(), url: update_check::RELEASES_URL.to_string() }),
            ..Shared::default()
        }
    }

    /// The diagnosis a scene shows: under way, with a problem, or with all well.
    fn diagnosis(scene: &str) -> Option<DiagnosisView> {
        let check = |topic, name: &str, mark, found: &str, fix| Check { topic, name: name.to_string(), mark, found: found.to_string(), fix };
        let program = |version: &str| check(Topic::Program, "Program", Mark::Ok, version, None);
        let files = |folder: &str| check(Topic::Files, "Files", Mark::Ok, &format!("only {folder}"), None);
        let clients = || [check(Topic::Client, "Claude Code", Mark::Ok, "has it", None), check(Topic::Client, "Codex", Mark::Ok, "has it", None)];
        let topics = [Topic::Program, Topic::Files, Topic::Server, Topic::Engine, Topic::Client, Topic::Client];
        let names = ["Program", "Files", "Server", "Engine", "Claude Code", "Codex"];
        let planned: Vec<(Topic, String)> = topics.into_iter().zip(names.map(str::to_string)).collect();
        let report = |name: &str| Some(format!("DIAGNOSIS  {name}  ·  made up for a picture\n\n[OK]  Program   …\n[OK]  Files     …"));
        let view = |id: &str, name: &str, checks: Vec<Check>, report: Option<String>| DiagnosisView {
            id: id.to_string(),
            name: name.to_string(),
            on: true,
            planned: planned.clone(),
            checks,
            report,
        };
        match scene {
            "diagnose-checking" => Some(view("photocraft", "PhotoCraft", vec![program("photocraft-cli 0.23.1"), files("C:\\Users\\Public\\Pictures")], None)),
            "diagnose-problem" => {
                let mut checks = vec![
                    program("vectorcraft-cli 0.24.1"),
                    files("C:\\Users\\Public\\Documents"),
                    check(Topic::Port, "Port 7802", Mark::Fail, "used by another program", Some(Fix::Port(7812))),
                    check(Topic::Server, "Server", Mark::Off, "not running: it can't start on a busy port", None),
                ];
                checks.extend(clients());
                Some(view("vectorcraft", "VectorCraft", checks, report("VectorCraft")))
            }
            "diagnose-good" => {
                let mut checks = vec![
                    program("photocraft-cli 0.23.1"),
                    files("C:\\Users\\Public\\Pictures"),
                    check(Topic::Server, "Server", Mark::Ok, "running on port 7801, for 2 hours", None),
                    check(Topic::Engine, "Engine", Mark::Ok, "ready, with 24 tools", None),
                ];
                checks.extend(clients());
                Some(view("photocraft", "PhotoCraft", checks, report("PhotoCraft")))
            }
            _ => None,
        }
    }

    /// Uninstalling as a scene shows it: under way, or done.
    fn uninstalling(scene: &str) -> Option<Uninstalling> {
        use uninstall::Stage;
        let stages = vec![Stage::Apps, Stage::Programs, Stage::Data];
        match scene {
            "uninstall-working" => Some(Uninstalling { apps: 4, stages, stage: Some(Stage::Programs), done: Vec::new(), left_over: None, finished: false }),
            "uninstall-done" => {
                let done = stages.iter().map(|stage| (*stage, String::new(), Ok(()))).collect();
                let bin = "C:\\Users\\Public\\.crafthub\\bin";
                let left_over = format!(
                    "Delete {bin} once this window is closed, and take it out of your PATH (Settings → \"Edit environment variables for your account\")."
                );
                Some(Uninstalling { apps: 4, stages, stage: None, done, left_over: Some(left_over), finished: true })
            }
            _ => None,
        }
    }

    /// What the scene has open: a row's ⋯ menu, the apps that aren't installed, a window.
    pub(super) fn arrange(panel: &mut Panel, ctx: &egui::Context, scene: &str) {
        match scene {
            "rows-2" => {
                ctx.memory_mut(|memory| memory.open_popup(egui::Id::new(("more", "photocraft-menu"))));
                panel.absent_open = true;
            }
            "languages" => ctx.memory_mut(|memory| memory.open_popup(egui::Id::new("languages"))),
            "clients-more" | "clients-none" => ctx.memory_mut(|memory| memory.open_popup(egui::Id::new("more clients"))),
            "files" => {
                panel.root_edit = Some(RootEdit {
                    id: "photocraft".to_string(),
                    name: "PhotoCraft".to_string(),
                    folders: vec!["C:\\Users\\Public\\Pictures".to_string(), "D:\\Photos\\Edits".to_string()],
                    any: false,
                    several: true,
                    required: false,
                    running: true,
                })
            }
            "custom-start" => panel.custom = Some(CustomForm::default()),
            "custom-chosen" => {
                panel.custom = Some(CustomForm {
                    cli: "C:\\Tools\\Chromaforge\\chromaforge-cli.exe".to_string(),
                    id: "chromaforge".to_string(),
                    name: "Chromaforge".to_string(),
                    recognized: Arc::new(Mutex::new(Some(true))),
                    ..CustomForm::default()
                })
            }
            "uninstall-confirm" => {
                let summary = uninstall::Summary { apps: 4, clients: vec!["Claude Code", "Claude Desktop", "Codex"], programs: true, data: true };
                panel.uninstall = Some((summary, Vec::new()));
            }
            _ => {}
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_read_as_a_sentence() {
        assert_eq!(and_list(&["Codex"]), "Codex");
        assert_eq!(and_list(&["Codex", "Cursor"]), "Codex and Cursor");
        assert_eq!(and_list(&["Codex", "Cursor", "VS Code"]), "Codex, Cursor and VS Code");
    }

    #[test]
    fn a_failed_check_is_said_in_a_few_words() {
        assert_eq!(short_failure("no answer to initialize", Duration::from_millis(10_200)), "no answer in 10 s");
        assert_eq!(short_failure("the program on port 7802 does not answer", Duration::from_millis(300)), "no answer in 1 s");
        assert_eq!(short_failure("no server is running on port 7801", Duration::ZERO), "not running");
        assert_eq!(short_failure("Unknown tool", Duration::ZERO), "Unknown tool");
    }
}
