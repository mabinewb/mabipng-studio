use super::*;

impl Studio {
    pub fn open_batch(&mut self, ctx: &egui::Context) {
        if self.pending > 0 || self.batch_dialog.is_some() {
            return;
        }
        self.finish_strokes(ctx);
        self.batch_dialog = Some(BatchDialog {
            paths: Vec::new(),
            options: batch::Options {
                algorithm: Algorithm::default(),
                output_dir: self
                    .output_dir
                    .clone()
                    .or_else(default_output_dir)
                    .unwrap_or_else(|| self.browser_dir.clone()),
                author: self.author.clone(),
                replace_existing_author: false,
                fix_palette: self.fix_palette,
                width: 256,
                height: 96,
                cover: false,
                sampling: Sampling::Smooth,
                palette: PaletteMode::Adaptive,
                preserve_white: true,
                resize_existing: false,
            },
            running: false,
            results: Vec::new(),
            stop: Arc::new(AtomicBool::new(false)),
            error: None,
            cancelled: false,
        });
    }

    pub fn add_batch_paths(&mut self, paths: Vec<PathBuf>) {
        let Some(dialog) = &mut self.batch_dialog else {
            return;
        };
        if dialog.running {
            return;
        }
        for path in paths {
            if !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("png"))
            {
                continue;
            }
            let path = path.canonicalize().unwrap_or(path);
            if !dialog.paths.contains(&path) {
                dialog.paths.push(path);
            }
        }
    }

    pub(super) fn start_batch(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.batch_dialog else {
            return;
        };
        if dialog.running || dialog.paths.is_empty() || self.pending > 0 {
            return;
        }
        if dialog.options.replace_existing_author && dialog.options.author.trim().is_empty() {
            dialog.error = Some("변경할 작성자 이름을 입력해 주시기 바랍니다.".into());
            return;
        }
        dialog.running = true;
        dialog.results.clear();
        dialog.error = None;
        dialog.cancelled = false;
        dialog.stop.store(false, Ordering::Relaxed);
        let paths = dialog.paths.clone();
        let options = dialog.options.clone();
        let stop = dialog.stop.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            for source in paths {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let output = batch::convert_one(&source, &options).map_err(|e| format!("{e:#}"));
                if tx
                    .send(Event::BatchItem(batch::ItemResult { source, output }))
                    .is_err()
                {
                    return;
                }
                ctx.request_repaint();
            }
            let _ = tx.send(Event::BatchDone(stop.load(Ordering::Relaxed)));
            ctx.request_repaint();
        });
    }

    pub(super) fn batch_modal(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.batch_dialog else {
            return;
        };
        let mut additions = Vec::new();
        let mut start = false;
        let mut close = false;
        let mut remove = None;
        egui::Modal::new(egui::Id::new("batch-conversion")).show(ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 80.0).clamp(600.0, 850.0));
            ui.heading("일괄 변환");
            egui::ScrollArea::vertical()
                .id_salt("batch-settings")
                .max_height((ctx.content_rect().height() - 200.0).max(200.0))
                .show(ui, |ui| {
                    ui.add_enabled_ui(!dialog.running, |ui| {
                        ui.horizontal(|ui| {
                            if command(ui, Lucide::FilePlus, "PNG 추가", false).clicked()
                                && let Some(paths) = rfd::FileDialog::new()
                                    .set_title("일괄 변환할 PNG 선택")
                                    .add_filter("PNG", &["png"])
                                    .pick_files()
                            {
                                additions = paths;
                            }
                            if command(ui, Lucide::FolderOpen, "폴더 추가", false).clicked()
                                && let Some(path) = rfd::FileDialog::new()
                                    .set_title("일괄 변환할 폴더 선택")
                                    .pick_folder()
                            {
                                match browser::list_pngs(&path) {
                                    Ok(paths) => additions = paths,
                                    Err(error) => dialog.error = Some(error.to_string()),
                                }
                            }
                            if icon_button(ui, Lucide::Trash2, "목록 비우기 · 원본 파일 유지")
                                .clicked()
                            {
                                dialog.paths.clear();
                                dialog.results.clear();
                            }
                            ui.label(format!("{}개 파일", dialog.paths.len()));
                        });
                    });
                    egui::ScrollArea::vertical()
                        .id_salt("batch-files")
                        .max_height(160.0)
                        .min_scrolled_height(80.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for (index, path) in dialog.paths.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    ui.add_sized(
                                        [(ui.available_width() - 110.0).max(80.0), 24.0],
                                        egui::Label::new(display_name(path))
                                            .truncate()
                                            .halign(egui::Align::Min),
                                    )
                                    .on_hover_text(path.display().to_string());
                                    if let Some(result) =
                                        dialog.results.iter().find(|r| &r.source == path)
                                    {
                                        match &result.output {
                                            Ok(output) => {
                                                ui.colored_label(GREEN, "완료")
                                                    .on_hover_text(output.display().to_string());
                                            }
                                            Err(error) => {
                                                ui.colored_label(RED, "실패").on_hover_text(error);
                                            }
                                        }
                                    }
                                    if ui
                                        .add_enabled(
                                            !dialog.running,
                                            egui::Button::image(
                                                Lucide::X.size(14.0).color(MUTED).image(),
                                            ),
                                        )
                                        .on_hover_text("변환 목록에서 제외")
                                        .clicked()
                                    {
                                        remove = Some(index);
                                    }
                                });
                            }
                        });
                    ui.separator();
                    ui.add_enabled_ui(!dialog.running, |ui| {
                        ui.columns(2, |columns| {
                            let ui = &mut columns[0];
                            section(ui, Lucide::UserRound, "작성자");
                            ui.add(
                                egui::TextEdit::singleline(&mut dialog.options.author)
                                    .background_color(Color32::from_rgb(242, 245, 244))
                                    .margin(Vec2::new(8.0, 6.0))
                                    .hint_text("새 그림대화 작성자")
                                    .desired_width(f32::INFINITY),
                            );
                            ui.checkbox(
                                &mut dialog.options.replace_existing_author,
                                "기존 그림대화 작성자도 변경",
                            ).on_hover_text("켜면 기존 그림대화의 작성자도 입력한 이름으로 바꿉니다. 끄면 기존 작성자를 유지하고 새 그림대화에만 입력한 이름을 사용합니다.");
                            reserved_palette_checkbox(ui, &mut dialog.options.fix_palette, true, "예약 색상 자동 보정");
                            ui.add_space(8.0);
                            section(ui, Lucide::FolderOpen, "저장 폴더");
                            ui.add(
                                egui::Label::new(dialog.options.output_dir.display().to_string())
                                    .wrap(),
                            );
                            if command(ui, Lucide::FolderOpen, "폴더 선택", false).clicked()
                                && let Some(path) = rfd::FileDialog::new()
                                    .set_title("일괄 변환 결과 저장 폴더")
                                    .pick_folder()
                            {
                                dialog.options.output_dir = path;
                            }
                            let ui = &mut columns[1];
                            section(ui, Lucide::Scaling, "캔버스");
                            ui.horizontal(|ui| {
                                ui.label("너비");
                                ui.add(
                                    egui::DragValue::new(&mut dialog.options.width)
                                        .range(1..=256)
                                        .suffix(" px"),
                                );
                                ui.label("높이");
                                ui.add(
                                    egui::DragValue::new(&mut dialog.options.height)
                                        .range(1..=96)
                                        .suffix(" px"),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.radio_value(&mut dialog.options.cover, false, "전체 맞춤").on_hover_text("이미지 전체가 캔버스 안에 들어오도록 비율을 유지해 배치합니다. 비율이 다르면 여백이 생깁니다.");
                                ui.radio_value(&mut dialog.options.cover, true, "채우기").on_hover_text("여백 없이 캔버스를 채우도록 비율을 유지해 배치합니다. 캔버스 밖으로 나간 이미지 부분은 잘립니다.");
                            });
                            ui.checkbox(
                                &mut dialog.options.resize_existing,
                                "기존 그림대화 크기도 변경",
                            ).on_hover_text("켜면 기존 그림대화도 지정한 캔버스 크기와 배치 방식으로 조절합니다. 끄면 기존 그림대화의 크기를 유지합니다. 원본은 덮어쓰지 않고 새 파일로 저장합니다.");
                            egui::ComboBox::from_id_salt("batch-palette")
                                .selected_text(if dialog.options.palette == PaletteMode::Default {
                                    "일반 이미지: 기본 회색조 4색"
                                } else {
                                    "일반 이미지: 이미지에 맞춘 4색"
                                })
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut dialog.options.palette,
                                        PaletteMode::Adaptive,
                                        "이미지에 맞춘 4색",
                                    );
                                    ui.selectable_value(
                                        &mut dialog.options.palette,
                                        PaletteMode::Default,
                                        "기본 회색조 4색",
                                    );
                                }).response.on_hover_text(PALETTE_MODE_HELP);
                            ui.add_enabled_ui(
                                dialog.options.palette == PaletteMode::Adaptive,
                                |ui| {
                                    egui::ComboBox::from_id_salt("batch-algorithm")
                                        .selected_text(dialog.options.algorithm.label())
                                        .show_ui(ui, |ui| {
                                            for algorithm in Algorithm::ALL {
                                                ui.selectable_value(
                                                    &mut dialog.options.algorithm,
                                                    algorithm,
                                                    algorithm.label(),
                                                ).on_hover_text(algorithm.description());
                                            }
                                        }).response.on_hover_text(dialog.options.algorithm.description());
                                },
                            );
                            egui::ComboBox::from_id_salt("batch-sampling")
                                .selected_text(if dialog.options.sampling == Sampling::Nearest {
                                    "픽셀 유지"
                                } else {
                                    "부드럽게 축소"
                                })
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut dialog.options.sampling,
                                        Sampling::Nearest,
                                        "픽셀 유지",
                                    );
                                    ui.selectable_value(
                                        &mut dialog.options.sampling,
                                        Sampling::Smooth,
                                        "부드럽게 축소",
                                    );
                                }).response.on_hover_text(SAMPLING_HELP);
                            ui.add_enabled(
                                dialog.options.palette == PaletteMode::Adaptive,
                                egui::Checkbox::new(
                                    &mut dialog.options.preserve_white,
                                    "흰 배경 분리",
                                ),
                            ).on_hover_text(WHITE_BACKGROUND_HELP).on_disabled_hover_text("흰 배경 분리는 '이미지에 맞춘 4색' 모드에서 사용할 수 있습니다.");
                        });
                    });
                    for result in dialog.results.iter().filter(|r| r.output.is_err()) {
                        ui.colored_label(
                            RED,
                            format!(
                                "{}: {}",
                                display_name(&result.source),
                                result.output.as_ref().unwrap_err()
                            ),
                        );
                    }
                });
            if let Some(error) = &dialog.error {
                ui.colored_label(RED, error);
            }
            let ok = dialog.results.iter().filter(|r| r.output.is_ok()).count();
            if dialog.running {
                ui.add(
                    egui::ProgressBar::new(
                        dialog.results.len() as f32 / dialog.paths.len().max(1) as f32,
                    )
                    .text(format!(
                        "변환 중 · {} / {}",
                        dialog.results.len(),
                        dialog.paths.len()
                    )),
                );
            } else if !dialog.results.is_empty() || dialog.cancelled {
                ui.label(format!(
                    "{} · 성공 {}개 · 실패 {}개",
                    if dialog.cancelled {
                        "중단됨"
                    } else {
                        "완료"
                    },
                    ok,
                    dialog.results.len() - ok
                ));
            }
            ui.horizontal(|ui| {
                if dialog.running {
                    if command(ui, Lucide::Square, "중단", false).clicked() {
                        dialog.stop.store(true, Ordering::Relaxed);
                    }
                } else {
                    start = ui
                        .add_enabled(
                            !dialog.paths.is_empty(),
                            primary_button(Lucide::Play, "새 파일로 일괄 변환"),
                        )
                        .clicked();
                    close = command(ui, Lucide::X, "닫기", false).clicked();
                }
            });
        });
        if let Some(index) = remove {
            dialog.paths.remove(index);
        }
        if !dialog.running && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        if !additions.is_empty() {
            self.add_batch_paths(additions);
        }
        if start {
            self.start_batch(ctx);
        }
        if close {
            self.batch_dialog = None;
        }
    }
}
