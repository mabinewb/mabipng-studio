use super::*;

impl FrameDialog {
    fn render(&self, preview: bool) -> anyhow::Result<Document> {
        let author = if preview { "Test" } else { &self.author };
        let settings = Settings {
            mode: Mode::Convert,
            author: Some(author.into()),
            fix_palette: self.fix_palette,
            preserve_white_background: self.preserve_white,
            resize: None,
        };
        let mut doc = self.frame.render(&self.source, self.mode, &settings)?;
        if self.mode == PaletteMode::Keep {
            let from = if self.fix_palette {
                RAW_PALETTE
            } else {
                engine::DEFAULT_CHAT_PALETTE
            };
            let to = if self.fix_palette {
                engine::DEFAULT_CHAT_PALETTE
            } else {
                RAW_PALETTE
            };
            let palette: Vec<_> = doc
                .palette
                .iter()
                .map(|&rgb| from.iter().position(|&c| c == rgb).map_or(rgb, |i| to[i]))
                .collect();
            doc = Document::from_bytes(engine::replace_palette(&doc, &palette)?)?;
        }
        if self.mode == PaletteMode::Keep
            && !preview
            && self.source.author.as_deref() != Some(self.author.as_str())
        {
            return Document::from_bytes(engine::process(
                &doc,
                &Settings {
                    mode: Mode::Author,
                    author: Some(self.author.clone()),
                    fix_palette: false,
                    ..Default::default()
                },
            )?);
        }
        Ok(doc)
    }
}

impl Studio {
    pub(super) fn queue_current_conversion(&mut self, ctx: &egui::Context) {
        if let Some(id) = self.selected
            && !self.conversion_queue.contains(&id)
        {
            self.conversion_queue.push_front(id);
        }
        self.open_next_conversion(ctx);
    }

    pub(super) fn open_next_conversion(&mut self, ctx: &egui::Context) {
        if self.pending > 0
            || self.frame_dialog.is_some()
            || self.batch_dialog.is_some()
            || self.show_new
            || self.save_dialog.is_some()
            || self.remove_request.is_some()
            || self.confirm_discard
            || self.paste_preview.is_some()
        {
            return;
        }
        while let Some(id) = self.conversion_queue.pop_front() {
            let Some(entry) = self
                .entries
                .iter()
                .find(|e| e.id == id && !e.effective_doc().is_chat())
            else {
                continue;
            };
            let source = entry.effective_doc().clone();
            self.activate_entry(id);
            self.open_framing(ctx, source.clone(), Some(id));
            if let Some(dialog) = &mut self.frame_dialog {
                dialog.initial_import = true;
                dialog.frame.fit(source.image.dimensions(), false);
            }
            break;
        }
    }

    pub(super) fn cancel_framing(&mut self) {
        if let Some(dialog) = self.frame_dialog.take()
            && dialog.initial_import
            && let Some(id) = dialog.target
        {
            self.close_entry(id);
        }
    }

    pub(super) fn open_framing(
        &mut self,
        ctx: &egui::Context,
        source: Arc<Document>,
        target: Option<u64>,
    ) {
        if self.pending > 0 || self.frame_dialog.is_some() {
            return;
        }
        self.finish_strokes(ctx);
        let mut frame = Frame::new(&source);
        if target.is_some() {
            frame.set_size(
                source.image.width().clamp(1, 256),
                source.image.height().clamp(1, 96),
            );
            frame.scale = 1.0;
            frame.center(source.image.dimensions());
        }
        self.frame_dialog = Some(FrameDialog {
            texture: texture(ctx, "framing-source", &source.image),
            mode: if source.is_chat() {
                PaletteMode::Keep
            } else {
                PaletteMode::Adaptive
            },
            author: if target.is_some() {
                self.selected_entry()
                    .and_then(|e| e.author_override.clone())
                    .or_else(|| source.author.clone())
                    .unwrap_or_else(|| self.author.clone())
            } else {
                self.author.clone()
            },
            source,
            target,
            frame,
            preserve_white: true,
            fix_palette: self.fix_palette,
            preview: None,
            error: None,
            dirty: true,
            viewport: None,
            initial_import: false,
        });
    }

    pub(super) fn open_canvas_resize(&mut self, ctx: &egui::Context) {
        if self.pending > 0 || self.clipboard_busy {
            return;
        }
        self.finish_strokes(ctx);
        if let Some(entry) = self.selected_entry() {
            self.open_framing(ctx, entry.effective_doc().clone(), Some(entry.id));
        }
    }

    pub fn start_framing(&mut self) {
        self.frame_when_ready = true;
    }

