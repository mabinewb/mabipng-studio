use eframe::egui::{self, Color32, RichText, Vec2};
use egui_lucide::Lucide;
use mabipng_studio::{
    batch, browser,
    brush::{Canvas, Shape},
    engine::{self, Document, Mode, Settings},
    framing::{Frame, PaletteMode, Sampling},
    raster::{self, Fragment, Region, Tool},
    storage,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

const GREEN: Color32 = Color32::from_rgb(24, 119, 91);
const MUTED: Color32 = Color32::from_rgb(108, 115, 120);
const RED: Color32 = Color32::from_rgb(180, 50, 66);
const RAW_PALETTE: [[u8; 3]; 4] = [[255; 3], [192; 3], [128; 3], [0; 3]];

struct Entry {
    id: u64,
    path: PathBuf,
    doc: Arc<Document>,
    texture: egui::TextureHandle,
    status: String,
    failed: bool,
    output: Option<PathBuf>,
    painting: Option<Painting>,
    draft: bool,
    generated: bool,
    author_override: Option<String>,
    undo: Vec<EditState>,
    redo: Vec<EditState>,
    auto_palette: bool,
    author_edit: bool,
}

#[derive(Clone)]
struct EditState {
    doc: Arc<Document>,
    author: Option<String>,
    auto_palette: bool,
}

struct Gesture {
    start: (i32, i32),
    before: Vec<u8>,
    moving: Option<Fragment>,
}

struct Painting {
    canvas: Canvas,
    doc: Arc<Document>,
    texture: egui::TextureHandle,
    colors: Vec<[u8; 4]>,
    active: bool,
    unsaved: bool,
    pending_changes: bool,
    viewport: Option<(egui::Rect, f32)>,
    tiles: HashMap<(u32, u32), egui::TextureHandle>,
    palette_hex: Vec<String>,
    initial_palette: Vec<[u8; 3]>,
    selection: Option<Region>,
    gesture: Option<Gesture>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SaveChoice {
    New,
    Existing,
}

impl Entry {
    fn snapshot(&self) -> EditState {
        EditState {
            doc: self.effective_doc().clone(),
            author: self.author_override.clone(),
            auto_palette: self.auto_palette,
        }
    }

    fn checkpoint(&mut self) {
        self.author_edit = false;
        self.undo.push(self.snapshot());
        self.redo.clear();
        while self.undo.len() > 50
            || (self.undo.len() > 1
                && self
                    .undo
                    .iter()
                    .map(|s| s.doc.bytes.len() + s.doc.image.as_raw().len())
                    .sum::<usize>()
                    > 32 * 1024 * 1024)
        {
            self.undo.remove(0);
        }
    }

    fn effective_doc(&self) -> &Arc<Document> {
        self.painting
            .as_ref()
            .map_or(&self.doc, |painting| &painting.doc)
    }

    fn has_unsaved(&self) -> bool {
        self.draft
            || self
                .painting
                .as_ref()
                .is_some_and(|paint| paint.unsaved || paint.active)
    }
}

struct Preview {
    doc: Document,
    texture: egui::TextureHandle,
}

struct FrameDialog {
    source: Arc<Document>,
    texture: egui::TextureHandle,
    target: Option<u64>,
    frame: Frame,
    mode: PaletteMode,
    preserve_white: bool,
    fix_palette: bool,
    author: String,
    preview: Option<Preview>,
    error: Option<String>,
    dirty: bool,
    viewport: Option<(egui::Rect, f32)>,
    initial_import: bool,
}

struct BatchDialog {
    paths: Vec<PathBuf>,
    options: batch::Options,
    running: bool,
    results: Vec<batch::ItemResult>,
    stop: Arc<AtomicBool>,
    error: Option<String>,
    cancelled: bool,
}

#[derive(Clone)]
enum RemoveRequest {
    Close(u64),
    Delete(PathBuf),
}

struct BrowserItem {
    path: PathBuf,
    texture: Option<egui::TextureHandle>,
    error: Option<String>,
}

enum Event {
    BatchItem(batch::ItemResult),
    BatchDone(bool),
    Loaded(PathBuf, Result<Document, String>),
    Preview(u64, Result<Document, String>),
    Saved(u64, Result<(PathBuf, Document), String>),
    Clipboard(Result<Document, String>),
    BrowserList(u64, Result<Vec<PathBuf>, String>),
    BrowserThumbnail(u64, PathBuf, Result<browser::Thumbnail, String>),
}

pub struct Studio {
    entries: Vec<Entry>,
    selected: Option<u64>,
    next_id: u64,
    loading: HashSet<PathBuf>,
    tx: mpsc::Sender<Event>,
    rx: mpsc::Receiver<Event>,
    author: String,
    fix_palette: bool,
    output_dir: Option<PathBuf>,
    preview: Option<Preview>,
    preview_error: Option<String>,
    revision: u64,
    dirty_at: Option<Instant>,
    preview_busy: bool,
    view: usize,
    zoom: f32,
    brush_shape: Shape,
    brush_size: u32,
    paint_color: u8,
    pixel_grid: bool,
    pending: usize,
    save_total: usize,
    save_ok: usize,
    logs: Vec<String>,
    status: String,
    error: bool,
    author_focus: bool,
    screenshot: Option<PathBuf>,
    screenshot_started: bool,
    ready_frames: u32,
    confirm_discard: bool,
    force_close: bool,
    show_new: bool,
    new_width: u32,
    new_height: u32,
    new_author: String,
    new_error: Option<String>,
    new_focus: bool,
    clipboard_busy: bool,
    clipboard_waiting: Option<Document>,
    paste_held: bool,
    save_dialog: Option<bool>,
    browser_dir: PathBuf,
    browser_items: Vec<BrowserItem>,
    browser_epoch: Arc<AtomicU64>,
    browser_busy: bool,
    browser_filter: String,
    activate_after_load: Option<PathBuf>,
    tool: Tool,
    shape_filled: bool,
    fill_global: bool,
    fragment: Option<Fragment>,
    paste_preview: Option<egui::TextureHandle>,
    frame_dialog: Option<FrameDialog>,
    remove_request: Option<RemoveRequest>,
    frame_when_ready: bool,
    conversion_queue: VecDeque<u64>,
    batch_dialog: Option<BatchDialog>,
}

impl Studio {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        paths: Vec<PathBuf>,
        screenshot: Option<PathBuf>,
        painting: bool,
        author: Option<String>,
    ) -> Self {
        let ctx = &cc.egui_ctx;
        let mut fonts = egui::FontDefinitions::default();
        if let Some(bytes) = ["C:/Windows/Fonts/malgun.ttf", "C:/Windows/Fonts/gulim.ttc"]
            .iter()
            .find_map(|path| std::fs::read(path).ok())
        {
            fonts
                .font_data
                .insert("korean".into(), egui::FontData::from_owned(bytes).into());
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts
                    .families
                    .entry(family)
                    .or_default()
                    .insert(0, "korean".into());
            }
        }
        ctx.set_fonts(fonts);
        egui_extras::install_image_loaders(ctx);
        ctx.set_visuals(egui::Visuals::light());
        ctx.style_mut_of(egui::Theme::Light, |style| {
            style.spacing.item_spacing = Vec2::new(8.0, 10.0);
            style.spacing.button_padding = Vec2::new(12.0, 8.0);
            style.spacing.interact_size.y = 32.0;
            style.spacing.scroll = egui::style::ScrollStyle::solid();
            style.visuals.panel_fill = Color32::from_rgb(249, 250, 250);
            style.visuals.window_fill = Color32::WHITE;
            style.visuals.selection.bg_fill = Color32::from_rgb(219, 238, 230);
            style.visuals.selection.stroke.color = GREEN;
            style.visuals.widgets.active.bg_fill = Color32::from_rgb(217, 235, 228);
            for (kind, size) in [
                (egui::TextStyle::Body, 14.0),
                (egui::TextStyle::Button, 14.0),
                (egui::TextStyle::Small, 12.0),
                (egui::TextStyle::Heading, 20.0),
            ] {
                style
                    .text_styles
                    .insert(kind, egui::FontId::proportional(size));
            }
        });
        let (tx, rx) = mpsc::channel();
        let mut app = Self {
            entries: Vec::new(),
            selected: None,
            next_id: 0,
            loading: HashSet::new(),
            tx,
            rx,
            author: String::new(),
            fix_palette: true,
            tool: Tool::Brush,
            shape_filled: false,
            fill_global: false,
            fragment: None,
            paste_preview: None,
            frame_dialog: None,
            remove_request: None,
            frame_when_ready: false,
            conversion_queue: VecDeque::new(),
            batch_dialog: None,
            output_dir: default_output_dir(),
            preview: None,
            preview_error: None,
            revision: 0,
            dirty_at: None,
            preview_busy: false,
            view: if painting { 3 } else { 0 },
            zoom: 0.0,
            brush_shape: Shape::Circle,
            brush_size: 3,
            paint_color: 0,
            pixel_grid: true,
            pending: 0,
            save_total: 0,
            save_ok: 0,
            logs: Vec::new(),
            status: "준비".into(),
            error: false,
            author_focus: false,
            screenshot,
            screenshot_started: false,
            ready_frames: 0,
            confirm_discard: false,
            force_close: false,
            show_new: false,
            new_width: 256,
            new_height: 96,
            new_author: String::new(),
            new_error: None,
            new_focus: false,
            clipboard_busy: false,
            clipboard_waiting: None,
            paste_held: false,
            save_dialog: None,
            browser_dir: default_output_dir().unwrap_or_else(|| PathBuf::from(".")),
            browser_items: Vec::new(),
            browser_epoch: Arc::new(AtomicU64::new(0)),
            browser_busy: false,
            browser_filter: String::new(),
            activate_after_load: None,
        };
        if app.screenshot.is_some() {
            app.author = "미리보기".into();
        }
        if let Some(author) = author {
            app.author = author;
        }
        app.load_paths(ctx, paths);
        app.refresh_browser(ctx);
        app
    }

    fn selected_entry(&self) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|entry| Some(entry.id) == self.selected)
    }

    fn activate_entry(&mut self, id: u64) {
        self.selected = Some(id);
        if let Some(entry) = self.selected_entry() {
            self.fix_palette = entry.auto_palette;
        }
        if let Some(entry) = self.selected_entry()
            && let Some(author) = entry
                .author_override
                .as_ref()
                .or(entry.effective_doc().author.as_ref())
        {
            self.author = author.clone();
        }
        self.invalidate();
    }

    fn refresh_browser(&mut self, ctx: &egui::Context) {
        let epoch = self.browser_epoch.fetch_add(1, Ordering::Relaxed) + 1;
        self.browser_busy = true;
        let folder = self.browser_dir.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let token = self.browser_epoch.clone();
        std::thread::spawn(move || {
            let paths = browser::list_pngs(&folder)
                .map(|paths| {
                    paths
                        .into_iter()
                        .map(|path| path.canonicalize().unwrap_or(path))
                        .collect::<Vec<_>>()
                })
                .map_err(|error| format!("{error:#}"));
            let thumbnails = paths.as_ref().cloned().unwrap_or_default();
            if tx.send(Event::BrowserList(epoch, paths)).is_err() {
                return;
            }
            ctx.request_repaint();
            for path in thumbnails {
                if token.load(Ordering::Relaxed) != epoch {
                    break;
                }
                let thumbnail = browser::thumbnail(&path).map_err(|error| format!("{error:#}"));
                if tx
                    .send(Event::BrowserThumbnail(epoch, path, thumbnail))
                    .is_err()
                {
                    break;
                }
                ctx.request_repaint();
            }
        });
    }

    fn open_browser_item(&mut self, ctx: &egui::Context, path: PathBuf) {
        let path = path.canonicalize().unwrap_or(path);
        if let Some(entry) = self.entries.iter().find(|entry| entry.path == path) {
            let id = entry.id;
            self.activate_entry(id);
            if self
                .selected_entry()
                .is_some_and(|e| !e.effective_doc().is_chat())
            {
                self.queue_current_conversion(ctx);
            }
        } else {
            self.activate_after_load = Some(path.clone());
            self.load_paths(ctx, vec![path]);
        }
    }

    fn set_browser_directory(&mut self, folder: PathBuf) {
        self.browser_dir = folder;
        self.browser_filter.clear();
    }

    fn update_author(&mut self) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| Some(entry.id) == self.selected)
        {
            if entry
                .author_override
                .as_deref()
                .or(entry.effective_doc().author.as_deref())
                == Some(self.author.as_str())
            {
                return;
            }
            if !entry.author_edit {
                entry.checkpoint();
            }
            entry.author_edit = true;
            entry.author_override = Some(self.author.clone());
            entry.draft = true;
            entry.output = None;
        }
        self.invalidate();
    }

    fn add_document(&mut self, ctx: &egui::Context, path: PathBuf, doc: Document, editor: bool) {
        self.next_id += 1;
        let id = self.next_id;
        let texture = texture(ctx, &format!("original-{id}"), &doc.image);
        self.entries.push(Entry {
            id,
            path,
            doc: Arc::new(doc),
            texture,
            status: "새 문서".into(),
            failed: false,
            output: None,
            painting: None,
            draft: true,
            generated: true,
            author_override: (!self.author.is_empty()).then(|| self.author.clone()),
            undo: Vec::new(),
            redo: Vec::new(),
            auto_palette: self.fix_palette,
            author_edit: false,
        });
        self.selected = Some(id);
        if editor {
            self.view = 3;
            self.paint_color = 3;
        }
        self.invalidate();
    }

    pub fn create_blank(
        &mut self,
        ctx: &egui::Context,
        width: u32,
        author: &str,
    ) -> Result<(), String> {
        self.create_blank_sized(ctx, width, 96, author)
    }

    fn create_blank_sized(
        &mut self,
        ctx: &egui::Context,
        width: u32,
        height: u32,
        author: &str,
    ) -> Result<(), String> {
        let mut doc =
            engine::new_chat_sized(width, height, author).map_err(|error| format!("{error:#}"))?;
        if !self.fix_palette {
            doc = Document::from_bytes(
                engine::replace_palette(&doc, &RAW_PALETTE).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        }
        let path = PathBuf::from(format!(
            "chat_{}_{}.png",
            chrono::Local::now().format("%Y%m%d_%H%M%S"),
            storage::safe_name(author)
        ));
        self.author = author.to_owned();
        self.add_document(ctx, path, doc, true);
        Ok(())
    }

    fn open_new_dialog(&mut self) {
        self.new_author = self.author.clone();
        self.new_error = None;
        self.new_focus = true;
        self.show_new = true;
    }

    pub fn start_new(&mut self, ctx: &egui::Context, width: u32) {
        if let Err(error) = self.create_blank(ctx, width, &self.author.clone()) {
            self.new_width = width.clamp(1, 256);
            self.open_new_dialog();
            self.new_error = Some(error);
        }
    }

    pub fn set_zoom(&mut self, zoom: f32) {
        if zoom.is_finite() {
            self.zoom = zoom.clamp(0.0, 6.0);
        }
    }

    fn new_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_new {
            return;
        }
        let mut create = false;
        egui::Modal::new(egui::Id::new("new-chat")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("새 그림대화");
            ui.label("작성자");
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.new_author)
                    .desired_width(f32::INFINITY)
                    .background_color(Color32::from_rgb(242, 245, 244))
                    .margin(Vec2::new(8.0, 8.0)),
            );
            if self.new_focus {
                response.request_focus();
                self.new_focus = false;
            }
            ui.label("캔버스 크기");
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut self.new_width)
                        .range(1..=256)
                        .prefix("W ")
                        .suffix(" px"),
                );
                ui.label("×");
                ui.add(
                    egui::DragValue::new(&mut self.new_height)
                        .range(1..=96)
                        .prefix("H ")
                        .suffix(" px"),
                );
            });
            ui.add(egui::Slider::new(&mut self.new_width, 1..=256).show_value(false));
            ui.horizontal(|ui| {
                for width in [96, 128, 192, 256] {
                    ui.selectable_value(&mut self.new_width, width, width.to_string());
                }
            });
            ui.label("기본 팔레트");
            palette(
                ui,
                if self.fix_palette {
                    &engine::DEFAULT_CHAT_PALETTE
                } else {
                    &RAW_PALETTE
                },
            );
            if let Some(error) = &self.new_error {
                ui.colored_label(RED, error);
            }
            ui.horizontal(|ui| {
                if command(ui, Lucide::FilePlus, "만들기", true).clicked() {
                    create = true;
                }
                if command(ui, Lucide::X, "취소", false).clicked() {
                    self.show_new = false;
                }
            });
        });
        if create {
            match self.create_blank_sized(
                ctx,
                self.new_width,
                self.new_height,
                &self.new_author.clone(),
            ) {
                Ok(()) => {
                    self.show_new = false;
                    self.new_error = None;
                }
                Err(error) => self.new_error = Some(error),
            }
        }
    }

    fn paste_image(&mut self, ctx: &egui::Context) {
        if self.clipboard_busy || self.clipboard_waiting.is_some() {
            return;
        }
        self.clipboard_busy = true;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<Document> {
                let mut clipboard = arboard::Clipboard::new().map_err(|e| anyhow::anyhow!("클립보드를 열 수 없습니다: {e}"))?;
                let image = clipboard.get_image().map_err(|e| anyhow::anyhow!("클립보드에서 이미지를 가져올 수 없습니다. 이미지를 복사한 뒤 다시 시도해 주시기 바랍니다. ({e})"))?;
                engine::document_from_rgba(u32::try_from(image.width)?, u32::try_from(image.height)?, image.bytes.as_ref())
            })().map_err(|e| format!("{e:#}"));
            let _ = tx.send(Event::Clipboard(result));
            ctx.request_repaint();
        });
    }

    fn clipboard_shortcut(&mut self, ctx: &egui::Context) {
        let (focused, release, release_command, paste_event) = ctx.input(|input| (
            input.focused,
            input.events.iter().any(|event| matches!(event, egui::Event::Key { key: egui::Key::V, pressed: false, .. })),
            input.events.iter().any(|event| matches!(event, egui::Event::Key { key: egui::Key::V, pressed: false, modifiers, .. } if modifiers.command)),
            input.events.iter().any(|event| matches!(event, egui::Event::Paste(_))),
        ));
        let down = focused && native_paste_down(ctx);
        if focused
            && !self.paste_held
            && (down || paste_event || release_command)
            && !ctx.egui_wants_keyboard_input()
            && !self.show_new
            && self.save_dialog.is_none()
            && !self.confirm_discard
            && self.paste_preview.is_none()
            && self.frame_dialog.is_none()
            && self.remove_request.is_none()
            && self.batch_dialog.is_none()
        {
            if self.view == 3 && self.tool == Tool::Select && self.fragment.is_some() {
                self.request_selection_paste(ctx);
            } else {
                self.paste_image(ctx);
            }
        }
        if release || !focused {
            self.paste_held = false;
        } else if down || paste_event {
            self.paste_held = true;
        }
    }

    fn invalidate(&mut self) {
        self.revision += 1;
        self.dirty_at = Some(Instant::now());
        self.preview = None;
        self.preview_error = None;
        self.preview_busy = false;
    }

    fn settings_for(&self, doc: &Document) -> Settings {
        let use_author = self.needs_author(doc);
        Settings {
            mode: Mode::Auto,
            author: if use_author && !self.author.is_empty() {
                Some(self.author.clone())
            } else {
                None
            },
            fix_palette: self.fix_palette,
            preserve_white_background: true,
            resize: None,
        }
    }

    fn needs_author(&self, doc: &Document) -> bool {
        !doc.is_chat()
    }

    fn settings_for_entry(&self, entry: &Entry) -> Settings {
        let mut settings = self.settings_for(entry.effective_doc());
        settings.fix_palette = entry.auto_palette;
        if let Some(author) = &entry.author_override {
            settings.author = Some(author.clone());
        } else if entry.generated {
            settings.author = (!self.author.is_empty()).then(|| self.author.clone());
        }
        settings
    }

    fn message(&mut self, text: String, error: bool) {
        self.status = text.clone();
        self.error = error;
        self.logs.push(text);
        if self.logs.len() > 300 {
            self.logs.remove(0);
        }
    }

    fn load_paths(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>) {
        if let Some(path) = paths.first() {
            self.activate_after_load = Some(path.canonicalize().unwrap_or_else(|_| path.clone()));
        }
        let mut accepted = Vec::new();
        for path in paths {
            let path = path.canonicalize().unwrap_or(path);
            if let Some(entry) = self.entries.iter().find(|entry| entry.path == path) {
                let id = entry.id;
                self.activate_entry(id);
                if self.activate_after_load.as_ref() == Some(&path) {
                    self.activate_after_load = None;
                }
                if self
                    .selected_entry()
                    .is_some_and(|e| !e.effective_doc().is_chat())
                {
                    self.queue_current_conversion(ctx);
                }
                continue;
            }
            if self.loading.contains(&path) {
                continue;
            }
            if !path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
            {
                self.message(
                    format!("PNG 파일을 선택해 주시기 바랍니다: {}", display_name(&path)),
                    true,
                );
                continue;
            }
            self.loading.insert(path.clone());
            accepted.push(path);
        }
        if accepted.is_empty() {
            return;
        }
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            for path in accepted {
                let result = Document::load(&path).map_err(|e| format!("{e:#}"));
                if tx.send(Event::Loaded(path, result)).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
    }

    fn open_files(&mut self, ctx: &egui::Context) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("PNG 파일 열기")
            .add_filter("PNG 이미지", &["png"])
            .pick_file()
        {
            self.load_paths(ctx, vec![path]);
        }
    }

    fn receive(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::BatchItem(result) => {
                    if let Some(dialog) = &mut self.batch_dialog {
                        dialog.results.push(result);
                    }
                }
                Event::BatchDone(cancelled) => {
                    if let Some(dialog) = &mut self.batch_dialog {
                        dialog.running = false;
                        dialog.cancelled = cancelled;
                    }
                    self.refresh_browser(ctx);
                }
                Event::Loaded(path, result) => {
                    self.loading.remove(&path);
                    let activate = self.activate_after_load.as_ref() == Some(&path);
                    if activate {
                        self.activate_after_load = None;
                    }
                    match result {
                        Ok(doc) => {
                            let needs_conversion = !doc.is_chat();
                            self.next_id += 1;
                            let id = self.next_id;
                            let texture = texture(ctx, &format!("original-{id}"), &doc.image);
                            self.entries.push(Entry {
                                id,
                                path,
                                doc: Arc::new(doc),
                                texture,
                                status: "대기".into(),
                                failed: false,
                                output: None,
                                painting: None,
                                draft: false,
                                generated: false,
                                author_override: None,
                                undo: Vec::new(),
                                redo: Vec::new(),
                                auto_palette: self.fix_palette,
                                author_edit: false,
                            });
                            if self.selected.is_none() || activate {
                                self.activate_entry(id);
                            }
                            if needs_conversion {
                                self.conversion_queue.push_back(id);
                            }
                        }
                        Err(error) => {
                            self.message(format!("{}: {error}", display_name(&path)), true)
                        }
                    }
                }
                Event::Preview(revision, result) if revision == self.revision => {
                    self.preview_busy = false;
                    match result {
                        Ok(doc) => {
                            let texture = texture(ctx, "result", &doc.image);
                            self.preview = Some(Preview { doc, texture });
                        }
                        Err(error) => self.preview_error = Some(error),
                    }
                }
                Event::Preview(_, _) => {}
                Event::BrowserList(epoch, result) => {
                    if epoch != self.browser_epoch.load(Ordering::Relaxed) {
                        continue;
                    }
                    self.browser_busy = false;
                    match result {
                        Ok(paths) => {
                            let mut old: HashMap<_, _> = self
                                .browser_items
                                .drain(..)
                                .map(|item| (item.path.clone(), item))
                                .collect();
                            self.browser_items = paths
                                .into_iter()
                                .map(|path| {
                                    let texture = old.remove(&path).and_then(|item| item.texture);
                                    BrowserItem {
                                        path,
                                        texture,
                                        error: None,
                                    }
                                })
                                .collect();
                        }
                        Err(error) => {
                            self.message(format!("폴더를 읽을 수 없습니다: {error}"), true)
                        }
                    }
                }
                Event::BrowserThumbnail(epoch, path, result) => {
                    if epoch != self.browser_epoch.load(Ordering::Relaxed) {
                        continue;
                    }
                    if let Some(item) = self.browser_items.iter_mut().find(|item| item.path == path)
                    {
                        match result {
                            Ok(thumbnail) => {
                                item.texture = Some(texture(
                                    ctx,
                                    &format!("browser-{}", path.display()),
                                    &thumbnail.image,
                                ));
                                item.error = None;
                            }
                            Err(error) => item.error = Some(error),
                        }
                    }
                }
                Event::Clipboard(result) => {
                    self.clipboard_busy = false;
                    match result {
                        Ok(doc) => {
                            self.clipboard_waiting = Some(doc);
                        }
                        Err(error) => self.message(error, true),
                    }
                }
                Event::Saved(id, result) => {
                    self.pending = self.pending.saturating_sub(1);
                    let entry = self.entries.iter_mut().find(|entry| entry.id == id);
                    match result {
                        Ok((path, doc)) => {
                            let path = path.canonicalize().unwrap_or(path);
                            self.save_ok += 1;
                            if let Some(entry) = entry {
                                let doc = Arc::new(doc);
                                entry.path = path.clone();
                                entry.texture = texture(ctx, "saved-source", &doc.image);
                                entry.doc = doc.clone();
                                entry.generated = false;
                                entry.author_override = None;
                                entry.author_edit = false;
                                entry.output = Some(path.clone());
                                entry.status = "저장 완료".into();
                                entry.failed = false;
                                entry.draft = false;
                                if let Some(painting) = &mut entry.painting {
                                    painting.colors = engine::palette_rgba(&doc);
                                    painting.palette_hex =
                                        doc.palette.iter().map(|&rgb| hex_color(rgb)).collect();
                                    painting.doc = doc;
                                    painting.tiles.clear();
                                    painting.unsaved = false;
                                }
                            }
                            self.message(format!("저장 완료: {}", path.display()), false);
                        }
                        Err(error) => {
                            if let Some(entry) = entry {
                                entry.status = "저장 실패".into();
                                entry.failed = true;
                            }
                            self.message(format!("저장 실패: {error}"), true);
                        }
                    }
                    if self.pending == 0 {
                        self.refresh_browser(ctx);
                        self.message(
                            format!(
                                "{}개 저장 완료 · {}개 실패",
                                self.save_ok,
                                self.save_total - self.save_ok
                            ),
                            self.save_ok != self.save_total,
                        );
                    }
                }
            }
        }
        self.open_waiting_clipboard(ctx);
        self.open_next_conversion(ctx);
    }

    fn open_waiting_clipboard(&mut self, ctx: &egui::Context) {
        if self.pending == 0
            && self.batch_dialog.is_none()
            && self.frame_dialog.is_none()
            && self.remove_request.is_none()
            && !self.show_new
            && self.save_dialog.is_none()
            && !self.confirm_discard
            && self.paste_preview.is_none()
            && let Some(doc) = self.clipboard_waiting.take()
        {
            self.open_framing(ctx, Arc::new(doc), None);
        }
    }

    fn start_preview(&mut self, ctx: &egui::Context) {
        self.dirty_at = None;
        let Some(entry) = self.selected_entry() else {
            return;
        };
        if !entry.effective_doc().is_chat() {
            return;
        }
        let settings = self.settings_for_entry(entry);
        let doc = entry.effective_doc().clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let revision = self.revision;
        self.preview_busy = true;
        std::thread::spawn(move || {
            let result = engine::process(&doc, &settings)
                .and_then(Document::from_bytes)
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Event::Preview(revision, result));
            ctx.request_repaint();
        });
    }

    fn request_save(&mut self, ctx: &egui::Context) {
        if self.pending > 0
            || self.batch_dialog.is_some()
            || self.frame_dialog.is_some()
            || !self.loading.is_empty()
        {
            return;
        }
        self.finish_strokes(ctx);
        if self
            .selected_entry()
            .is_some_and(|e| !e.effective_doc().is_chat())
        {
            self.queue_current_conversion(ctx);
        } else if self.selected_entry().is_some() {
            self.save_dialog = Some(true);
        }
    }

    fn save_modal(&mut self, ctx: &egui::Context) {
        let Some(_) = self.save_dialog else {
            return;
        };
        let entries: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| Some(entry.id) == self.selected)
            .collect();
        let existing = entries
            .iter()
            .filter(|entry| !entry.generated && entry.doc.is_chat() && entry.path.is_file())
            .count();
        let mut choice = None;
        let mut cancel = false;
        egui::Modal::new(egui::Id::new("save-choice")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading("저장 방식");
            ui.label(format!("{}개 파일", entries.len()));
            egui::ScrollArea::vertical()
                .id_salt("save-targets")
                .max_height(110.0)
                .show(ui, |ui| {
                    for entry in &entries {
                        ui.add(
                            egui::Label::new(RichText::new(display_name(&entry.path)).small())
                                .truncate(),
                        )
                        .on_hover_text(entry.path.display().to_string());
                    }
                });
            ui.label(
                RichText::new(format!(
                    "chat_{}_작성자.png",
                    chrono::Local::now().format("%Y%m%d_%H%M%S")
                ))
                .small()
                .color(MUTED),
            );
            if ui
                .add_sized(
                    [ui.available_width(), 36.0],
                    primary_button(Lucide::FilePlus, "현재 시간으로 새 파일 저장"),
                )
                .clicked()
            {
                choice = Some(SaveChoice::New);
            }
            if entries.len() == 1 && existing == 1 {
                ui.add(
                    egui::Label::new(
                        RichText::new(entries[0].path.display().to_string())
                            .small()
                            .color(MUTED),
                    )
                    .wrap(),
                );
            }
            if ui
                .add_enabled(
                    existing > 0,
                    egui::Button::image_and_text(
                        Lucide::Save.size(17.0).color(MUTED).image(),
                        "기존 파일에 저장",
                    ),
                )
                .clicked()
            {
                choice = Some(SaveChoice::Existing);
            }
            if existing > 0 && existing < entries.len() {
                ui.label(
                    RichText::new("새 문서는 현재 시간의 새 파일로 저장됩니다.")
                        .small()
                        .color(MUTED),
                );
            }
            if command(ui, Lucide::X, "취소", false).clicked() {
                cancel = true;
            }
        });
        if cancel {
            self.save_dialog = None;
        }
        if let Some(choice) = choice {
            self.save_dialog = None;
            self.save_current_as(ctx, choice);
        }
    }

    fn save_current_as(&mut self, ctx: &egui::Context, choice: SaveChoice) {
        if self.pending > 0
            || self.batch_dialog.is_some()
            || self.frame_dialog.is_some()
            || !self.loading.is_empty()
        {
            return;
        }
        self.finish_strokes(ctx);
        if self
            .selected_entry()
            .is_some_and(|e| !e.effective_doc().is_chat())
        {
            self.queue_current_conversion(ctx);
            return;
        }
        let mut jobs = Vec::new();
        for entry in &self.entries {
            if Some(entry.id) != self.selected {
                continue;
            }
            if let Err(error) = engine::validate_game_size(entry.effective_doc()) {
                self.message(error.to_string(), true);
                return;
            }
            let settings = self.settings_for_entry(entry);
            if settings.author.is_none()
                && (self.needs_author(entry.effective_doc()) || entry.generated)
            {
                self.message(
                    "새 그림대화 또는 작성자 변경에 사용할 닉네임을 입력해 주시기 바랍니다.".into(),
                    true,
                );
                self.author_focus = true;
                return;
            }
            jobs.push((
                entry.id,
                entry.path.clone(),
                entry.effective_doc().clone(),
                settings,
                entry.generated,
                !entry.generated && entry.doc.is_chat() && entry.path.is_file(),
            ));
        }
        if jobs.is_empty() {
            return;
        }
        self.pending = jobs.len();
        self.save_total = jobs.len();
        self.save_ok = 0;
        for entry in &mut self.entries {
            if jobs.iter().any(|job| job.0 == entry.id) {
                entry.status = "저장 중".into();
                entry.failed = false;
            }
        }
        self.error = false;
        let output_dir = self.output_dir.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            for (id, path, doc, settings, _generated, can_overwrite) in jobs {
                let result = (|| -> anyhow::Result<(PathBuf, Document)> {
                    let bytes = engine::process(&doc, &settings)?;
                    let saved_doc = Document::from_bytes(bytes.clone())?;
                    engine::validate_game_size(&saved_doc)?;
                    if choice == SaveChoice::Existing && can_overwrite {
                        return Ok((storage::save_existing(&path, &bytes)?, saved_doc));
                    }
                    let converting = settings.mode == Mode::Convert
                        || (settings.mode == Mode::Auto && !doc.is_chat());
                    let directory = output_dir.clone().unwrap_or_else(|| {
                        path.parent().unwrap_or(Path::new(".")).join(if converting {
                            "converted"
                        } else {
                            "palette_fixed"
                        })
                    });
                    let filename = {
                        format!(
                            "chat_{}_{}.png",
                            chrono::Local::now().format("%Y%m%d_%H%M%S"),
                            storage::safe_name(
                                settings
                                    .author
                                    .as_deref()
                                    .or(doc.author.as_deref())
                                    .unwrap_or("author")
                            )
                        )
                    };
                    Ok((
                        storage::save_unique(&directory, &filename, &bytes)?,
                        saved_doc,
                    ))
                })()
                .map_err(|e| format!("{}: {e:#}", display_name(&path)));
                if tx.send(Event::Saved(id, result)).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
    }

    fn import_author(&mut self) {
        if self.pending > 0 {
            self.message(
                "저장이 완료된 후 작성자를 변경해 주시기 바랍니다.".into(),
                true,
            );
            return;
        }
        if let Some(path) = rfd::FileDialog::new()
            .set_title("작성자를 가져올 그림대화 PNG 선택")
            .add_filter("그림대화 PNG", &["png"])
            .pick_file()
        {
            match Document::load(&path) {
                Ok(doc) if doc.is_chat() => {
                    self.author = doc.author.unwrap_or_default();
                    self.update_author();
                    self.message(
                        "그림대화 파일에서 작성자 이름을 가져왔습니다.".into(),
                        false,
                    );
                }
                Ok(_) => self.message(
                    "정상적인 그림대화 파일을 선택해 주시기 바랍니다.".into(),
                    true,
                ),
                Err(error) => {
                    self.message(format!("작성자를 가져오지 못했습니다: {error:#}"), true)
                }
            }
        }
    }

    fn header(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::Panel::top("header")
            .exact_size(64.0)
            .frame(
                egui::Frame::new()
                    .fill(Color32::WHITE)
                    .inner_margin(egui::Margin::symmetric(20, 12)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.add(Lucide::Images.size(27.0).color(GREEN));
                    ui.label(RichText::new("MabiPNG Studio").size(22.0).strong());
                    ui.label(RichText::new("그림대화").color(MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if command(ui, Lucide::FolderOpen, "PNG 열기", true).clicked() {
                            self.open_files(ctx);
                        }
                        if icon_button(ui, Lucide::ClipboardPaste, "클립보드 이미지 붙여넣기")
                            .clicked()
                        {
                            self.paste_image(ctx);
                        }
                        if command(ui, Lucide::FilePlus, "새 그림대화", false).clicked() {
                            self.open_new_dialog();
                        }
                        if command(ui, Lucide::Layers, "일괄 변환", false).clicked() {
                            self.open_batch(ctx);
                        }
                        if icon_button(ui, Lucide::UserRound, "그림대화 PNG에서 작성자 가져오기")
                            .clicked()
                        {
                            self.import_author();
                        }
                    });
                });
            });
    }

    fn footer(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::Panel::bottom("footer")
            .exact_size(70.0)
            .frame(
                egui::Frame::new()
                    .fill(Color32::WHITE)
                    .inner_margin(egui::Margin::symmetric(18, 12)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.vertical(|ui| {
                        ui.set_max_width((ui.available_width() - 245.0).max(200.0));
                        if self.pending > 0 {
                            ui.add(
                                egui::ProgressBar::new(
                                    (self.save_total - self.pending) as f32
                                        / self.save_total.max(1) as f32,
                                )
                                .text(format!(
                                    "저장 중 · {} / {}",
                                    self.save_total - self.pending,
                                    self.save_total
                                ))
                                .desired_width(260.0),
                            );
                        } else {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&self.status).color(if self.error {
                                        RED
                                    } else {
                                        MUTED
                                    }),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&self.status);
                        }
                        ui.label(
                            RichText::new(format!("열린 문서 {}개", self.entries.len()))
                                .small()
                                .color(MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let enabled =
                            self.selected.is_some() && self.pending == 0 && self.loading.is_empty();
                        let response =
                            ui.add_enabled(enabled, primary_button(Lucide::Save, "현재 문서 저장"));
                        if response.clicked() {
                            self.request_save(ctx);
                        }
                    });
                });
            });
    }

    fn file_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut open = None;
        let mut draft = None;
        let mut refresh = false;
        let mut pick_folder = false;
        let mut reset = false;
        let mut close = None;
        let mut delete = None;
        egui::Panel::left("files")
            .exact_size(246.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(246, 248, 248))
                    .inner_margin(12),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("그림대화").strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        refresh = icon_button(ui, Lucide::RefreshCw, "폴더 새로고침").clicked();
                        pick_folder =
                            icon_button(ui, Lucide::FolderOpen, "다른 폴더 보기").clicked();
                        reset = icon_button(ui, Lucide::RotateCcw, "게임 그림대화 폴더로 돌아가기")
                            .clicked();
                    });
                });
                ui.add(
                    egui::Label::new(
                        RichText::new(self.browser_dir.display().to_string())
                            .small()
                            .color(MUTED),
                    )
                    .truncate(),
                )
                .on_hover_text(self.browser_dir.display().to_string());
                ui.add(
                    egui::TextEdit::singleline(&mut self.browser_filter)
                        .hint_text("파일 검색")
                        .desired_width(f32::INFINITY),
                );
                let has_drafts = self.entries.iter().any(|entry| {
                    !self
                        .browser_items
                        .iter()
                        .any(|item| item.path == entry.path)
                });
                if has_drafts {
                    egui::CollapsingHeader::new("작업 중 문서")
                        .default_open(true)
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("working-documents")
                                .max_height(120.0)
                                .show(ui, |ui| {
                                    for entry in &mut self.entries {
                                        if self
                                            .browser_items
                                            .iter()
                                            .any(|item| item.path == entry.path)
                                        {
                                            continue;
                                        }
                                        ui.horizontal(|ui| {
                                            if ui
                                                .add_sized(
                                                    [(ui.available_width() - 42.0).max(80.0), 32.0],
                                                    egui::Button::new(
                                                        RichText::new(display_name(&entry.path))
                                                            .size(12.0),
                                                    )
                                                    .selected(Some(entry.id) == self.selected)
                                                    .truncate(),
                                                )
                                                .on_hover_text(display_name(&entry.path))
                                                .clicked()
                                            {
                                                draft = Some(entry.id);
                                            }
                                            if ui
                                                .add_enabled(
                                                    self.pending == 0,
                                                    egui::Button::image(
                                                        Lucide::X.size(14.0).color(MUTED).image(),
                                                    ),
                                                )
                                                .on_hover_text("문서 닫기 · 디스크 파일 유지")
                                                .clicked()
                                            {
                                                close = Some(entry.id);
                                            }
                                        });
                                    }
                                });
                        });
                }
                ui.separator();
                let filter = self.browser_filter.to_lowercase();
                let current_path = self
                    .entries
                    .iter()
                    .find(|entry| Some(entry.id) == self.selected)
                    .map(|entry| entry.path.clone());
                egui::ScrollArea::vertical()
                    .id_salt("folder-scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for item in &mut self.browser_items {
                            if !display_name(&item.path).to_lowercase().contains(&filter) {
                                continue;
                            }
                            let loaded = self.entries.iter().find(|entry| entry.path == item.path);
                            let selected = current_path.as_ref() == Some(&item.path)
                                || self.activate_after_load.as_ref() == Some(&item.path);
                            let edited = loaded.is_some_and(Entry::has_unsaved);
                            egui::Frame::new()
                                .fill(if selected {
                                    Color32::from_rgb(225, 239, 232)
                                } else {
                                    Color32::TRANSPARENT
                                })
                                .corner_radius(4)
                                .inner_margin(6)
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        let content = ui
                                            .vertical(|ui| {
                                                ui.set_width(162.0);
                                                if let Some(texture) = &item.texture {
                                                    ui.add_sized(
                                                        [162.0, 52.0],
                                                        egui::Image::new((
                                                            texture.id(),
                                                            texture.size_vec2(),
                                                        ))
                                                        .max_size(Vec2::new(112.0, 52.0)),
                                                    );
                                                } else {
                                                    let (rect, _) = ui.allocate_exact_size(
                                                        Vec2::new(162.0, 52.0),
                                                        egui::Sense::hover(),
                                                    );
                                                    ui.painter().rect_filled(
                                                        rect,
                                                        3.0,
                                                        Color32::from_gray(233),
                                                    );
                                                }
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(display_name(&item.path))
                                                            .size(12.0),
                                                    )
                                                    .truncate(),
                                                );
                                                if edited {
                                                    ui.label(
                                                        RichText::new("편집 중")
                                                            .size(11.0)
                                                            .color(GREEN),
                                                    );
                                                }
                                                if let Some(error) = &item.error {
                                                    ui.label(
                                                        RichText::new("읽기 실패")
                                                            .small()
                                                            .color(RED),
                                                    )
                                                    .on_hover_text(error);
                                                }
                                            })
                                            .response;
                                        let response = ui
                                            .interact(
                                                content.rect,
                                                egui::Id::new(("browse-file", &item.path)),
                                                egui::Sense::click(),
                                            )
                                            .on_hover_text(item.path.display().to_string());
                                        if response.clicked() {
                                            open = Some(item.path.clone());
                                        }
                                        response.context_menu(|ui| {
                                            if ui
                                                .add_enabled(
                                                    self.pending == 0,
                                                    egui::Button::image_and_text(
                                                        Lucide::Trash2
                                                            .size(16.0)
                                                            .color(RED)
                                                            .image(),
                                                        "파일 삭제…",
                                                    ),
                                                )
                                                .clicked()
                                            {
                                                delete = Some(item.path.clone());
                                                ui.close();
                                            }
                                        });
                                    });
                                });
                            ui.add_space(4.0);
                        }
                        if self.browser_busy {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("폴더를 읽고 있습니다");
                            });
                        } else if self.browser_items.is_empty() {
                            ui.label(RichText::new("PNG 파일이 없습니다.").small().color(MUTED));
                        }
                    });
            });
        if let Some(path) = open {
            self.open_browser_item(&ctx, path);
        }
        if let Some(id) = draft {
            self.activate_entry(id);
        }
        if let Some(id) = close {
            self.request_close_entry(&ctx, id);
        }
        if let Some(path) = delete {
            self.remove_request = Some(RemoveRequest::Delete(path));
        }
        if pick_folder
            && let Some(folder) = rfd::FileDialog::new()
                .set_title("그림대화 폴더 선택")
                .pick_folder()
        {
            self.set_browser_directory(folder);
            refresh = true;
        }
        if reset {
            self.set_browser_directory(default_output_dir().unwrap_or_else(|| PathBuf::from(".")));
            refresh = true;
        }
        if refresh {
            self.refresh_browser(&ctx);
        }
    }

    fn settings_panel(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        egui::Panel::right("settings")
            .exact_size(298.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(Color32::WHITE).inner_margin(18))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("settings-scroll")
                    .show(ui, |ui| {
                        section(ui, Lucide::UserRound, "작성자");
                        let response = ui.add_enabled(self.pending == 0,
                            egui::TextEdit::singleline(&mut self.author)
                                .hint_text("새 그림대화의 닉네임")
                                .background_color(Color32::from_rgb(242, 245, 244))
                                .margin(Vec2::new(8.0, 8.0))
                                .desired_width(f32::INFINITY),
                        );
                        if response.changed() { self.update_author(); }
                        if response.lost_focus() && let Some(entry) = self.entries.iter_mut().find(|e| Some(e.id) == self.selected) { entry.author_edit = false; }
                        if self.author_focus {
                            response.request_focus();
                            self.author_focus = false;
                        }
                        if !self.author.is_empty() {
                            ui.label(
                                RichText::new(format!(
                                    "authid  {}",
                                    engine::author_to_authid(&self.author)
                                ))
                                .small()
                                .color(MUTED),
                            );
                        }
                        if command(ui, Lucide::FileInput, "PNG에서 작성자 가져오기", false)
                            .clicked()
                        {
                            self.import_author();
                        }
                        ui.add_space(14.0);
                        section(ui, Lucide::Palette, "팔레트");
                        self.palette_editor(ui);
                        if ui.add_enabled(self.pending == 0, egui::Checkbox::new(&mut self.fix_palette, "팔레트 자동 보정")).on_hover_text("#FFFFFF ↔ #FEFEFE\n#C0C0C0 ↔ #C1C1C1\n#808080 ↔ #818181\n#000000 ↔ #010101").changed() {
                            if let Err(error) = self.apply_auto_palette(ui.ctx()) { self.message(error, true); }
                            changed = true;
                        }
                        ui.add_space(14.0);
                        section(ui, Lucide::FolderOutput, "저장 위치");
                        let label = self
                            .output_dir
                            .as_ref()
                            .map(|path| path.display().to_string())
                            .unwrap_or_else(|| "원본 폴더의 하위 폴더".into());
                        ui.add(egui::Label::new(RichText::new(&label).small().color(MUTED)).wrap())
                            .on_hover_text(&label);
                        ui.horizontal(|ui| {
                            if command(ui, Lucide::FolderOpen, "폴더 선택", false).clicked()
                                && let Some(folder) = rfd::FileDialog::new()
                                    .set_title("저장 폴더 선택")
                                    .pick_folder()
                            {
                                self.output_dir = Some(folder);
                            }
                            if icon_button(ui, Lucide::RotateCcw, "기본 저장 위치로 되돌리기")
                                .clicked()
                            {
                                self.output_dir = default_output_dir();
                            }
                        });
                    });
            });
        if changed {
            self.invalidate();
        }
    }

    fn finish_strokes(&mut self, ctx: &egui::Context) {
        let mut changed = false;
        let mut errors = Vec::new();
        for entry in &mut self.entries {
            if let Some(painting) = &mut entry.painting {
                painting.gesture = None;
                if !painting.active {
                    continue;
                }
                painting.active = false;
                let stroke_changed = painting.canvas.end_stroke() || painting.pending_changes;
                painting.pending_changes = false;
                if stroke_changed {
                    match sync_paint(entry, ctx) {
                        Ok(()) => changed = true,
                        Err(error) => errors.push(format!("그림을 적용하지 못했습니다: {error:#}")),
                    }
                }
            }
        }
        if changed {
            self.invalidate();
        }
        for error in errors {
            self.message(error, true);
        }
    }

    fn paint_history(&mut self, ctx: &egui::Context, redo: bool) {
        if self.pending > 0 {
            return;
        }
        self.finish_strokes(ctx);
        let mut changed = false;
        let mut error = None;
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| Some(entry.id) == self.selected)
        {
            let state = if redo {
                entry.redo.pop()
            } else {
                entry.undo.pop()
            };
            if let Some(state) = state {
                let current = entry.snapshot();
                match if state.doc.is_chat() {
                    make_painting(ctx, state.doc.clone()).map(Some)
                } else {
                    Ok(None)
                } {
                    Ok(mut painting) => {
                        if let (Some(restored), Some(previous)) = (&mut painting, &entry.painting) {
                            restored
                                .initial_palette
                                .clone_from(&previous.initial_palette);
                        }
                        if redo {
                            entry.undo.push(current);
                        } else {
                            entry.redo.push(current);
                        }
                        entry.painting = painting;
                        entry.author_edit = false;
                        if let Some(painting) = &mut entry.painting {
                            painting.unsaved = true;
                        } else {
                            entry.doc = state.doc;
                        }
                        entry.author_override = state.author;
                        entry.auto_palette = state.auto_palette;
                        self.fix_palette = state.auto_palette;
                        self.author = entry
                            .author_override
                            .clone()
                            .or_else(|| entry.effective_doc().author.clone())
                            .unwrap_or_default();
                        entry.draft = true;
                        entry.output = None;
                        changed = true;
                    }
                    Err(problem) => {
                        if redo {
                            entry.redo.push(state);
                        } else {
                            entry.undo.push(state);
                        }
                        error = Some(problem.to_string());
                    }
                }
            }
        }
        if changed {
            self.invalidate();
        }
        if let Some(error) = error {
            self.message(error, true);
        }
    }

    fn ensure_painting(&mut self, ctx: &egui::Context) -> Result<(), String> {
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| Some(entry.id) == self.selected)
            .ok_or("파일을 선택해 주시기 바랍니다.")?;
        if entry.painting.is_none() {
            entry.painting =
                Some(make_painting(ctx, entry.doc.clone()).map_err(|error| format!("{error:#}"))?);
        }
        Ok(())
    }

    fn edit_preview(&mut self, ctx: &egui::Context) -> Result<(), String> {
        if self.pending > 0 {
            return Err("저장이 완료된 후 변환 결과를 편집해 주시기 바랍니다.".into());
        }
        self.finish_strokes(ctx);
        let preview = self
            .preview
            .take()
            .ok_or("먼저 작성자 이름을 입력해 변환 결과를 생성해 주시기 바랍니다.")?;
        let painting =
            make_painting(ctx, Arc::new(preview.doc)).map_err(|error| format!("{error:#}"))?;
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| Some(entry.id) == self.selected)
        {
            entry.painting = Some(painting);
            entry.draft = true;
            entry.generated = true;
            entry.status = "변환됨".into();
            self.view = 3;
            self.paint_color = entry.effective_doc().palette.len().saturating_sub(1) as u8;
            self.invalidate();
        }
        Ok(())
    }

    fn set_palette_color(
        &mut self,
        ctx: &egui::Context,
        index: usize,
        rgb: [u8; 3],
    ) -> Result<(), String> {
        if self.pending > 0 {
            return Err("저장이 완료된 후 색상을 변경해 주시기 바랍니다.".into());
        }
        self.finish_strokes(ctx);
        self.ensure_painting(ctx)?;
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| Some(entry.id) == self.selected)
            .unwrap();
        if entry.effective_doc().palette.get(index) == Some(&rgb) {
            return Ok(());
        }
        entry.checkpoint();
        let painting = entry.painting.as_mut().unwrap();
        let mut palette = painting.doc.palette.clone();
        let color = palette
            .get_mut(index)
            .ok_or("팔레트 색상 번호가 올바르지 않습니다.")?;
        if *color == rgb {
            return Ok(());
        }
        *color = rgb;
        let bytes = engine::replace_palette(&painting.doc, &palette)
            .map_err(|error| format!("{error:#}"))?;
        let doc = Document::from_bytes(bytes).map_err(|error| format!("{error:#}"))?;
        painting.colors = engine::palette_rgba(&doc);
        painting.palette_hex = doc.palette.iter().map(|&rgb| hex_color(rgb)).collect();
        painting.texture = texture(ctx, "palette-edited", &doc.image);
        painting.tiles.clear();
        painting.doc = Arc::new(doc);
        painting.unsaved = true;
        entry.status = "팔레트 편집됨".into();
        entry.output = None;
        self.invalidate();
        Ok(())
    }

    fn apply_auto_palette(&mut self, ctx: &egui::Context) -> Result<(), String> {
        if self.pending > 0 {
            return Err("저장이 완료된 후 변경해 주시기 바랍니다.".into());
        }
        self.finish_strokes(ctx);
        let enabled = self.fix_palette;
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| Some(e.id) == self.selected)
        else {
            return Ok(());
        };
        entry.checkpoint();
        entry.auto_palette = enabled;
        if !entry.effective_doc().is_chat() {
            return Ok(());
        }
        if entry.painting.is_none() {
            entry.painting =
                Some(make_painting(ctx, entry.doc.clone()).map_err(|e| e.to_string())?);
        }
        let painting = entry.painting.as_mut().unwrap();
        let colors: Vec<_> = painting
            .doc
            .palette
            .iter()
            .map(|&rgb| {
                let from = if enabled {
                    RAW_PALETTE
                } else {
                    engine::DEFAULT_CHAT_PALETTE
                };
                let to = if enabled {
                    engine::DEFAULT_CHAT_PALETTE
                } else {
                    RAW_PALETTE
                };
                from.iter().position(|&c| c == rgb).map_or(rgb, |i| to[i])
            })
            .collect();
        let bytes = engine::replace_palette(&painting.doc, &colors).map_err(|e| e.to_string())?;
        let doc = Document::from_bytes(bytes).map_err(|e| e.to_string())?;
        painting.colors = engine::palette_rgba(&doc);
        painting.palette_hex = colors.iter().map(|&c| hex_color(c)).collect();
        painting.texture = texture(ctx, "auto-palette", &doc.image);
        painting.doc = Arc::new(doc);
        painting.tiles.clear();
        painting.unsaved = true;
        entry.output = None;
        self.invalidate();
        Ok(())
    }

    fn copy_selection(&mut self, ctx: &egui::Context) {
        self.finish_strokes(ctx);
        if let Some(p) = self.selected_entry().and_then(|e| e.painting.as_ref())
            && let Some(region) = p.selection
        {
            self.fragment = Some(raster::copy(&p.canvas, region, p.colors.clone()));
        }
    }

    fn request_selection_paste(&mut self, ctx: &egui::Context) {
        if self.pending > 0 {
            return;
        }
        self.finish_strokes(ctx);
        if self.ensure_painting(ctx).is_err() {
            return;
        }
        let Some(fragment) = &self.fragment else {
            return;
        };
        let p = self.selected_entry().unwrap().painting.as_ref().unwrap();
        if fragment.palette == p.colors {
            self.paste_selection(ctx);
            return;
        }
        let result = (|| -> anyhow::Result<Document> {
            let mut canvas = Canvas::new(p.canvas.width, p.canvas.height, p.canvas.pixels.clone())?;
            let at = p.selection.map_or((0, 0), |r| (r.x, r.y));
            raster::paste(&mut canvas, fragment, at, &p.colors);
            Document::from_bytes(engine::paint_indexed(&p.doc, &canvas.pixels)?)
        })();
        match result {
            Ok(doc) => self.paste_preview = Some(texture(ctx, "paste-preview", &doc.image)),
            Err(error) => self.message(error.to_string(), true),
        }
    }

    fn selection_paste_modal(&mut self, ctx: &egui::Context) {
        let Some(texture) = &self.paste_preview else {
            return;
        };
        let mut apply = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::new("selection-paste")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.heading("대상 팔레트로 변환한 결과");
            preview_surface(ui, "붙여넣기", Some(texture), 0.0, 240.0, false, None);
            ui.horizontal(|ui| {
                apply = command(ui, Lucide::Check, "붙여넣기", true).clicked();
                cancel = command(ui, Lucide::X, "취소", false).clicked();
            });
        });
        if apply || cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.paste_preview = None;
        }
        if apply {
            self.paste_selection(ctx);
        }
    }

    fn paste_selection(&mut self, ctx: &egui::Context) {
        if self.pending > 0 {
            return;
        }
        self.finish_strokes(ctx);
        let Some(fragment) = self.fragment.clone() else {
            return;
        };
        if self.ensure_painting(ctx).is_err() {
            return;
        }
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| Some(e.id) == self.selected)
        {
            let p = entry.painting.as_mut().unwrap();
            let at = p.selection.map_or((0, 0), |r| (r.x, r.y));
            raster::paste(&mut p.canvas, &fragment, at, &p.colors);
            p.selection = Some(Region::between(
                at,
                (
                    at.0 + fragment.region.width as i32 - 1,
                    at.1 + fragment.region.height as i32 - 1,
                ),
                p.canvas.width,
                p.canvas.height,
            ));
            if let Err(e) = sync_paint(entry, ctx) {
                self.message(e.to_string(), true);
            }
        }
        self.tool = Tool::Select;
        self.invalidate();
    }

    fn delete_selection(&mut self, ctx: &egui::Context) {
        if self.pending > 0 {
            return;
        }
        self.finish_strokes(ctx);
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| Some(e.id) == self.selected)
            && let Some(p) = &mut entry.painting
            && let Some(region) = p.selection
        {
            raster::clear(&mut p.canvas, region, 0);
            if let Err(e) = sync_paint(entry, ctx) {
                self.message(e.to_string(), true);
            }
        }
        self.invalidate();
    }

    fn palette_editor(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let Some(entry) = self.selected_entry() else {
            for rgb in if self.fix_palette {
                engine::DEFAULT_CHAT_PALETTE
            } else {
                RAW_PALETTE
            } {
                ui.horizontal(|ui| {
                    swatch(ui, rgb);
                });
            }
            return;
        };
        if !entry.effective_doc().is_chat() {
            let defaults = if self.fix_palette {
                engine::DEFAULT_CHAT_PALETTE
            } else {
                RAW_PALETTE
            };
            let colors = self
                .preview
                .as_ref()
                .map(|preview| preview.doc.palette.as_slice())
                .unwrap_or(&defaults);
            for (index, &default) in defaults.iter().enumerate() {
                let rgb = colors.get(index).copied().unwrap_or(default);
                ui.horizontal(|ui| {
                    swatch(ui, rgb);
                });
            }
            if self.preview.is_some()
                && command(ui, Lucide::Paintbrush, "변환 결과 편집", false).clicked()
                && let Err(error) = self.edit_preview(&ctx)
            {
                self.message(error, true);
            }
            return;
        }
        if let Err(error) = self.ensure_painting(&ctx) {
            ui.colored_label(RED, error);
            return;
        }
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| Some(entry.id) == self.selected)
            .unwrap();
        let painting = entry.painting.as_mut().unwrap();
        let mut changed = None;
        ui.add_enabled_ui(self.pending == 0, |ui| {
            for index in 0..painting.doc.palette.len() {
                ui.push_id(("palette-editor", entry.id, index), |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new((index + 1).to_string()).small().color(MUTED));
                        let mut rgb = painting.doc.palette[index];
                        if ui.color_edit_button_srgb(&mut rgb).changed() {
                            changed = Some((index, rgb));
                        }
                        let valid = parse_hex_color(&painting.palette_hex[index]);
                        let response = ui.add_sized(
                            [98.0, 32.0],
                            egui::TextEdit::singleline(&mut painting.palette_hex[index])
                                .desired_width(98.0)
                                .background_color(Color32::from_rgb(242, 245, 244))
                                .char_limit(7)
                                .text_color(if valid.is_some() {
                                    Color32::from_gray(55)
                                } else {
                                    RED
                                }),
                        );
                        if response.changed()
                            && let Some(rgb) = parse_hex_color(&painting.palette_hex[index])
                        {
                            changed = Some((index, rgb));
                        }
                        if response.lost_focus()
                            && parse_hex_color(&painting.palette_hex[index]).is_none()
                        {
                            painting.palette_hex[index] = hex_color(painting.doc.palette[index]);
                        }
                        if icon_button(ui, Lucide::RotateCcw, "이 색상을 처음 색상으로 되돌리기")
                            .clicked()
                        {
                            changed = Some((index, painting.initial_palette[index]));
                        }
                    });
                });
            }
        });
        if let Some((index, rgb)) = changed
            && let Err(error) = self.set_palette_color(&ctx, index, rgb)
        {
            self.message(error, true);
        }
        ui.add_space(8.0);
    }

    fn drawing(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, height: f32) {
        let Some(index) = self
            .entries
            .iter()
            .position(|entry| Some(entry.id) == self.selected)
        else {
            return;
        };
        if !self.entries[index].effective_doc().is_chat() {
            ui.add_space(16.0);
            ui.label("그리기는 기존 그림대화 PNG에서 사용할 수 있습니다.");
            if self.preview.is_some()
                && command(ui, Lucide::Paintbrush, "변환 결과 편집", true).clicked()
                && let Err(error) = self.edit_preview(ctx)
            {
                self.message(error, true);
            }
            return;
        }
        if self.pending > 0 {
            preview_surface(
                ui,
                "저장 중",
                Some(
                    self.entries[index]
                        .painting
                        .as_ref()
                        .map_or(&self.entries[index].texture, |painting| &painting.texture),
                ),
                self.zoom,
                height,
                false,
                None,
            );
            return;
        }
        if let Err(error) = self.ensure_painting(ctx) {
            ui.colored_label(RED, error);
            return;
        }
        let can_undo = !self.entries[index].undo.is_empty();
        let can_redo = !self.entries[index].redo.is_empty();
        let mut history = None;
        ui.horizontal_wrapped(|ui| {
            for (tool, icon, title) in [
                (Tool::Brush, Lucide::Paintbrush, "브러시"),
                (
                    Tool::Select,
                    Lucide::SquareDashed,
                    "사각형 선택 · 드래그하여 이동",
                ),
                (Tool::Fill, Lucide::PaintBucket, "영역 채우기"),
                (Tool::Line, Lucide::Minus, "직선"),
                (Tool::Rectangle, Lucide::RectangleHorizontal, "사각형"),
                (Tool::Ellipse, Lucide::Circle, "타원"),
            ] {
                if ui
                    .add(
                        egui::Button::image(
                            icon.size(17.0)
                                .color(if self.tool == tool { GREEN } else { MUTED })
                                .image(),
                        )
                        .selected(self.tool == tool),
                    )
                    .on_hover_text(title)
                    .clicked()
                {
                    self.finish_strokes(ctx);
                    self.tool = tool;
                }
            }
        });
        let mut copy = false;
        let mut paste = false;
        let mut delete = false;
        ui.horizontal_wrapped(|ui| {
            if self.tool == Tool::Select {
                copy = icon_button(ui, Lucide::Copy, "선택 영역 복사 · Ctrl+C").clicked();
                paste = ui
                    .add_enabled(
                        self.fragment.is_some(),
                        egui::Button::image(Lucide::ClipboardPaste.size(17.0).color(MUTED).image()),
                    )
                    .on_hover_text("선택 영역 붙여넣기 · Ctrl+V")
                    .clicked();
                delete = icon_button(ui, Lucide::Trash2, "선택 영역 비우기 · Delete").clicked();
            }
            if self.tool == Tool::Fill {
                ui.checkbox(&mut self.fill_global, "이미지 전체의 같은 색");
            }
            if matches!(self.tool, Tool::Rectangle | Tool::Ellipse) {
                ui.checkbox(&mut self.shape_filled, "도형 내부 채우기");
            }
            for (shape, icon, title) in [
                (Shape::Circle, Lucide::Circle, "원형 브러시"),
                (Shape::Square, Lucide::Square, "사각형 브러시"),
                (Shape::Diamond, Lucide::Diamond, "마름모 브러시"),
            ] {
                if ui
                    .add(
                        egui::Button::image(
                            icon.size(17.0)
                                .color(if self.brush_shape == shape {
                                    GREEN
                                } else {
                                    MUTED
                                })
                                .image(),
                        )
                        .selected(self.brush_shape == shape),
                    )
                    .on_hover_text(title)
                    .clicked()
                {
                    self.brush_shape = shape;
                }
            }
            ui.separator();
            if ui
                .add_enabled(
                    can_undo,
                    egui::Button::image(Lucide::Undo2.size(17.0).color(MUTED).image()),
                )
                .on_hover_text("실행 취소")
                .clicked()
            {
                history = Some(false);
            }
            if ui
                .add_enabled(
                    can_redo,
                    egui::Button::image(Lucide::Redo2.size(17.0).color(MUTED).image()),
                )
                .on_hover_text("다시 실행")
                .clicked()
            {
                history = Some(true);
            }
        });
        if let Some(redo) = history {
            self.paint_history(ctx, redo);
        }
        if copy {
            self.copy_selection(ctx);
        }
        if paste {
            self.request_selection_paste(ctx);
        }
        if delete {
            self.delete_selection(ctx);
        }
        let entry = &mut self.entries[index];
        let painting = entry.painting.as_mut().unwrap();
        self.paint_color = self.paint_color.min((painting.colors.len() - 1) as u8);
        ui.horizontal(|ui| {
            for (index, &color) in painting.colors.iter().enumerate() {
                let button = egui::Button::new("")
                    .fill(Color32::from_rgba_unmultiplied(
                        color[0], color[1], color[2], color[3],
                    ))
                    .min_size(Vec2::splat(28.0))
                    .stroke(egui::Stroke::new(
                        if self.paint_color as usize == index {
                            3.0
                        } else {
                            1.0
                        },
                        if self.paint_color as usize == index {
                            GREEN
                        } else {
                            Color32::from_gray(185)
                        },
                    ));
                if ui
                    .add(button)
                    .on_hover_text(format!(
                        "색상 {} · #{:02X}{:02X}{:02X} · α {}",
                        index + 1,
                        color[0],
                        color[1],
                        color[2],
                        color[3]
                    ))
                    .clicked()
                {
                    self.paint_color = index as u8;
                }
            }
            ui.add_space(8.0);
            ui.spacing_mut().slider_width = 90.0;
            ui.add(egui::Slider::new(&mut self.brush_size, 1..=64).suffix(" px"));
            if ui
                .add(
                    egui::Button::image(Lucide::Grid2x2.size(17.0).color(MUTED).image())
                        .selected(self.pixel_grid),
                )
                .on_hover_text("픽셀 격자")
                .clicked()
            {
                self.pixel_grid = !self.pixel_grid;
            }
        });
        let available_width = ui.available_width();
        let size = Vec2::new(painting.canvas.width as f32, painting.canvas.height as f32);
        let height = height.max(210.0);
        let scale = if self.zoom == 0.0 {
            ((available_width - 24.0) / size.x)
                .min((height - 24.0) / size.y)
                .min(6.0)
        } else {
            self.zoom
        };
        let scaled = size * scale;
        canvas_scroll("paint-canvas", Vec2::new(available_width, height)).show(ui, |ui| {
            let (canvas, response) = ui.allocate_exact_size(
                Vec2::new(
                    available_width.max(scaled.x + 24.0),
                    height.max(scaled.y + 24.0),
                ),
                egui::Sense::drag(),
            );
            let rect = egui::Rect::from_center_size(canvas.center(), scaled);
            painting.viewport = Some((rect, scale));
            let painter = ui.painter_at(canvas);
            checkerboard(&painter, canvas);
            if !self.confirm_discard
                && !self.show_new
                && self.save_dialog.is_none()
                && self.paste_preview.is_none()
                && self.frame_dialog.is_none()
                && self.remove_request.is_none()
                && self.batch_dialog.is_none()
                && !egui::Popup::is_any_open(ctx)
                && if self.tool == Tool::Brush {
                    paint_events(
                        painting,
                        &ctx.input(|input| input.events.clone()),
                        rect,
                        rect.intersect(painter.clip_rect()),
                        (self.brush_size, self.brush_shape, self.paint_color),
                    )
                } else {
                    tool_events(
                        painting,
                        &ctx.input(|input| input.events.clone()),
                        rect,
                        rect.intersect(painter.clip_rect()),
                        self.tool,
                        (
                            self.paint_color,
                            self.shape_filled,
                            self.brush_size,
                            self.fill_global,
                        ),
                    )
                }
            {
                painting.tiles.clear();
                ctx.request_repaint();
            }
            paint_tiles(ctx, &painter, rect, scale, painting);
            if self.tool == Tool::Select
                && let Some(r) = painting.selection
            {
                let selection = egui::Rect::from_min_size(
                    rect.min + Vec2::new(r.x as f32, r.y as f32) * scale,
                    Vec2::new(r.width as f32, r.height as f32) * scale,
                );
                painter.rect_stroke(
                    selection,
                    0.0,
                    egui::Stroke::new(2.0, Color32::WHITE),
                    egui::StrokeKind::Inside,
                );
                painter.rect_stroke(
                    selection,
                    0.0,
                    egui::Stroke::new(1.0, GREEN),
                    egui::StrokeKind::Inside,
                );
            }
            let pointer = ctx.input(|input| input.pointer.interact_pos());
            if let Some(pos) = pointer {
                let x = ((pos.x - rect.left()) / scale).floor() as i32;
                let y = ((pos.y - rect.top()) / scale).floor() as i32;
                if self.tool == Tool::Brush && rect.contains(pos) && response.hovered() {
                    ui.output_mut(|output| output.cursor_icon = egui::CursorIcon::Crosshair);
                    let anchor = rect.min
                        + Vec2::new(
                            (x - self.brush_size as i32 / 2) as f32,
                            (y - self.brush_size as i32 / 2) as f32,
                        ) * scale;
                    let cursor = egui::Rect::from_min_size(
                        anchor,
                        Vec2::splat(self.brush_size as f32 * scale),
                    );
                    let stroke = egui::Stroke::new(1.5, Color32::from_rgb(225, 66, 94));
                    match self.brush_shape {
                        Shape::Circle => {
                            painter.circle_stroke(cursor.center(), cursor.width() / 2.0, stroke);
                        }
                        Shape::Square => {
                            painter.rect_stroke(cursor, 0.0, stroke, egui::StrokeKind::Inside);
                        }
                        Shape::Diamond => {
                            painter.add(egui::Shape::closed_line(
                                vec![
                                    cursor.center_top(),
                                    cursor.right_center(),
                                    cursor.center_bottom(),
                                    cursor.left_center(),
                                ],
                                stroke,
                            ));
                        }
                    }
                }
            }
            if self.pixel_grid && scale >= 4.0 {
                let clip = rect.intersect(painter.clip_rect());
                let stroke = egui::Stroke::new(0.5, Color32::from_black_alpha(40));
                for x in ((clip.left() - rect.left()) / scale).ceil() as i32
                    ..=((clip.right() - rect.left()) / scale).floor() as i32
                {
                    let x = rect.left() + x as f32 * scale;
                    painter.line_segment(
                        [egui::pos2(x, clip.top()), egui::pos2(x, clip.bottom())],
                        stroke,
                    );
                }
                for y in ((clip.top() - rect.top()) / scale).ceil() as i32
                    ..=((clip.bottom() - rect.top()) / scale).floor() as i32
                {
                    let y = rect.top() + y as f32 * scale;
                    painter.line_segment(
                        [egui::pos2(clip.left(), y), egui::pos2(clip.right(), y)],
                        stroke,
                    );
                }
            }
        });
    }

    fn discard_modal(&mut self, ctx: &egui::Context) {
        if !self.confirm_discard {
            return;
        }
        let mut discard = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::new("discard-paint")).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.heading("저장하지 않은 그림");
            ui.label("새 문서 또는 편집한 내용이 저장되지 않았습니다.");
            ui.horizontal(|ui| {
                if command(ui, Lucide::ArrowLeft, "계속 편집", true).clicked() {
                    cancel = true;
                }
                if command(ui, Lucide::Trash2, "편집 내용 버리기", false).clicked() {
                    discard = true;
                }
            });
        });
        if cancel {
            self.confirm_discard = false;
        }
        if discard {
            self.confirm_discard = false;
            self.force_close = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn workspace(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(249, 250, 250))
                    .inner_margin(20),
            )
            .show(ui, |ui| {
                if self.entries.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.add(Lucide::ImagePlus.size(42.0).color(GREEN));
                            ui.add_space(12.0);
                            if command(ui, Lucide::FolderOpen, "PNG 파일 열기", true).clicked()
                            {
                                self.open_files(ctx);
                            }
                            if command(ui, Lucide::FilePlus, "새 그림대화", false).clicked() {
                                self.open_new_dialog();
                            }
                            if command(ui, Lucide::ClipboardPaste, "이미지 붙여넣기", false)
                                .clicked()
                            {
                                self.paste_image(ctx);
                            }
                        });
                    });
                    return;
                }
                ui.horizontal(|ui| {
                    ui.heading("미리보기");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_enabled_ui(self.pending == 0, |ui| {
                            if let Some(id) = self.selected {
                                if icon_button(ui, Lucide::X, "현재 문서 닫기 · 디스크 파일 유지")
                                    .clicked()
                                {
                                    self.request_close_entry(ctx, id);
                                }
                                let path = self
                                    .selected_entry()
                                    .filter(|e| !e.generated && e.path.is_file())
                                    .map(|e| e.path.clone());
                                if ui
                                    .add_enabled(
                                        path.is_some(),
                                        egui::Button::image(
                                            Lucide::Trash2.size(18.0).color(MUTED).image(),
                                        ),
                                    )
                                    .on_hover_text("현재 PNG 파일 삭제…")
                                    .clicked()
                                {
                                    self.remove_request = path.map(RemoveRequest::Delete);
                                }
                                if icon_button(ui, Lucide::Crop, "캔버스 크기 및 이미지 배치")
                                    .clicked()
                                {
                                    self.open_canvas_resize(ctx);
                                }
                            }
                        });
                        if icon_button(ui, Lucide::RefreshCw, "미리보기 다시 생성").clicked()
                        {
                            self.invalidate();
                        }
                        if ui
                            .add_enabled(
                                self.selected.is_some() && self.pending == 0,
                                egui::Button::image(Lucide::Save.size(18.0).color(GREEN).image()),
                            )
                            .on_hover_text("현재 파일 저장")
                            .clicked()
                        {
                            self.request_save(ctx);
                        }
                    });
                });
                let compact_toolbar = ui.available_width() < 580.0;
                ui.horizontal(|ui| {
                    for (id, label) in [(0, "비교"), (1, "원본"), (2, "결과"), (3, "그리기")]
                    {
                        ui.selectable_value(&mut self.view, id, label);
                    }
                    if !compact_toolbar {
                        zoom_controls(ui, &mut self.zoom);
                    }
                });
                if compact_toolbar {
                    ui.horizontal(|ui| {
                        zoom_controls(ui, &mut self.zoom);
                    });
                }
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .id_salt("workspace-scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let preview_height = (ui.available_height() - 275.0).clamp(190.0, 350.0);
                        if self.view == 3 {
                            self.drawing(ui, ctx, preview_height);
                            if !ctx.input(|input| input.pointer.primary_down()) {
                                self.finish_strokes(ctx);
                            }
                        } else if self.view == 0 {
                            let Some(entry) = self.selected_entry() else {
                                return;
                            };
                            ui.columns(2, |columns| {
                                preview_surface(
                                    &mut columns[0],
                                    "원본",
                                    Some(&entry.texture),
                                    self.zoom,
                                    preview_height,
                                    false,
                                    None,
                                );
                                preview_surface(
                                    &mut columns[1],
                                    "결과",
                                    self.preview.as_ref().map(|p| &p.texture),
                                    self.zoom,
                                    preview_height,
                                    self.preview_busy,
                                    self.preview_error.as_deref(),
                                );
                            });
                        } else {
                            let Some(entry) = self.selected_entry() else {
                                return;
                            };
                            let original = self.view == 1;
                            preview_surface(
                                ui,
                                if original { "원본" } else { "결과" },
                                if original {
                                    Some(&entry.texture)
                                } else {
                                    self.preview.as_ref().map(|p| &p.texture)
                                },
                                self.zoom,
                                preview_height,
                                !original && self.preview_busy,
                                if original {
                                    None
                                } else {
                                    self.preview_error.as_deref()
                                },
                            );
                        }
                        ui.add_space(18.0);
                        let Some(entry) = self.selected_entry() else {
                            return;
                        };
                        let doc = entry.effective_doc();
                        ui.horizontal(|ui| {
                            ui.add(Lucide::FileImage.size(17.0).color(MUTED));
                            ui.add(
                                egui::Label::new(RichText::new(display_name(&entry.path)).strong())
                                    .truncate(),
                            );
                        });
                        egui::Grid::new("metadata")
                            .num_columns(2)
                            .spacing([18.0, 7.0])
                            .show(ui, |ui| {
                                for (key, value) in [
                                    (
                                        "파일 형식",
                                        if doc.is_chat() {
                                            "그림대화 PNG".into()
                                        } else {
                                            "일반 PNG".into()
                                        },
                                    ),
                                    (
                                        "이미지",
                                        format!(
                                            "{} × {} · {}비트{}",
                                            doc.image.width(),
                                            doc.image.height(),
                                            doc.bit_depth,
                                            if doc.color_type == 3 {
                                                " 팔레트"
                                            } else {
                                                ""
                                            }
                                        ),
                                    ),
                                    (
                                        "작성자",
                                        entry
                                            .author_override
                                            .clone()
                                            .or_else(|| doc.author.clone())
                                            .unwrap_or_else(|| "없음".into()),
                                    ),
                                    (
                                        "authid",
                                        entry
                                            .author_override
                                            .as_ref()
                                            .map(|s| engine::author_to_authid(s).to_string())
                                            .or_else(|| doc.authid.clone())
                                            .unwrap_or_else(|| "없음".into()),
                                    ),
                                ] {
                                    ui.label(RichText::new(key).small().color(MUTED));
                                    ui.add(egui::Label::new(value).wrap());
                                    ui.end_row();
                                }
                                if let Some(preview) = &self.preview {
                                    ui.label(RichText::new("결과 작성자").small().color(GREEN));
                                    ui.add(
                                        egui::Label::new(format!(
                                            "{}  ·  {}",
                                            preview.doc.author.as_deref().unwrap_or("없음"),
                                            preview.doc.authid.as_deref().unwrap_or("없음")
                                        ))
                                        .wrap(),
                                    );
                                    ui.end_row();
                                }
                            });
                        if doc.is_chat() {
                            if let Ok(colors) = engine::used_palette_colors(doc) {
                                ui.label(
                                    RichText::new(format!("실제 사용 팔레트 · {colors}색"))
                                        .small()
                                        .color(MUTED),
                                );
                            }
                            if let Err(error) = engine::validate_game_size(doc) {
                                ui.colored_label(RED, error.to_string());
                            }
                        }
                        ui.add_space(12.0);
                        if !entry.doc.palette.is_empty() {
                            ui.label(
                                RichText::new(format!(
                                    "원본 팔레트 · {}색",
                                    entry.doc.palette.len()
                                ))
                                .small()
                                .color(MUTED),
                            );
                            palette(ui, &entry.doc.palette);
                        }
                        if let Some(preview) = &self.preview {
                            ui.label(
                                RichText::new(format!(
                                    "결과 팔레트 · {}색",
                                    preview.doc.palette.len()
                                ))
                                .small()
                                .color(GREEN),
                            );
                            palette(ui, &preview.doc.palette);
                            ui.label(
                                RichText::new(format!(
                                    "{} × {} · {} bytes",
                                    preview.doc.image.width(),
                                    preview.doc.image.height(),
                                    preview.doc.bytes.len()
                                ))
                                .small()
                                .color(MUTED),
                            );
                        }
                        if let Some(output) = &entry.output {
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.add(Lucide::CircleCheck.size(16.0).color(GREEN));
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(output.display().to_string())
                                            .small()
                                            .color(GREEN),
                                    )
                                    .wrap(),
                                );
                            });
                            if command(ui, Lucide::FolderOpen, "저장 폴더 열기", false).clicked()
                                && let Some(parent) = output.parent()
                            {
                                open_folder(parent);
                            }
                        }
                        ui.add_space(10.0);
                        egui::CollapsingHeader::new(format!("처리 기록 ({})", self.logs.len()))
                            .show(ui, |ui| {
                                egui::ScrollArea::vertical()
                                    .id_salt("logs")
                                    .max_height(140.0)
                                    .stick_to_bottom(true)
                                    .show(ui, |ui| {
                                        for log in &self.logs {
                                            ui.add(
                                                egui::Label::new(RichText::new(log).small()).wrap(),
                                            );
                                        }
                                    });
                            });
                    });
            });
    }

    fn capture_if_requested(&mut self, ctx: &egui::Context) {
        let Some(path) = &self.screenshot else {
            return;
        };
        for event in ctx.input(|input| input.events.clone()) {
            if let egui::Event::Screenshot { image, .. } = event {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let pixels: Vec<u8> = image
                    .pixels
                    .iter()
                    .flat_map(|pixel| pixel.to_array())
                    .collect();
                image::save_buffer(
                    path,
                    &pixels,
                    image.width() as u32,
                    image.height() as u32,
                    image::ColorType::Rgba8,
                )
                .expect("Screenshot save failed");
                self.force_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        if self.loading.is_empty() && !self.preview_busy && self.dirty_at.is_none() {
            self.ready_frames += 1;
            if self.ready_frames > 15 && !self.screenshot_started {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                self.screenshot_started = true;
            }
        }
        ctx.request_repaint_after(Duration::from_millis(60));
    }
}

impl eframe::App for Studio {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.receive(&ctx);
        self.open_waiting_clipboard(&ctx);
        if self.frame_when_ready && self.selected.is_some() {
            self.frame_when_ready = false;
            self.open_canvas_resize(&ctx);
        }
        self.clipboard_shortcut(&ctx);
        let modal_open = self.show_new
            || self.save_dialog.is_some()
            || self.confirm_discard
            || self.paste_preview.is_some()
            || self.frame_dialog.is_some()
            || self.clipboard_waiting.is_some()
            || self.remove_request.is_some()
            || self.batch_dialog.is_some();
        if !modal_open
            && ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::N))
        {
            self.open_new_dialog();
        }
        if self.view != 3 && !ctx.input(|input| input.pointer.primary_down()) {
            self.finish_strokes(&ctx);
        }
        let dropped = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect::<Vec<_>>()
        });
        if self.batch_dialog.is_some() && !dropped.is_empty() {
            self.add_batch_paths(dropped.clone());
        } else if !modal_open && !dropped.is_empty() {
            self.load_paths(&ctx, dropped);
        }
        if !modal_open
            && ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::O))
        {
            self.open_files(&ctx);
        }
        if !modal_open
            && ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::S))
        {
            self.request_save(&ctx);
        }
        if !modal_open && !ctx.egui_wants_keyboard_input() {
            if ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::Z)) {
                self.paint_history(&ctx, ctx.input(|input| input.modifiers.shift));
            } else if ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::Y))
            {
                self.paint_history(&ctx, true);
            }
            if self.view == 3 && self.tool == Tool::Select && self.pending == 0 {
                if ctx.input(|i| {
                    (i.modifiers.command && i.key_pressed(egui::Key::C))
                        || i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                }) {
                    self.copy_selection(&ctx);
                }
                if ctx.input(|i| i.key_pressed(egui::Key::Delete)) {
                    self.delete_selection(&ctx);
                }
            }
        }
        if !self.force_close && ctx.input(|input| input.viewport().close_requested()) {
            if self.pending > 0 || self.batch_dialog.as_ref().is_some_and(|d| d.running) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.message(
                    "파일을 저장하고 있습니다. 완료 후 창을 닫아 주시기 바랍니다.".into(),
                    false,
                );
            } else if self.entries.iter().any(Entry::has_unsaved)
                || self.frame_dialog.is_some()
                || self.clipboard_waiting.is_some()
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.finish_strokes(&ctx);
                self.confirm_discard = true;
            }
        }
        ui.add_enabled_ui(!modal_open, |ui| {
            self.header(ui, &ctx);
            self.footer(ui, &ctx);
            self.file_panel(ui);
            self.settings_panel(ui);
            self.workspace(ui, &ctx);
        });
        if self.confirm_discard {
            self.discard_modal(&ctx);
        } else {
            self.new_dialog(&ctx);
            self.save_modal(&ctx);
            self.selection_paste_modal(&ctx);
            self.framing_modal(&ctx);
            self.remove_modal(&ctx);
            self.batch_modal(&ctx);
        }
        if let Some(start) = self.dirty_at {
            if start.elapsed() >= Duration::from_millis(250) {
                self.start_preview(&ctx);
            } else {
                ctx.request_repaint_after(Duration::from_millis(80));
            }
        }
        if !ctx.input(|input| input.raw.hovered_files.is_empty()) {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("drop"),
            ));
            let rect = ctx.content_rect().shrink(8.0);
            painter.rect_filled(
                rect,
                6.0,
                Color32::from_rgba_unmultiplied(230, 245, 237, 235),
            );
            painter.rect_stroke(
                rect,
                6.0,
                egui::Stroke::new(2.0, GREEN),
                egui::StrokeKind::Inside,
            );
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "PNG 파일 추가",
                egui::FontId::proportional(24.0),
                GREEN,
            );
        }
        self.capture_if_requested(&ctx);
    }
}

