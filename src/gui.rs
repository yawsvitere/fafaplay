use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{mpsc, Arc},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, FontId, Layout, Margin, Rect, RichText, Rounding,
    Sense, Stroke, Vec2,
};
#[cfg(target_os = "windows")]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::{
    library::{self, letter_of, Library, ScanMsg},
    player::Engine,
};

// ───────────────────────── палитра ─────────────────────────
const CARD: Color32 = Color32::from_rgb(24, 24, 30);
const BORDER: Color32 = Color32::from_rgb(38, 38, 47);
const ACCENT: Color32 = Color32::from_rgb(139, 124, 255);
const ACCENT_SOFT: Color32 = Color32::from_rgb(52, 46, 96);
const TEXT: Color32 = Color32::from_rgb(236, 236, 242);
const DIM: Color32 = Color32::from_rgb(140, 140, 154);

fn load_logo(ctx: &egui::Context) -> egui::TextureHandle {
    let svg = include_bytes!("images/logo.svg");
    let tree = resvg::usvg::Tree::from_data(svg, &resvg::usvg::Options::default())
        .expect("embedded application logo SVG is valid");
    let size = tree.size().to_int_size();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height())
        .expect("embedded application logo has valid dimensions");
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::default(),
        &mut pixmap.as_mut(),
    );
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [size.width() as usize, size.height() as usize],
        pixmap.data(),
    );

    ctx.load_texture("application-logo", image, egui::TextureOptions::LINEAR)
}

type TrackId = (usize, usize, usize);

#[derive(Clone, Copy, PartialEq)]
enum Repeat {
    Off,
    All,
    One,
}

pub struct App {
    logo: egui::TextureHandle,
    engine: Option<Engine>,
    audio_error: Option<String>,

    folder: Option<PathBuf>,
    library: Option<Arc<Library>>,
    scan_rx: Option<mpsc::Receiver<ScanMsg>>,
    scan_count: usize,
    status: Option<String>,

    selected: Option<usize>,
    search: String,
    textures: HashMap<(usize, usize), egui::TextureHandle>,

    queue: Vec<TrackId>,
    qpos: usize,
    current: Option<TrackId>,
    paused: bool,

    seek_pos: f32,
    seeking: bool,
    volume: f32,
    shuffle: bool,
    repeat: Repeat,
    rng: u64,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        apply_theme(&cc.egui_ctx);
        let logo = load_logo(&cc.egui_ctx);
        #[cfg(target_os = "windows")]
        if let Err(error) = enable_windows_backdrop(cc) {
            eprintln!("Не удалось включить системное размытие окна: {error}");
        }

        let (engine, audio_error) = match Engine::new() {
            Ok(e) => (Some(e), None),
            Err(e) => (None, Some(format!("{e:#}"))),
        };

        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(88172645463325252)
            | 1;

        let mut app = Self {
            logo,
            engine,
            audio_error,
            folder: None,
            library: None,
            scan_rx: None,
            scan_count: 0,
            status: None,
            selected: None,
            search: String::new(),
            textures: HashMap::new(),
            queue: Vec::new(),
            qpos: 0,
            current: None,
            paused: false,
            seek_pos: 0.0,
            seeking: false,
            volume: 0.8,
            shuffle: false,
            repeat: Repeat::Off,
            rng: seed,
        };