    pub(super) fn apply_framing(&mut self, ctx: &egui::Context) -> Result<(), String> {
        if self.pending > 0 {
            return Err("저장 중에는 적용할 수 없습니다.".into());
        }
        let dialog = self
            .frame_dialog
            .as_ref()
            .ok_or("영역 선택 창이 열려 있지 않습니다.")?;
        let doc = dialog.render(false).map_err(|e| format!("{e:#}"))?;
        let mut painting = make_painting(ctx, Arc::new(doc)).map_err(|e| e.to_string())?;
        painting.unsaved = true;
        let target = dialog.target;
        let author = dialog.author.clone();
        let auto = dialog.fix_palette;
        if let Some(id) = target {
            let entry = self
                .entries
                .iter_mut()
                .find(|e| e.id == id)
                .ok_or("편집할 문서가 닫혔습니다.")?;
            if !dialog.initial_import {
                entry.checkpoint();
            }
            if let Some(previous) = &entry.painting {
                painting
                    .initial_palette
                    .clone_from(&previous.initial_palette);
            }
            entry.painting = Some(painting);
            entry.author_override =
                (dialog.source.author.as_deref() != Some(author.as_str())).then(|| author.clone());
            entry.auto_palette = auto;
            entry.draft = true;
            entry.output = None;
            entry.status = "캔버스 조절됨".into();
            self.selected = Some(id);
        } else {
            let original =
                Document::from_bytes(dialog.source.bytes.clone()).map_err(|e| e.to_string())?;
            self.author = author.clone();
            self.fix_palette = auto;
            self.add_document(
                ctx,
                PathBuf::from(format!("클립보드_{}.png", self.next_id + 1)),
                original,
                true,
            );
            self.entries.last_mut().unwrap().painting = Some(painting);
        }
        self.author = author;
        self.fix_palette = auto;
        self.frame_dialog = None;
        self.view = 3;
        self.tool = Tool::Brush;
        self.invalidate();
        Ok(())
    }