#[path = "app_batch.rs"]
mod batch_ui;
#[path = "app_transform.rs"]
mod transform_ui;

fn make_painting(ctx: &egui::Context, doc: Arc<Document>) -> anyhow::Result<Painting> {
    let doc = Arc::new(engine::four_color_document(&doc)?);
    let pixels = engine::indexed_pixels(&doc)?;
    Ok(Painting {
        canvas: Canvas::new(doc.image.width(), doc.image.height(), pixels)?,
        colors: engine::palette_rgba(&doc),
        texture: texture(ctx, "paint-initial", &doc.image),
        palette_hex: doc.palette.iter().map(|&rgb| hex_color(rgb)).collect(),
        initial_palette: doc.palette.clone(),
        selection: None,
        gesture: None,
        doc,
        active: false,
        unsaved: false,
        pending_changes: false,
        viewport: None,
        tiles: HashMap::new(),
    })
}

fn hex_color(rgb: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}

fn parse_hex_color(value: &str) -> Option<[u8; 3]> {
    let value = value.trim().strip_prefix('#').unwrap_or(value.trim());
    if value.len() != 6 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some([
        u8::from_str_radix(&value[..2], 16).ok()?,
        u8::from_str_radix(&value[2..4], 16).ok()?,
        u8::from_str_radix(&value[4..], 16).ok()?,
    ])
}