        // Если в прошлый раз уже выбирали папку — сразу подхватываем её.
        if let Some(dir) = cc
            .storage
            .and_then(|s| s.get_string("music_dir"))
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
        {
            app.start_scan(dir, &cc.egui_ctx);
        }
        app
    }

    // ───────────────────────── библиотека ─────────────────────────

    fn pick_folder(&mut self, ctx: &egui::Context) {
        if let Some(dir) = rfd::FileDialog::new()
            .set_title("Выбери папку с музыкой")
            .pick_folder()
        {
            self.start_scan(dir, ctx);
        }
    }

    fn start_scan(&mut self, dir: PathBuf, ctx: &egui::Context) {
        self.stop_all();
        self.library = None;
        self.textures.clear();
        self.selected = None;
        self.search.clear();
        self.status = None;
        self.scan_count = 0;
        self.folder = Some(dir.clone());

        let (tx, rx) = mpsc::channel();
        self.scan_rx = Some(rx);
        let ctx = ctx.clone();

        thread::spawn(move || {
            let lib = library::scan(&dir, |n| {
                let _ = tx.send(ScanMsg::Progress(n));
                ctx.request_repaint();
            });
            let _ = tx.send(ScanMsg::Done(lib));
            ctx.request_repaint();
        });
    }

    fn poll_scan(&mut self) {
        let mut done = None;
        if let Some(rx) = &self.scan_rx {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    ScanMsg::Progress(n) => self.scan_count = n,
                    ScanMsg::Done(lib) => done = Some(lib),
                }
            }
        }
        if let Some(lib) = done {
            self.scan_rx = None;
            if lib.artists.is_empty() {
                self.status = Some("В этой папке не нашлось mp3 / flac / wav".into());
            } else {
                self.selected = Some(0);
                self.library = Some(Arc::new(lib));
            }
        }
    }

    fn cover_texture(
        &mut self,
        ctx: &egui::Context,
        lib: &Library,
        ai: usize,
        bi: usize,
    ) -> Option<egui::TextureId> {
        if let Some(t) = self.textures.get(&(ai, bi)) {
            return Some(t.id());
        }
        let cover = lib.artists[ai].albums[bi].cover.as_ref()?;
        let img =
            egui::ColorImage::from_rgba_unmultiplied([cover.width, cover.height], &cover.rgba);
        let handle = ctx.load_texture(
            format!("cover-{ai}-{bi}"),
            img,
            egui::TextureOptions::LINEAR,
        );
        let id = handle.id();
        self.textures.insert((ai, bi), handle);
        Some(id)
    }

    fn cover_widget(
        &mut self,
        ui: &mut egui::Ui,
        lib: &Library,
        ai: usize,
        bi: usize,
        size: Vec2,
        round: f32,
    ) {
        match self.cover_texture(ui.ctx(), lib, ai, bi) {
            Some(id) => {
                ui.add(
                    egui::Image::new(egui::load::SizedTexture::new(id, size))
                        .fit_to_exact_size(size)
                        .rounding(Rounding::same(round)),
                );
            }
            None => placeholder(ui, size, round),
        }
    }

    // ───────────────────────── воспроизведение ─────────────────────────

    fn is_playing(&self) -> bool {
        self.current.is_some() && !self.paused
    }

    fn stop_all(&mut self) {
        if let Some(e) = self.engine.as_mut() {
            e.stop();
        }
        self.queue.clear();
        self.current = None;
        self.paused = false;
        self.seek_pos = 0.0;
    }

    fn rand(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }

    fn play_album(&mut self, ai: usize, bi: usize, start: usize) {
        let Some(lib) = self.library.clone() else {
            return;
        };
        let n = lib.artists[ai].albums[bi].tracks.len();
        self.queue = (0..n).map(|t| (ai, bi, t)).collect();
        self.play_at(start);
    }

    fn play_at(&mut self, qpos: usize) {
        let Some(lib) = self.library.clone() else {
            return;
        };
        let Some(&id) = self.queue.get(qpos) else {
            return;
        };
        let track = &lib.artists[id.0].albums[id.1].tracks[id.2];
        let Some(engine) = self.engine.as_mut() else {
            return;
        };

        match engine.play(&track.path) {
            Ok(()) => {
                self.qpos = qpos;
                self.current = Some(id);
                self.paused = false;
                self.seek_pos = 0.0;
                self.status = None;
            }
            Err(err) => {
                self.current = None;
                self.status = Some(format!("{err:#}"));
            }
        }
    }

    fn toggle_play(&mut self) {
        if self.current.is_none() {
            return;
        }
        let finished = self.engine.as_ref().map_or(true, |e| e.is_finished());
        if finished {
            self.play_at(self.qpos);
            return;
        }
        if let Some(e) = &self.engine {
            if self.paused {
                e.resume();
                self.paused = false;
            } else {
                e.pause();
                self.paused = true;
            }
        }
    }

    fn next(&mut self, auto: bool) {
        let n = self.queue.len();
        if n == 0 {
            return;
        }
        let target = if self.repeat == Repeat::One && auto {
            Some(self.qpos)
        } else if self.shuffle && n > 1 {
            let mut i = (self.rand() % n as u64) as usize;
            if i == self.qpos {
                i = (i + 1) % n;
            }
            Some(i)
        } else if self.qpos + 1 < n {
            Some(self.qpos + 1)
        } else if self.repeat == Repeat::All {
            Some(0)
        } else {
            None
        };

        match target {
            Some(i) => self.play_at(i),
            None => {
                // конец очереди
                if let Some(e) = self.engine.as_mut() {
                    e.stop();
                }
                self.paused = true;
                self.seek_pos = 0.0;
            }
        }
    }

    fn seek(&mut self, secs: f32) {
        if let Some(e) = &self.engine {
            e.seek(Duration::from_secs_f32(secs.max(0.0)));
        }
    }

    fn prev(&mut self) {
        if self.current.is_none() {
            return;
        }
        let pos = self
            .engine
            .as_ref()
            .map_or(0.0, |e| e.position().as_secs_f32());
        if pos > 3.0 || self.queue.len() < 2 {
            self.seek(0.0);
        } else if self.qpos > 0 {
            self.play_at(self.qpos - 1);
        } else if self.repeat == Repeat::All {
            self.play_at(self.queue.len() - 1);
        } else {
            self.seek(0.0);
        }
    }

    fn tick_playback(&mut self) {
        if self.current.is_none() || self.paused {
            return;
        }
        let finished = self.engine.as_ref().map_or(false, |e| e.is_finished());
        if finished {
            self.next(true);
        }
    }

    // ───────────────────────── интерфейс ─────────────────────────

    fn welcome(&mut self, ctx: &egui::Context) {
        let mut pick = false;

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.24).max(20.0));
                ui.add(egui::Image::new((self.logo.id(), vec2(128.0, 128.0))));
                ui.label(
                    RichText::new("Твоя музыка. Твоя папка.")
                        .size(18.0)
                        .color(DIM),
                );
                ui.add_space(30.0);

                if self.scan_rx.is_some() {
                    ui.spinner();
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(format!("Сканирую… найдено треков: {}", self.scan_count))
                            .color(TEXT),
                    );
                } else {
                    let btn = egui::Button::new(
                        RichText::new("📂  Выбрать папку с музыкой")
                            .size(17.0)
                            .color(Color32::WHITE),
                    )
                    .fill(ACCENT)
                    .rounding(Rounding::same(26.0))
                    .min_size(vec2(290.0, 50.0));
                    if ui.add(btn).clicked() {
                        pick = true;
                    }
                    ui.add_space(12.0);
                    ui.label(RichText::new("mp3 · flac · wav").color(DIM));
                }

                if let Some(s) = &self.status {
                    ui.add_space(16.0);
                    ui.colored_label(Color32::from_rgb(255, 120, 120), s);
                }
                if let Some(e) = &self.audio_error {
                    ui.add_space(16.0);
                    ui.colored_label(
                        Color32::from_rgb(255, 170, 90),
                        format!("Звук недоступен: {e}"),
                    );
                }
            });
        });

        if pick {
            self.pick_folder(ctx);
        }
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        let lib = self.library.clone();

        egui::TopBottomPanel::top("player")
            .exact_height(92.0)
            .frame(
                egui::Frame::none()
                    .fill(Color32::TRANSPARENT)
                    .inner_margin(Margin::symmetric(18.0, 12.0))
                    .stroke(Stroke::new(1.0, BORDER)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    let cur = self.current;

                    // обложка текущего трека
                    let cover_size = vec2(66.0, 66.0);
                    match (cur, &lib) {
                        (Some((ai, bi, _)), Some(l)) => {
                            self.cover_widget(ui, l, ai, bi, cover_size, 8.0)
                        }
                        _ => placeholder(ui, cover_size, 8.0),
                    }
                    ui.add_space(14.0);

                    // кнопки транспорта
                    if icon_button(ui, 36.0, "⏮", None, TEXT).clicked() {
                        self.prev();
                    }
                    let glyph = if self.is_playing() { "⏸" } else { "▶" };
                    if icon_button(ui, 46.0, glyph, Some(ACCENT), Color32::WHITE).clicked() {
                        self.toggle_play();
                    }
                    if icon_button(ui, 36.0, "⏭", None, TEXT).clicked() {
                        self.next(false);
                    }
                    ui.add_space(16.0);

                    // центр: название + прогресс
                    let right_w = 250.0;
                    let center_w = (ui.available_width() - right_w - 16.0).max(200.0);
                    ui.allocate_ui_with_layout(
                        vec2(center_w, 66.0),
                        Layout::top_down(Align::LEFT),
                        |ui| {
                            let (title, sub, dur) = match (cur, &lib) {
                                (Some((a, b, t)), Some(l)) => {
                                    let ar = &l.artists[a];
                                    let al = &ar.albums[b];
                                    let tr = &al.tracks[t];
                                    (
                                        tr.title.clone(),
                                        format!("{} — {}", ar.name, al.title),
                                        tr.duration.as_secs_f32(),
                                    )
                                }
                                _ => ("Ничего не играет".to_string(), String::new(), 0.0),
                            };

                            ui.add(
                                egui::Label::new(
                                    RichText::new(title).size(15.0).strong().color(TEXT),
                                )
                                .truncate(),
                            );
                            ui.add(
                                egui::Label::new(RichText::new(sub).size(12.0).color(DIM))
                                    .truncate(),
                            );
                            ui.add_space(2.0);

                            let max = dur.max(1.0);
                            let pos_now = self
                                .engine
                                .as_ref()
                                .map_or(0.0, |e| e.position().as_secs_f32());
                            if !self.seeking {
                                self.seek_pos = pos_now.min(max);
                            }

                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(fmt_time(self.seek_pos))
                                        .monospace()
                                        .size(12.0)
                                        .color(DIM),
                                );
                                ui.spacing_mut().slider_width =
                                    (ui.available_width() - 60.0).max(50.0);
                                let resp = ui.add_enabled(
                                    cur.is_some(),
                                    egui::Slider::new(&mut self.seek_pos, 0.0..=max)
                                        .show_value(false)
                                        .trailing_fill(true),
                                );
                                self.seeking = resp.dragged();
                                if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
                                    let s = self.seek_pos;
                                    self.seek(s);
                                }
                                ui.label(
                                    RichText::new(fmt_time(dur))
                                        .monospace()
                                        .size(12.0)
                                        .color(DIM),
                                );
                            });
                        },
                    );

                    // справа: shuffle / repeat / громкость
                    ui.allocate_ui_with_layout(
                        vec2(right_w, 66.0),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            let sh_col = if self.shuffle { ACCENT } else { DIM };
                            if ui
                                .add(
                                    egui::Button::new(RichText::new("🔀").size(18.0).color(sh_col))
                                        .frame(false),
                                )
                                .on_hover_text("Перемешать")
                                .clicked()
                            {
                                self.shuffle = !self.shuffle;
                            }

                            let (glyph, col) = match self.repeat {
                                Repeat::Off => ("🔁", DIM),
                                Repeat::All => ("🔁", ACCENT),
                                Repeat::One => ("🔂", ACCENT),
                            };
                            if ui
                                .add(
                                    egui::Button::new(RichText::new(glyph).size(18.0).color(col))
                                        .frame(false),
                                )
                                .on_hover_text("Повтор: выкл / всё / один трек")
                                .clicked()
                            {
                                self.repeat = match self.repeat {
                                    Repeat::Off => Repeat::All,
                                    Repeat::All => Repeat::One,
                                    Repeat::One => Repeat::Off,
                                };
                            }

                            ui.add_space(10.0);
                            ui.label(RichText::new("🔈").color(DIM));
                            ui.spacing_mut().slider_width = 100.0;
                            let r = ui.add(
                                egui::Slider::new(&mut self.volume, 0.0..=1.0)
                                    .show_value(false)
                                    .trailing_fill(true),
                            );
                            if r.changed() {
                                let v = self.volume;
                                if let Some(e) = self.engine.as_mut() {
                                    e.set_volume(v);
                                }
                            }
                        },
                    );
                });
            });
    }

    /// Возвращает true, если нажали «сменить папку».
    fn sidebar(&mut self, ctx: &egui::Context) -> bool {
        let Some(lib) = self.library.clone() else {
            return false;
        };
        let mut pick = false;

        egui::SidePanel::left("artists")
            .default_width(260.0)
            .width_range(200.0..=420.0)
            .frame(
                egui::Frame::none()
                    .fill(Color32::TRANSPARENT)
                    .inner_margin(Margin::symmetric(12.0, 14.0))
                    .stroke(Stroke::new(1.0, BORDER)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Артисты").size(20.0).strong().color(TEXT));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add(egui::Button::new(RichText::new("📂").size(17.0)).frame(false))
                            .on_hover_text("Выбрать другую папку")
                            .clicked()
                        {
                            pick = true;
                        }
                    });
                });
                ui.add_space(6.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("🔍  Поиск артиста или альбома")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(4.0);

                let q = self.search.trim().to_lowercase();

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let mut last = '\0';
                        for (i, artist) in lib.artists.iter().enumerate() {
                            if !q.is_empty()
                                && !(artist.name.to_lowercase().contains(&q)
                                    || artist
                                        .albums
                                        .iter()
                                        .any(|a| a.title.to_lowercase().contains(&q)))
                            {
                                continue;
                            }

                            let l = letter_of(&artist.name);
                            if l != last {
                                last = l;
                                ui.add_space(10.0);
                                ui.label(
                                    RichText::new(l.to_string())
                                        .size(13.0)
                                        .strong()
                                        .color(ACCENT),
                                );
                                ui.add_space(2.0);
                            }

                            let selected = self.selected == Some(i);
                            let (rect, resp) = ui.allocate_exact_size(
                                vec2(ui.available_width(), 28.0),
                                Sense::click(),
                            );
                            if selected {
                                ui.painter()
                                    .rect_filled(rect, Rounding::same(7.0), ACCENT_SOFT);
                            } else if resp.hovered() {
                                ui.painter()
                                    .rect_filled(rect, Rounding::same(7.0), Color32::WHITE);
                            }
                            ui.painter_at(rect).text(
                                pos2(rect.left() + 10.0, rect.center().y),
                                Align2::LEFT_CENTER,
                                &artist.name,
                                FontId::proportional(14.0),
                                if selected {
                                    TEXT
                                } else if resp.hovered() {
                                    Color32::from_rgb(25, 25, 30)
                                } else {
                                    Color32::from_rgb(200, 200, 212)
                                },
                            );
                            if resp.clicked() {
                                self.selected = Some(i);
                            }
                        }
                    });
            });

        pick
    }

    fn main_view(&mut self, ctx: &egui::Context) {
        let Some(lib) = self.library.clone() else {
            return;
        };

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(Color32::TRANSPARENT)
                    .inner_margin(Margin::symmetric(28.0, 20.0)),
            )
            .show(ctx, |ui| {
                if let Some(s) = &self.status {
                    ui.colored_label(Color32::from_rgb(255, 120, 120), s);
                }

                let Some(ai) = self.selected.filter(|&i| i < lib.artists.len()) else {
                    ui.centered_and_justified(|ui| {
                        ui.label(RichText::new("Выбери артиста слева").size(18.0).color(DIM));
                    });
                    return;
                };

                let artist = &lib.artists[ai];

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for bi in 0..artist.albums.len() {
                            self.album_card(ui, &lib, ai, bi);
                            if bi + 1 < artist.albums.len() {
                                ui.add_space(8.0);
                                ui.separator();
                                ui.add_space(8.0);
                            }
                        }
                    });
            });
    }

    fn album_card(&mut self, ui: &mut egui::Ui, lib: &Library, ai: usize, bi: usize) {
        let album = &lib.artists[ai].albums[bi];
        let w = ui.available_width();

        egui::Frame::none().show(ui, |ui| {
            ui.set_width(w);

            ui.horizontal_top(|ui| {
                // обложка слева
                self.cover_widget(ui, lib, ai, bi, vec2(200.0, 200.0), 0.0);
                ui.add_space(18.0);

                // справа: заголовок и треки
                ui.vertical(|ui| {
                    ui.label(RichText::new(&album.title).size(22.0).strong().color(TEXT));

                    let total: f32 = album.tracks.iter().map(|t| t.duration.as_secs_f32()).sum();
                    let mut meta = Vec::new();
                    if let Some(y) = album.year {
                        meta.push(y.to_string());
                    }
                    meta.push(format!("{} треков", album.tracks.len()));
                    meta.push(format!("{} мин", (total / 60.0).round() as u32));
                    ui.label(RichText::new(meta.join("  ·  ")).color(DIM));
                    ui.add_space(8.0);

                    let multi_disc = album.tracks.iter().any(|t| t.disc.unwrap_or(1) > 1);
                    let mut last_disc = 0u32;
                    let mut to_play = None;

                    for (ti, t) in album.tracks.iter().enumerate() {
                        if multi_disc {
                            let d = t.disc.unwrap_or(1);
                            if d != last_disc {
                                last_disc = d;
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new(format!("Диск {d}"))
                                        .size(12.0)
                                        .strong()
                                        .color(ACCENT),
                                );
                            }
                        }

                        let (rect, resp) = ui
                            .allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
                        let is_cur = self.current == Some((ai, bi, ti));

                        if is_cur {
                            ui.painter()
                                .rect_filled(rect, Rounding::same(8.0), ACCENT_SOFT);
                        } else if resp.hovered() {
                            ui.painter()
                                .rect_filled(rect, Rounding::same(8.0), Color32::WHITE);
                        }
                        if resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }

                        let num = if is_cur {
                            if self.is_playing() { "▶" } else { "⏸" }.to_string()
                        } else if resp.hovered() {
                            "▶".to_string()
                        } else {
                            t.track_no
                                .map_or_else(|| (ti + 1).to_string(), |n| n.to_string())
                        };
                        let num_col = if is_cur {
                            ACCENT
                        } else if resp.hovered() {
                            Color32::from_rgb(25, 25, 30)
                        } else {
                            DIM
                        };
                        let title_col = if is_cur {
                            TEXT
                        } else if resp.hovered() {
                            Color32::from_rgb(25, 25, 30)
                        } else {
                            Color32::from_rgb(214, 214, 224)
                        };

                        ui.painter().text(
                            pos2(rect.left() + 22.0, rect.center().y),
                            Align2::CENTER_CENTER,
                            num,
                            FontId::proportional(13.0),
                            num_col,
                        );

                        let title_clip =
                            Rect::from_min_max(rect.min, pos2(rect.right() - 70.0, rect.bottom()));
                        ui.painter_at(title_clip).text(
                            pos2(rect.left() + 50.0, rect.center().y),
                            Align2::LEFT_CENTER,
                            &t.title,
                            FontId::proportional(14.5),
                            title_col,
                        );
                        ui.painter().text(
                            pos2(rect.right() - 14.0, rect.center().y),
                            Align2::RIGHT_CENTER,
                            fmt_time(t.duration.as_secs_f32()),
                            FontId::monospace(12.5),
                            DIM,
                        );

                        if resp.clicked() {
                            to_play = Some(ti);
                        }
                    }

                    if let Some(ti) = to_play {
                        self.play_album(ai, bi, ti);
                    }
                });
            });
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_scan();
        self.tick_playback();

        if self.library.is_none() {
            self.welcome(ctx);
        } else {
            self.top_bar(ctx);
            let pick = self.sidebar(ctx);
            self.main_view(ctx);
            if pick {
                self.pick_folder(ctx);
            }
        }

        // пробел — пауза / продолжить (если не печатаем в поиске)
        if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Space)) {
            self.toggle_play();
        }

        if self.scan_rx.is_some() || self.is_playing() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Some(f) = &self.folder {
            storage.set_string("music_dir", f.to_string_lossy().into_owned());
        }
    }
}