    pub(super) fn framing_modal(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.frame_dialog else {
            return;
        };
        let mut apply = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::new("frame-image")).show(ctx, |ui| {
            let width = (ctx.content_rect().width() - 64.0).clamp(600.0, 900.0);
            ui.set_width(width);
            ui.horizontal(|ui| {
                ui.heading(if dialog.initial_import {
                    "PNG 변환 · 캔버스 및 이미지 배치"
                } else if dialog.target.is_some() {
                    "캔버스 크기 및 이미지 배치"
                } else {
                    "클립보드 이미지 영역 선택"
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    dialog.dirty |= ui
                        .add(
                            egui::TextEdit::singleline(&mut dialog.author)
                                .background_color(Color32::from_rgb(242, 245, 244))
                                .margin(Vec2::new(8.0, 6.0))
                                .desired_width(220.0),
                        )
                        .changed();
                    ui.label("작성자");
                });
            });
            egui::ScrollArea::vertical()
                .id_salt("frame-controls-scroll")
                .max_height((ctx.content_rect().height() - 180.0).max(200.0))
                .show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(width - 284.0);
                            section(ui, Lucide::Crop, "이미지 영역");
                            frame_view(ui, dialog, 270.0);
                            ui.label(
                                RichText::new(format!(
                                    "원본 {} × {}",
                                    dialog.source.image.width(),
                                    dialog.source.image.height()
                                ))
                                .small()
                                .color(MUTED),
                            );
                            ui.add_space(12.0);
                            section(ui, Lucide::Image, "변환 결과");
                            if dialog.dirty
                                && (!ctx.input(|i| i.pointer.primary_down())
                                    || dialog.preview.is_none())
                            {
                                match dialog.render(true) {
                                    Ok(doc) => {
                                        dialog.preview = Some(Preview {
                                            texture: texture(ctx, "framing-result", &doc.image),
                                            doc,
                                        });
                                        dialog.error = None;
                                    }
                                    Err(error) => {
                                        dialog.error = Some(error.to_string());
                                        dialog.preview = None;
                                    }
                                }
                                dialog.dirty = false;
                            }
                            preview_surface(
                                ui,
                                "출력",
                                dialog.preview.as_ref().map(|p| &p.texture),
                                0.0,
                                140.0,
                                false,
                                None,
                            );
                        });
                        ui.separator();
                        ui.vertical(|ui| {
                            ui.set_width(250.0);
                            section(ui, Lucide::Scaling, "출력 크기");
                            let before = dialog.frame.clone();
                            let mut w = dialog.frame.width;
                            let mut h = dialog.frame.height;
                            ui.horizontal(|ui| {
                                ui.label("너비");
                                ui.add(egui::DragValue::new(&mut w).range(1..=256).suffix(" px"));
                                ui.label("높이");
                                ui.add(egui::DragValue::new(&mut h).range(1..=96).suffix(" px"));
                            });
                            egui::ComboBox::from_id_salt("frame-size-preset")
                                .selected_text("크기 프리셋")
                                .width(236.0)
                                .show_ui(ui, |ui| {
                                    for (pw, ph) in [(96, 96), (128, 48), (192, 72), (256, 96)] {
                                        if ui
                                            .selectable_label(
                                                w == pw && h == ph,
                                                format!("{pw} × {ph}"),
                                            )
                                            .clicked()
                                        {
                                            w = pw;
                                            h = ph;
                                        }
                                    }
                                });
                            if (w, h) != (dialog.frame.width, dialog.frame.height) {
                                dialog.frame.set_size(w, h);
                            }
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                if icon_button(ui, Lucide::Minimize, "전체 이미지 맞춤").clicked()
                                {
                                    dialog.frame.fit(dialog.source.image.dimensions(), false);
                                }
                                if icon_button(ui, Lucide::Maximize, "캔버스 채우기").clicked()
                                {
                                    dialog.frame.fit(dialog.source.image.dimensions(), true);
                                }
                                if icon_button(ui, Lucide::Scan, "원본 배율 100%").clicked() {
                                    dialog.frame.scale = 1.0;
                                    dialog.frame.center(dialog.source.image.dimensions());
                                }
                                if icon_button(
                                    ui,
                                    Lucide::AlignCenterHorizontal,
                                    "이미지 가운데 정렬",
                                )
                                .clicked()
                                {
                                    dialog.frame.center(dialog.source.image.dimensions());
                                }
                            });
                            let mut percent = dialog.frame.scale * 100.0;
                            if ui
                                .add(
                                    egui::Slider::new(&mut percent, 0.001..=6400.0)
                                        .logarithmic(true)
                                        .text("배율 %"),
                                )
                                .changed()
                            {
                                dialog.frame.zoom(percent / 100.0);
                            }
                            ui.horizontal(|ui| {
                                ui.label("X");
                                ui.add(
                                    egui::DragValue::new(&mut dialog.frame.offset[0])
                                        .speed(0.5)
                                        .range(-1e6..=1e6),
                                );
                                ui.label("Y");
                                ui.add(
                                    egui::DragValue::new(&mut dialog.frame.offset[1])
                                        .speed(0.5)
                                        .range(-1e6..=1e6),
                                );
                            });
                            ui.add_space(10.0);
                            section(ui, Lucide::Palette, "변환 옵션");
                            let old_mode = dialog.mode;
                            egui::ComboBox::from_id_salt("framing-palette-mode")
                                .selected_text(match dialog.mode {
                                    PaletteMode::Keep => "원본 팔레트 유지",
                                    PaletteMode::Adaptive => "이미지에 맞춘 4색",
                                    PaletteMode::Default => "기본 회색조 4색",
                                })
                                .width(236.0)
                                .show_ui(ui, |ui| {
                                    if dialog.source.is_chat() {
                                        ui.selectable_value(
                                            &mut dialog.mode,
                                            PaletteMode::Keep,
                                            "원본 팔레트 유지",
                                        );
                                    }
                                    ui.selectable_value(
                                        &mut dialog.mode,
                                        PaletteMode::Adaptive,
                                        "이미지에 맞춘 4색",
                                    );
                                    ui.selectable_value(
                                        &mut dialog.mode,
                                        PaletteMode::Default,
                                        "기본 회색조 4색",
                                    );
                                });
                            if dialog.mode == PaletteMode::Keep {
                                dialog.frame.sampling = Sampling::Nearest;
                            }
                            let mut options_changed = old_mode != dialog.mode;
                            ui.add_enabled_ui(dialog.mode != PaletteMode::Keep, |ui| {
                                egui::ComboBox::from_id_salt("framing-sampling")
                                    .selected_text(match dialog.frame.sampling {
                                        Sampling::Nearest => "픽셀 유지",
                                        Sampling::Smooth => "부드럽게 축소",
                                    })
                                    .width(236.0)
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut dialog.frame.sampling,
                                            Sampling::Nearest,
                                            "픽셀 유지",
                                        );
                                        ui.selectable_value(
                                            &mut dialog.frame.sampling,
                                            Sampling::Smooth,
                                            "부드럽게 축소",
                                        );
                                    });
                            });
                            options_changed |= ui
                                .add_enabled(
                                    dialog.mode == PaletteMode::Adaptive,
                                    egui::Checkbox::new(&mut dialog.preserve_white, "흰 배경 분리"),
                                )
                                .changed();
                            options_changed |= ui
                                .checkbox(&mut dialog.fix_palette, "예약 색상 자동 보정")
                                .changed();
                            ui.horizontal(|ui| {
                                ui.label("여백 색상");
                                if dialog.mode == PaletteMode::Keep {
                                    for (i, &rgb) in dialog.source.palette.iter().enumerate() {
                                        if ui
                                            .add(
                                                egui::Button::new("")
                                                    .fill(Color32::from_rgb(rgb[0], rgb[1], rgb[2]))
                                                    .min_size(Vec2::splat(24.0))
                                                    .selected(
                                                        dialog.frame.background_index == i as u8,
                                                    ),
                                            )
                                            .on_hover_text(format!("팔레트 {}", i + 1))
                                            .clicked()
                                        {
                                            dialog.frame.background_index = i as u8;
                                        }
                                    }
                                } else {
                                    ui.color_edit_button_srgb(&mut dialog.frame.background);
                                }
                            });
                            dialog.dirty |= before != dialog.frame || options_changed;
                        });
                    });
                });
            if let Some(error) = &dialog.error {
                ui.colored_label(RED, error);
            }
            ui.separator();
            ui.horizontal(|ui| {
                apply = command(ui, Lucide::Check, "적용", true).clicked();
                cancel = command(ui, Lucide::X, "취소", false).clicked();
            });
        });
        if apply && let Err(error) = self.apply_framing(ctx) {
            self.frame_dialog.as_mut().unwrap().error = Some(error);
        }
        if cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.cancel_framing();
        }
        if self.frame_dialog.as_ref().is_some_and(|d| d.dirty) {
            ctx.request_repaint();
        }
    }

    pub(super) fn close_entry(&mut self, id: u64) {
        if self.pending > 0 {
            return;
        }
        if let Some(index) = self.entries.iter().position(|e| e.id == id) {
            self.entries.remove(index);
            self.conversion_queue.retain(|&queued| queued != id);
            if self.selected == Some(id) {
                self.selected = None;
                if let Some(next) = self
                    .entries
                    .get(index.min(self.entries.len().saturating_sub(1)))
                {
                    self.activate_entry(next.id);
                }
            }
            self.invalidate();
        }
    }

    pub(super) fn request_close_entry(&mut self, ctx: &egui::Context, id: u64) {
        if self.pending > 0 {
            return;
        }
        self.finish_strokes(ctx);
        if self
            .entries
            .iter()
            .find(|e| e.id == id)
            .is_some_and(Entry::has_unsaved)
        {
            self.remove_request = Some(RemoveRequest::Close(id));
        } else {
            self.close_entry(id);
        }
    }

    pub(super) fn confirm_remove(&mut self, ctx: &egui::Context) -> Result<(), String> {
        if self.pending > 0 {
            return Err("저장 중에는 문서를 닫거나 파일을 삭제할 수 없습니다.".into());
        }
        match self
            .remove_request
            .clone()
            .ok_or("닫거나 삭제할 문서가 없습니다.")?
        {
            RemoveRequest::Close(id) => self.close_entry(id),
            RemoveRequest::Delete(path) => {
                if self.loading.contains(&path) {
                    return Err("파일을 불러오고 있습니다. 완료 후 삭제해 주시기 바랍니다.".into());
                }
                storage::delete_png(&path).map_err(|e| e.to_string())?;
                let ids: Vec<_> = self
                    .entries
                    .iter()
                    .filter(|e| !e.generated && e.path == path)
                    .map(|e| e.id)
                    .collect();
                for id in ids {
                    self.close_entry(id);
                }
                self.refresh_browser(ctx);
                self.message("PNG 파일을 삭제했습니다.".into(), false);
            }
        }
        self.remove_request = None;
        Ok(())
    }

    pub(super) fn remove_modal(&mut self, ctx: &egui::Context) {
        let Some(request) = &self.remove_request else {
            return;
        };
        let mut confirm = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::new("remove-document")).show(ctx, |ui| {
            ui.set_width(420.0);
            match request {
                RemoveRequest::Close(id) => {
                    ui.heading("문서 닫기");
                    if let Some(entry) = self.entries.iter().find(|e| e.id == *id) { ui.label(display_name(&entry.path)); }
                    ui.label("저장하지 않은 변경 내용을 버리고 작업 목록에서 닫으시겠습니까? 디스크의 원본 파일은 삭제되지 않습니다.");
                }
                RemoveRequest::Delete(path) => {
                    ui.heading("PNG 파일 영구 삭제");
                    ui.label(display_name(path));
                    ui.label(egui::RichText::new(path.display().to_string()).small()).on_hover_text(path.display().to_string());
                    ui.colored_label(RED, "이 파일과 저장하지 않은 편집 내용을 삭제하시겠습니까? 휴지통으로 이동하지 않으며 실행 취소할 수 없습니다.");
                }
            }
            ui.horizontal(|ui| {
                confirm = command(ui, Lucide::Trash2, if matches!(request, RemoveRequest::Delete(_)) { "영구 삭제" } else { "변경 내용 버리고 닫기" }, false).clicked();
                cancel = command(ui, Lucide::X, "취소", true).clicked();
            });
        });
        if confirm && let Err(error) = self.confirm_remove(ctx) {
            self.message(error, true);
            self.remove_request = None;
        }
        if cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.remove_request = None;
        }
    }
}