fn native_paste_down(ctx: &egui::Context) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL};
        let _ = ctx;
        // egui-winit consumes Ctrl+V key-down even when no text is available.
        unsafe {
            (GetKeyState(VK_CONTROL as i32) as u16 & 0x8000) != 0
                && (GetKeyState(0x56) as u16 & 0x8000) != 0
        }
    }
    #[cfg(not(windows))]
    {
        ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::V))
    }
}

fn texture(ctx: &egui::Context, name: &str, image: &image::RgbaImage) -> egui::TextureHandle {
    let max_dimension = image.width().max(image.height());
    let image = if max_dimension > 1536 {
        let width = (u64::from(image.width()) * 1536 / u64::from(max_dimension)).max(1) as u32;
        let height = (u64::from(image.height()) * 1536 / u64::from(max_dimension)).max(1) as u32;
        image::imageops::resize(image, width, height, image::imageops::FilterType::Nearest)
    } else {
        image.clone()
    };
    let color = egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    );
    ctx.load_texture(name, color, egui::TextureOptions::NEAREST)
}

fn default_output_dir() -> Option<PathBuf> {
    dirs::document_dir().map(|directory| directory.join("마비노기").join("그림대화"))
}

fn zoom_controls(ui: &mut egui::Ui, zoom: &mut f32) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().slider_width = 78.0;
        if ui.small_button("맞춤").clicked() {
            *zoom = 0.0;
        }
        ui.add(
            egui::Slider::new(zoom, 0.0..=6.0)
                .step_by(0.5)
                .custom_formatter(|value, _| {
                    if value == 0.0 {
                        "맞춤".into()
                    } else {
                        format!("{value:.1}×")
                    }
                })
                .show_value(true)
                .smart_aim(false)
                .trailing_fill(true),
        );
    });
}

