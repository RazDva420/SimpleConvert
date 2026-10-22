#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod convert;

use convert::{Detected, Group};
use eframe::egui::{self, Color32, FontId, Stroke, Vec2};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc},
    thread,
};

const CANVAS: Color32 = Color32::from_rgb(251, 251, 250);
const SURFACE: Color32 = Color32::from_rgb(247, 246, 243);
const TEXT: Color32 = Color32::from_rgb(47, 52, 55);
const BORDER: Color32 = Color32::from_rgb(133, 130, 123);
const FOCUS: Color32 = Color32::from_rgb(65, 86, 73);
const ERROR: Color32 = Color32::from_rgb(159, 47, 45);

fn configure(ctx: &egui::Context) {
    // Use the installed Windows UI font; no font download or bundled OS font.
    if let Some(font) = std::env::var_os("WINDIR").and_then(|dir| std::fs::read(PathBuf::from(dir).join("Fonts/segoeui.ttf")).ok()) {
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert("windows-ui".into(), egui::FontData::from_owned(font).into());
        fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap().insert(0, "windows-ui".into());
        ctx.set_fonts(fonts);
    }
    ctx.set_visuals(egui::Visuals::light());
    ctx.style_mut(|style| {
        style.spacing.interact_size.y = 36.0;
        style.spacing.button_padding = Vec2::new(12.0, 8.0);
        style.spacing.item_spacing = Vec2::new(8.0, 12.0);
        style.text_styles.insert(egui::TextStyle::Body, FontId::proportional(15.0));
        style.text_styles.insert(egui::TextStyle::Button, FontId::proportional(15.0));
        // ponytail: this frequent utility needs instant feedback, including reduced-motion users.
        style.animation_time = 0.0;
        style.visuals.override_text_color = Some(TEXT);
        style.visuals.window_fill = CANVAS;
        style.visuals.selection.bg_fill = Color32::from_rgb(232, 237, 232);
        style.visuals.selection.stroke = Stroke::new(1.0_f32, FOCUS);
        for state in [&mut style.visuals.widgets.inactive, &mut style.visuals.widgets.hovered,
            &mut style.visuals.widgets.active, &mut style.visuals.widgets.open] {
            state.corner_radius = egui::CornerRadius::same(6);
            state.bg_fill = CANVAS;
            state.weak_bg_fill = CANVAS;
            state.bg_stroke = Stroke::new(1.0_f32, BORDER);
            state.fg_stroke = Stroke::new(1.0_f32, TEXT);
        }
        style.visuals.widgets.hovered.bg_fill = SURFACE;
        style.visuals.widgets.hovered.weak_bg_fill = SURFACE;
        style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, TEXT);
        style.visuals.widgets.active.bg_fill = Color32::from_rgb(235, 234, 230);
        style.visuals.widgets.active.weak_bg_fill = Color32::from_rgb(235, 234, 230);
        style.visuals.widgets.active.bg_stroke = Stroke::new(2.0_f32, FOCUS);
    });
}

fn format_info(format: &str) -> Detected {
    Detected {
        group: if format == "pdf" { Group::Documents } else if convert::IMAGES.contains(&format) { Group::Images } else { Group::Media },
        format: format.into(),
        video: convert::VIDEO.contains(&format),
    }
}

fn targets(from: &str) -> Vec<&'static str> {
    if !from.is_empty() {
        return format_info(from).targets().iter().copied().filter(|t| *t != from).collect();
    }
    let mut targets = Vec::new();
    for source in convert::IMAGES.iter().chain(&["pdf"]).chain(convert::VIDEO).chain(convert::AUDIO) {
        for target in format_info(source).targets() {
            if !targets.contains(target) { targets.push(*target); }
        }
    }
    targets
}