// ───────────────────────── хелперы ─────────────────────────

fn apply_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = Color32::TRANSPARENT;
    v.window_fill = Color32::TRANSPARENT;
    v.extreme_bg_color = Color32::from_rgb(10, 10, 13);
    v.faint_bg_color = CARD;
    v.selection.bg_fill = ACCENT_SOFT;
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.inactive.bg_fill = Color32::from_rgb(44, 44, 54);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(34, 34, 42);
    v.widgets.hovered.bg_fill = Color32::WHITE;
    v.widgets.hovered.weak_bg_fill = Color32::WHITE;
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::from_rgb(25, 25, 30));
    v.widgets.active.bg_fill = ACCENT;
    ctx.set_visuals(v);

    ctx.style_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 6.0);
        s.spacing.button_padding = vec2(10.0, 6.0);
        s.spacing.scroll.bar_width = 8.0;
    });
}

#[cfg(target_os = "windows")]
fn enable_windows_backdrop(cc: &eframe::CreationContext<'_>) -> Result<(), String> {
    let handle = cc
        .window_handle()
        .map_err(|error| format!("ошибка получения дескриптора окна: {error}"))?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Err("получен неподдерживаемый тип дескриптора окна".to_owned());
    };
    let hwnd = handle.hwnd.get() as windows_sys::Win32::Foundation::HWND;
    let backdrop_type = windows_sys::Win32::Graphics::Dwm::DWMSBT_TRANSIENTWINDOW;
    let result = unsafe {
        windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            hwnd,
            windows_sys::Win32::Graphics::Dwm::DWMWA_SYSTEMBACKDROP_TYPE as u32,
            &backdrop_type as *const _ as *const std::ffi::c_void,
            std::mem::size_of_val(&backdrop_type) as u32,
        )
    };
    if result < 0 {
        return Err(format!(
            "DwmSetWindowAttribute завершился с кодом {result:#x}"
        ));
    }
    Ok(())
}

fn placeholder(ui: &mut egui::Ui, size: Vec2, round: f32) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_filled(rect, Rounding::same(round), Color32::from_rgb(34, 34, 43));
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        "♪",
        FontId::proportional(size.y * 0.45),
        Color32::from_rgb(76, 76, 96),
    );
}

fn lighten(c: Color32) -> Color32 {
    Color32::from_rgb(
        c.r().saturating_add(22),
        c.g().saturating_add(22),
        c.b().saturating_add(22),
    )
}

fn icon_button(
    ui: &mut egui::Ui,
    size: f32,
    glyph: &str,
    fill: Option<Color32>,
    fg: Color32,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let bg = match (fill, resp.hovered()) {
        (Some(c), true) => lighten(c),
        (Some(c), false) => c,
        (None, true) => Color32::from_rgb(44, 44, 54),
        (None, false) => Color32::TRANSPARENT,
    };
    ui.painter().circle_filled(rect.center(), size / 2.0, bg);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(size * 0.45),
        fg,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn fmt_time(secs: f32) -> String {
    let s = secs.max(0.0) as u32;
    format!("{}:{:02}", s / 60, s % 60)
}