fn paint_events(
    painting: &mut Painting,
    events: &[egui::Event],
    rect: egui::Rect,
    clip: egui::Rect,
    brush: (u32, Shape, u8),
) -> bool {
    let scale = rect.width() / painting.canvas.width as f32;
    let point = |pos: egui::Pos2| {
        (
            ((pos.x - rect.left()) / scale).floor() as i32,
            ((pos.y - rect.top()) / scale).floor() as i32,
        )
    };
    let mut touched = false;
    let mut released = false;
    // Preserve event order: a complete press/move/release may arrive in one frame.
    for event in events {
        match *event {
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                ..
            } if clip.contains(pos) => {
                if painting.active {
                    painting.pending_changes |= painting.canvas.end_stroke();
                }
                painting.canvas.begin_stroke();
                painting.active = true;
                released = false;
                let (x, y) = point(pos);
                painting.canvas.stroke_to(x, y, brush.0, brush.1, brush.2);
                touched = true;
            }
            egui::Event::PointerMoved(pos) if painting.active && !released => {
                let (x, y) = point(pos);
                painting.canvas.stroke_to(x, y, brush.0, brush.1, brush.2);
                touched = true;
            }
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                ..
            } if painting.active => {
                let (x, y) = point(pos);
                painting.canvas.stroke_to(x, y, brush.0, brush.1, brush.2);
                released = true;
                touched = true;
            }
            _ => {}
        }
    }
    touched
}