fn plan(paths: Vec<PathBuf>, from: &str, to: &str) -> convert::Result<Vec<(PathBuf, String)>> {
    let mut seen = HashSet::new();
    let mut jobs = Vec::new();
    for path in paths {
        let path = path.canonicalize().map_err(|e| e.to_string())?;
        if !seen.insert(path.clone()) { continue; }
        let detected = convert::detect(&path)?;
        if !from.is_empty() && detected.format != from {
            return Err(format!("{}: выбран {}, обнаружен {}.", path.file_name().unwrap_or_default().to_string_lossy(), from.to_uppercase(), detected.format.to_uppercase()));
        }
        let target = if to.is_empty() { detected.default_target() } else { to };
        if target == detected.format || !detected.targets().contains(&target) {
            return Err(format!("{} → {} не поддерживается.", detected.format.to_uppercase(), target.to_uppercase()));
        }
        jobs.push((path, target.into()));
    }
    Ok(jobs)
}

enum Event {
    Progress(String),
    Finished { files: usize, errors: Vec<String>, cancelled: bool },
}

#[derive(Default)]
struct App {
    from: String,
    to: String,
    status: String,
    detail: String,
    busy: bool,
    close_after: bool,
    cancel: Arc<AtomicBool>,
    receiver: Option<mpsc::Receiver<Event>>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure(&cc.egui_ctx);
        let mut app = Self::default();
        let files: Vec<_> = std::env::args_os().skip(1).map(PathBuf::from).collect();
        if !files.is_empty() { app.start(files, &cc.egui_ctx); }
        app
    }

    fn start(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        let jobs = match plan(paths, &self.from, &self.to) {
            Ok(jobs) if !jobs.is_empty() => jobs,
            Ok(_) => return,
            Err(error) => {
                self.status = "Ошибка · нажмите для подробностей".into();
                self.detail = error;
                return;
            }
        };
        self.busy = true;
        self.status = "Конвертация…".into();
        self.detail.clear();
        self.cancel.store(false, Ordering::Relaxed);
        let cancel = self.cancel.clone();
        let ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        // ponytail: convert sequentially so large files do not compete for memory.
        thread::spawn(move || {
            let total = jobs.len();
            let mut files = 0;
            let mut errors = Vec::new();
            for (i, (path, target)) in jobs.into_iter().enumerate() {
                if cancel.load(Ordering::Relaxed) { break; }
                let _ = tx.send(Event::Progress(format!("Конвертация {} / {total}…", i + 1)));
                ctx.request_repaint();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    convert::convert(&path, &target, &cancel, |message| {
                        let _ = tx.send(Event::Progress(format!("{} / {total} · {message}", i + 1)));
                        ctx.request_repaint();
                    })
                })).unwrap_or_else(|_| Err("Не удалось обработать файл.".into()));
                match result {
                    Ok(outputs) => files += outputs.len(),
                    Err(error) if error == "Отменено" => break,
                    Err(error) => errors.push(format!("{}: {error}", path.file_name().unwrap_or_default().to_string_lossy())),
                }
            }
            let _ = tx.send(Event::Finished { files, errors, cancelled: cancel.load(Ordering::Relaxed) });
            ctx.request_repaint();
        });
    }

    fn events(&mut self) {
        let events: Vec<_> = self.receiver.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        for event in events {
            match event {
                Event::Progress(message) => self.status = message,
                Event::Finished { files, errors, cancelled } => {
                    self.busy = false;
                    self.receiver = None;
                    self.detail = errors.join("\n\n");
                    self.status = if !self.detail.is_empty() {
                        format!("Готово: {files} · ошибок: {} · подробнее", errors.len())
                    } else if cancelled { format!("Отменено · готово: {files}") }
                    else { format!("Готово · файлов: {files}") };
                }
            }
        }
    }

    fn draw(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().frame(egui::Frame::new().fill(CANVAS).inner_margin(20)).show(ctx, |ui| {
            ui.add_enabled_ui(!self.busy, |ui| {
                ui.horizontal(|ui| {
                    let before = (self.from.clone(), self.to.clone());
                    let width = (ui.available_width() - 80.0) / 2.0;
                    let source_label = ui.label("Из");
                    egui::ComboBox::from_id_salt("source").width(width)
                        .selected_text(if self.from.is_empty() { "Авто".into() } else { self.from.to_uppercase() }).show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.from, String::new(), "Авто");
                            for (group, formats) in [("Изображения", convert::IMAGES), ("Документы", &["pdf"][..]), ("Видео", convert::VIDEO), ("Аудио", convert::AUDIO)] {
                                ui.separator();
                                ui.strong(group);
                                for format in formats { ui.selectable_value(&mut self.from, (*format).into(), format.to_uppercase()); }
                            }
                        }).response.labelled_by(source_label.id);
                    if self.from != before.0 { self.to.clear(); }
                    let target_label = ui.label("В");
                    egui::ComboBox::from_id_salt("target").width(width)
                        .selected_text(if self.to.is_empty() { "Авто".into() } else { self.to.to_uppercase() }).show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.to, String::new(), "Авто");
                            for target in targets(&self.from) { ui.selectable_value(&mut self.to, target.into(), target.to_uppercase()); }
                        }).response.labelled_by(target_label.id);
                    if before != (self.from.clone(), self.to.clone()) { self.status.clear(); self.detail.clear(); }
                });
            });
            ui.add_space(2.0);
            let (rect, response) = ui.allocate_exact_size(ui.available_size(), if self.busy { egui::Sense::hover() } else { egui::Sense::click() });
            let dragging = !self.busy && ctx.input(|i| !i.raw.hovered_files.is_empty());
            let active = dragging || response.has_focus();
            let fill = if !self.busy && response.is_pointer_button_down_on() { Color32::from_rgb(234, 232, 227) }
                else if dragging || (!self.busy && response.hovered()) { Color32::from_rgb(239, 242, 236) } else { SURFACE };
            ui.painter().rect_filled(rect, 8, fill);
            ui.painter().rect_stroke(rect, 8, Stroke::new(if active { 2.0_f32 } else { 1.0_f32 }, if active { FOCUS } else { BORDER }), egui::StrokeKind::Inside);
            let label = if dragging { "Отпустите файлы" } else if self.status.is_empty() { "Перетащите файлы" } else { &self.status };
            let galley = ui.painter().layout(label.to_string(), FontId::proportional(16.0), if self.detail.is_empty() { TEXT } else { ERROR }, rect.width() - 24.0);
            let pos = rect.center() - galley.size() * 0.5;
            ui.painter().galley(pos, galley, TEXT);
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, !self.busy, label));
            let response = response.on_hover_text(if !self.detail.is_empty() { &self.detail } else { "Нажмите, чтобы выбрать файлы. Результат появится рядом с оригиналом." });
            if !self.busy { response.clone().on_hover_cursor(egui::CursorIcon::PointingHand); }
            if !self.busy && response.clicked() {
                if !self.detail.is_empty() {
                    rfd::MessageDialog::new().set_title("Ошибка конвертации").set_description(&self.detail).set_level(rfd::MessageLevel::Error).show();
                } else if let Some(files) = rfd::FileDialog::new().set_title("Выберите файлы").pick_files() {
                    self.start(files, ctx);
                }
            }
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.events();
        if !self.busy && self.close_after { ctx.send_viewport_cmd(egui::ViewportCommand::Close); return; }
        if self.busy && ctx.input(|i| i.viewport().close_requested()) {
            self.cancel.store(true, Ordering::Relaxed);
            self.close_after = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        if self.busy && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.cancel.store(true, Ordering::Relaxed);
            self.status = "Отмена…".into();
        }
        let files = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect::<Vec<_>>());
        if !files.is_empty() && !self.busy { self.start(files, ctx); }
        self.draw(ctx);
    }
}

fn main() -> eframe::Result {
    eframe::run_native("SimpleConvert", eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([460.0, 200.0]).with_resizable(false).with_drag_and_drop(true)
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).expect("Valid app icon")),
        ..Default::default()
    }, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_window_and_format_choices() {
        let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).unwrap();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(image::load_from_memory_with_format(include_bytes!("../assets/icon.ico"), image::ImageFormat::Ico).unwrap().width(), 256);
        assert_eq!(targets("pdf"), ["jpg", "png", "txt"]);
        assert!(!targets("mp3").contains(&"mp4"));
        assert!(!targets("png").contains(&"png"));
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut app = App::default();
        let output = ctx.run(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(460.0, 200.0))),
            ..Default::default()
        }, |ctx| app.draw(ctx));
        let labels: Vec<_> = output.shapes.iter().filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.job.text.clone()), _ => None,
        }).collect();
        assert_eq!(labels, ["Из", "Авто", "В", "Авто", "Перетащите файлы"]);
    }
}