fn frame_view(ui: &mut egui::Ui, dialog: &mut FrameDialog, height: f32) {
    let (bounds, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), height), egui::Sense::drag());
    let scale = ((bounds.width() - 36.0) / dialog.frame.width as f32)
        .min((bounds.height() - 36.0) / dialog.frame.height as f32);
    let crop = egui::Rect::from_center_size(
        bounds.center(),
        Vec2::new(dialog.frame.width as f32, dialog.frame.height as f32) * scale,
    );
    dialog.viewport = Some((crop, scale));
    if response.dragged() {
        let delta = response.drag_delta() / scale;
        dialog.frame.offset[0] += delta.x;
        dialog.frame.offset[1] += delta.y;
        dialog.dirty = true;
    }
    if response.hovered()
        && let Some(pointer) = response.hover_pos()
    {
        let scroll = ui.input_mut(|i| {
            let y = i.smooth_scroll_delta.y;
            i.smooth_scroll_delta.y = 0.0;
            y
        });
        if scroll != 0.0 {
            let anchor = (pointer - crop.min) / scale;
            dialog.frame.zoom_at(
                dialog.frame.scale * (scroll * 0.005).exp(),
                [anchor.x, anchor.y],
            );
            dialog.dirty = true;
        }
    }
    let cursor = if response.dragged() {
        egui::CursorIcon::Grabbing
    } else {
        egui::CursorIcon::Grab
    };
    response
        .on_hover_cursor(cursor)
        .on_hover_text("드래그하여 이동 · 마우스 휠로 확대/축소");
    let painter = ui.painter_at(bounds);
    checkerboard(&painter, bounds);
    let background = if dialog.mode == PaletteMode::Keep {
        dialog.source.palette[dialog.frame.background_index as usize]
    } else {
        dialog.frame.background
    };
    painter.rect_filled(
        crop,
        0.0,
        Color32::from_rgb(background[0], background[1], background[2]),
    );
    painter.image(
        dialog.texture.id(),
        egui::Rect::from_min_size(
            crop.min + Vec2::new(dialog.frame.offset[0], dialog.frame.offset[1]) * scale,
            Vec2::new(
                dialog.source.image.width() as f32,
                dialog.source.image.height() as f32,
            ) * dialog.frame.scale
                * scale,
        ),
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    for rect in [
        egui::Rect::from_min_max(bounds.min, egui::pos2(bounds.right(), crop.top())),
        egui::Rect::from_min_max(egui::pos2(bounds.left(), crop.bottom()), bounds.max),
        egui::Rect::from_min_max(egui::pos2(bounds.left(), crop.top()), crop.left_bottom()),
        egui::Rect::from_min_max(crop.right_top(), egui::pos2(bounds.right(), crop.bottom())),
    ] {
        painter.rect_filled(rect, 0.0, Color32::from_black_alpha(135));
    }
    painter.rect_stroke(
        crop,
        0.0,
        egui::Stroke::new(2.0, GREEN),
        egui::StrokeKind::Inside,
    );
    for ratio in [1.0 / 3.0, 2.0 / 3.0] {
        painter.line_segment(
            [
                egui::pos2(crop.left() + crop.width() * ratio, crop.top()),
                egui::pos2(crop.left() + crop.width() * ratio, crop.bottom()),
            ],
            egui::Stroke::new(0.5, Color32::from_white_alpha(120)),
        );
        painter.line_segment(
            [
                egui::pos2(crop.left(), crop.top() + crop.height() * ratio),
                egui::pos2(crop.right(), crop.top() + crop.height() * ratio),
            ],
            egui::Stroke::new(0.5, Color32::from_white_alpha(120)),
        );
    }
}