fn tool_events(
    p: &mut Painting,
    events: &[egui::Event],
    rect: egui::Rect,
    clip: egui::Rect,
    tool: Tool,
    options: (u8, bool, u32, bool),
) -> bool {
    let scale = rect.width() / p.canvas.width as f32;
    let point = |pos: egui::Pos2| {
        (
            ((pos.x - rect.left()) / scale).floor() as i32,
            ((pos.y - rect.top()) / scale).floor() as i32,
        )
    };
    let mut touched = false;
    for event in events {
        if matches!(
            event,
            egui::Event::Key {
                key: egui::Key::Escape,
                pressed: true,
                ..
            }
        ) {
            if let Some(g) = p.gesture.take() {
                p.canvas.pixels = g.before;
            }
            p.selection = None;
            p.active = false;
            p.pending_changes = false;
            touched = true;
            continue;
        }
        let (pos, press, release) = match *event {
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                ..
            } => (pos, pressed, !pressed),
            egui::Event::PointerMoved(pos) => (pos, false, false),
            _ => continue,
        };
        let at = point(pos);
        if press && clip.contains(pos) {
            if tool == Tool::Fill {
                raster::fill(&mut p.canvas, at, options.0, options.3);
                p.active = true;
                p.pending_changes = true;
                touched = true;
                continue;
            }
            let moving = if tool == Tool::Select {
                p.selection
                    .filter(|r| r.contains(at))
                    .map(|r| raster::copy(&p.canvas, r, p.colors.clone()))
            } else {
                None
            };
            p.gesture = Some(Gesture {
                start: at,
                before: p.canvas.pixels.clone(),
                moving,
            });
        }
        if let Some(g) = &p.gesture {
            p.canvas.pixels.clone_from(&g.before);
            if tool == Tool::Select {
                if let Some(fragment) = &g.moving {
                    let x = (fragment.region.x + at.0 - g.start.0)
                        .clamp(0, (p.canvas.width - fragment.region.width) as i32);
                    let y = (fragment.region.y + at.1 - g.start.1)
                        .clamp(0, (p.canvas.height - fragment.region.height) as i32);
                    raster::clear(&mut p.canvas, fragment.region, 0);
                    raster::paste(&mut p.canvas, fragment, (x, y), &p.colors);
                    p.selection = Some(Region {
                        x,
                        y,
                        ..fragment.region
                    });
                    p.active = true;
                    p.pending_changes = true;
                } else {
                    p.selection = Some(Region::between(
                        g.start,
                        at,
                        p.canvas.width,
                        p.canvas.height,
                    ));
                }
            } else {
                raster::figure(
                    &mut p.canvas,
                    tool,
                    g.start,
                    at,
                    options.0,
                    options.1,
                    options.2,
                );
                p.active = true;
                p.pending_changes = true;
            }
            touched = true;
            if release {
                p.gesture = None;
            }
        }
    }
    touched
}

fn paint_tiles(
    ctx: &egui::Context,
    painter: &egui::Painter,
    rect: egui::Rect,
    scale: f32,
    painting: &mut Painting,
) {
    const TILE: u32 = 256;
    let clip = rect.intersect(painter.clip_rect());
    if clip.width() <= 0.0 || clip.height() <= 0.0 {
        return;
    }
    let start_x = (((clip.left() - rect.left()) / scale).floor().max(0.0) as u32 / TILE) * TILE;
    let start_y = (((clip.top() - rect.top()) / scale).floor().max(0.0) as u32 / TILE) * TILE;
    let end_x = (((clip.right() - rect.left()) / scale).ceil() as u32).min(painting.canvas.width);
    let end_y = (((clip.bottom() - rect.top()) / scale).ceil() as u32).min(painting.canvas.height);
    // Native-size tiles keep individual pixels visible at every zoom level.
    for y in (start_y..end_y).step_by(TILE as usize) {
        for x in (start_x..end_x).step_by(TILE as usize) {
            let width = TILE.min(painting.canvas.width - x);
            let height = TILE.min(painting.canvas.height - y);
            let handle = painting.tiles.entry((x, y)).or_insert_with(|| {
                let mut rgba = Vec::with_capacity((width * height * 4) as usize);
                for py in y..y + height {
                    for px in x..x + width {
                        let index = painting.canvas.pixels
                            [(py * painting.canvas.width + px) as usize]
                            as usize;
                        rgba.extend_from_slice(&painting.colors[index]);
                    }
                }
                ctx.load_texture(
                    format!("paint-tile-{x}-{y}"),
                    egui::ColorImage::from_rgba_unmultiplied(
                        [width as usize, height as usize],
                        &rgba,
                    ),
                    egui::TextureOptions::NEAREST,
                )
            });
            painter.image(
                handle.id(),
                egui::Rect::from_min_size(
                    rect.min + Vec2::new(x as f32, y as f32) * scale,
                    Vec2::new(width as f32, height as f32) * scale,
                ),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }
}

fn sync_paint(entry: &mut Entry, ctx: &egui::Context) -> anyhow::Result<()> {
    if let Some(p) = &entry.painting
        && engine::indexed_pixels(&p.doc)? == p.canvas.pixels
    {
        return Ok(());
    }
    entry.checkpoint();
    if let Some(painting) = &mut entry.painting {
        let bytes = engine::paint_indexed(&painting.doc, &painting.canvas.pixels)?;
        let doc = Document::from_bytes(bytes)?;
        painting.texture = texture(ctx, "paint-committed", &doc.image);
        painting.tiles.clear();
        painting.doc = Arc::new(doc);
        painting.unsaved = true;
        entry.status = "편집됨".into();
        entry.output = None;
    }
    Ok(())
}

fn checkerboard(painter: &egui::Painter, rect: egui::Rect) {
    let tile = 12.0;
    let clip = rect.intersect(painter.clip_rect());
    let first_x = ((clip.left() - rect.left()) / tile).floor().max(0.0) as i32;
    let first_y = ((clip.top() - rect.top()) / tile).floor().max(0.0) as i32;
    let last_x = ((clip.right() - rect.left()) / tile).ceil() as i32;
    let last_y = ((clip.bottom() - rect.top()) / tile).ceil() as i32;
    for y in first_y..last_y {
        for x in first_x..last_x {
            painter.rect_filled(
                egui::Rect::from_min_size(
                    rect.min + Vec2::new(x as f32 * tile, y as f32 * tile),
                    Vec2::splat(tile),
                ),
                0.0,
                if (x + y) % 2 == 0 {
                    Color32::from_gray(237)
                } else {
                    Color32::from_gray(248)
                },
            );
        }
    }
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn command(ui: &mut egui::Ui, icon: Lucide, text: &str, primary: bool) -> egui::Response {
    if primary {
        ui.add(primary_button(icon, text))
    } else {
        ui.add(egui::Button::image_and_text(
            icon.size(16.0).color(MUTED).image(),
            text,
        ))
    }
}

fn primary_button(icon: Lucide, text: &str) -> egui::Button<'static> {
    egui::Button::image_and_text(
        icon.size(18.0).color(Color32::WHITE).image(),
        RichText::new(text.to_owned()).color(Color32::WHITE),
    )
    .fill(GREEN)
    .min_size(Vec2::new(100.0, 36.0))
}

fn icon_button(ui: &mut egui::Ui, icon: Lucide, tooltip: &str) -> egui::Response {
    ui.add(egui::Button::image(icon.size(18.0).color(MUTED).image()).min_size(Vec2::splat(32.0)))
        .on_hover_text(tooltip)
}

fn section(ui: &mut egui::Ui, icon: Lucide, text: &str) {
    ui.horizontal(|ui| {
        ui.add(icon.size(16.0).color(MUTED));
        ui.label(RichText::new(text).strong());
    });
}

fn swatch(ui: &mut egui::Ui, rgb: [u8; 3]) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, 2.0, Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
        ui.painter().rect_stroke(
            rect,
            2.0,
            egui::Stroke::new(1.0, Color32::from_gray(205)),
            egui::StrokeKind::Inside,
        );
        ui.label(
            RichText::new(format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]))
                .size(11.0)
                .color(MUTED),
        );
    });
}

fn palette(ui: &mut egui::Ui, colors: &[[u8; 3]]) {
    ui.horizontal_wrapped(|ui| {
        for &rgb in colors {
            swatch(ui, rgb);
        }
    });
}

fn canvas_scroll(id: &str, size: Vec2) -> egui::ScrollArea {
    egui::ScrollArea::both()
        .id_salt(id)
        .max_width(size.x)
        .max_height(size.y)
        .min_scrolled_width(0.0)
        .min_scrolled_height(0.0)
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
}

fn preview_surface(
    ui: &mut egui::Ui,
    label: &str,
    texture: Option<&egui::TextureHandle>,
    zoom: f32,
    height: f32,
    busy: bool,
    error: Option<&str>,
) {
    ui.label(RichText::new(label).small().strong().color(MUTED));
    let width = ui.available_width();
    canvas_scroll(label, Vec2::new(width, height)).show(ui, |ui| {
        let image_size = texture.map(|texture| {
            let size = texture.size_vec2();
            size * if zoom == 0.0 {
                ((width - 24.0) / size.x)
                    .min((height - 24.0) / size.y)
                    .min(4.0)
            } else {
                zoom
            }
        });
        let canvas = image_size.map_or(Vec2::new(width, height), |size| {
            Vec2::new(width.max(size.x + 24.0), height.max(size.y + 24.0))
        });
        let (rect, _) = ui.allocate_exact_size(canvas, egui::Sense::hover());
        let painter = ui.painter_at(rect);
        checkerboard(&painter, rect);
        if let (Some(texture), Some(size)) = (texture, image_size) {
            painter.image(
                texture.id(),
                egui::Rect::from_center_size(rect.center(), size),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            let text = if busy {
                "변환 중…"
            } else if error.is_some() {
                "미리보기를 생성할 수 없습니다"
            } else {
                "미리보기 대기 중"
            };
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(13.0),
                MUTED,
            );
        }
    });
    if let Some(error) = error {
        ui.add(egui::Label::new(RichText::new(error).small().color(RED)).wrap());
    }
}

fn open_folder(path: &Path) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("explorer.exe")
            .arg(path)
            .creation_flags(0x08000000)
            .spawn();
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
