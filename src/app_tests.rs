use super::*;

#[test]
fn reserved_palette_help_opens_over_label_without_changing_checkbox() {
    fn contains_help(shape: &egui::epaint::Shape) -> bool {
        match shape {
            egui::epaint::Shape::Text(text) => text.galley.text().contains("채팅 배경색"),
            egui::epaint::Shape::Vec(shapes) => shapes.iter().any(contains_help),
            _ => false,
        }
    }
    let ctx = egui::Context::default();
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        ctx.style_mut_of(theme, |style| style.interaction.tooltip_delay = 0.0);
    }
    let mut checked = true;
    let mut label_point = egui::Pos2::ZERO;
    let mut seen = false;
    for frame in 0..4 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(800.0, 600.0),
                )),
                time: Some(frame as f64),
                events: if frame == 0 {
                    vec![]
                } else {
                    vec![egui::Event::PointerMoved(label_point)]
                },
                ..Default::default()
            },
            |ui| {
                let response =
                    reserved_palette_checkbox(ui, &mut checked, true, "예약 색상 자동 보정");
                label_point = egui::pos2(response.rect.right() - 12.0, response.rect.center().y);
            },
        );
        seen |= output
            .shapes
            .iter()
            .any(|shape| contains_help(&shape.shape));
        output.textures_delta.clear();
    }
    assert!(seen, "hovering checkbox text should show the explanation");
    assert!(checked, "hovering must not toggle the option");
}

#[test]
fn algorithm_comparisons_match_selected_preview_and_applied_pixels() {
    let (ctx, mut app) = setup();
    let rgba = (0..128 * 48)
        .flat_map(|i| {
            [
                (i % 128 * 2) as u8,
                (i / 128 * 5) as u8,
                (i % 199) as u8,
                255,
            ]
        })
        .collect::<Vec<_>>();
    let source = Arc::new(engine::document_from_rgba(128, 48, &rgba).unwrap());
    app.open_framing(&ctx, source, None);
    let dialog = app.frame_dialog.as_mut().unwrap();
    dialog.author = "Test".into();
    dialog.frame.width = 128;
    dialog.frame.height = 48;
    dialog.frame.scale = 1.0;
    dialog.frame.offset = [0.0, 0.0];
    dialog.compare_algorithms = true;
    dialog.preserve_white = false;
    dialog.rebuild_comparisons(&ctx);
    assert_eq!(dialog.comparisons.len(), 4);
    assert!(dialog.comparisons.iter().all(|(_, result)| result.is_ok()));
    for (algorithm, result) in &dialog.comparisons {
        let preview = result.as_ref().unwrap();
        assert!(preview.doc.is_chat(), "{algorithm:?}");
        assert!(preview.doc.palette.len() <= 4);
    }
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1000.0, 740.0),
            )),
            ..Default::default()
        },
        |_| app.framing_modal(&ctx),
    );
    output.textures_delta.clear();
    let dialog = app.frame_dialog.as_mut().unwrap();
    let result = dialog.comparisons.last().unwrap().1.as_ref().unwrap();
    let expected = result.doc.image.clone();
    dialog.algorithm = Algorithm::ExoQuant;
    dialog.preview = Some(Preview {
        texture: result.texture.clone(),
        doc: Document::from_bytes(result.doc.bytes.clone()).unwrap(),
    });
    dialog.dirty = false;
    app.apply_framing(&ctx).unwrap();
    assert_eq!(app.entries.last().unwrap().effective_doc().image, expected);
}

fn four_color_import(app: &mut Studio, ctx: &egui::Context) -> Vec<u8> {
    let colors = [
        [255, 255, 255, 255],
        [192, 192, 192, 255],
        [128, 128, 128, 255],
        [0, 0, 0, 255],
    ];
    let rgba = (0..(128 * 48))
        .flat_map(|i| colors[i % 4])
        .collect::<Vec<_>>();
    let source = engine::document_from_rgba(128, 48, &rgba).unwrap();
    add(app, ctx, "ordinary.png", source);
    app.open_framing(ctx, app.entries[0].doc.clone(), Some(app.entries[0].id));
    let dialog = app.frame_dialog.as_mut().unwrap();
    dialog.author = "Test".into();
    dialog.initial_import = true;
    rgba
}

#[test]
fn low_color_import_waits_for_choice_and_preserves_exact_colors_through_save() {
    let (ctx, mut app) = setup();
    let rgba = four_color_import(&mut app, &ctx);
    app.request_apply_framing(&ctx).unwrap();
    assert_eq!(
        app.frame_dialog.as_ref().unwrap().color_confirmation,
        Some(4)
    );
    assert!(app.entries[0].painting.is_none());
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1000.0, 740.0),
            )),
            ..Default::default()
        },
        |_| app.framing_modal(&ctx),
    );
    output.textures_delta.clear();
    app.resolve_color_choice(&ctx, ColorChoice::Preserve)
        .unwrap();
    assert!(app.frame_dialog.is_none());
    let entry = &app.entries[0];
    assert!(!entry.auto_palette);
    assert_eq!(entry.effective_doc().image.as_raw(), &rgba);
    let saved = engine::process(entry.effective_doc(), &app.settings_for_entry(entry)).unwrap();
    assert_eq!(Document::from_bytes(saved).unwrap().image.as_raw(), &rgba);
}

#[test]
fn low_color_confirmation_back_keeps_options_and_current_keeps_correction_enabled() {
    let (ctx, mut app) = setup();
    four_color_import(&mut app, &ctx);
    app.request_apply_framing(&ctx).unwrap();
    app.resolve_color_choice(&ctx, ColorChoice::Back).unwrap();
    let dialog = app.frame_dialog.as_ref().unwrap();
    assert!(dialog.color_confirmation.is_none());
    assert!(dialog.fix_palette && dialog.preserve_white);
    assert_eq!(dialog.frame.sampling, Sampling::Smooth);
    assert!(app.entries[0].painting.is_none());
    app.request_apply_framing(&ctx).unwrap();
    app.resolve_color_choice(&ctx, ColorChoice::Current)
        .unwrap();
    assert!(app.entries[0].auto_palette);
    assert_ne!(
        app.entries[0].effective_doc().image.get_pixel(0, 0).0,
        [255, 255, 255, 255]
    );
}

#[test]
fn low_color_import_preserve_respects_game_size_error_without_replacing_source() {
    let (ctx, mut app) = setup();
    four_color_import(&mut app, &ctx);
    let before = app.entries[0].doc.bytes.clone();
    app.frame_dialog.as_mut().unwrap().frame.width = 96;
    app.request_apply_framing(&ctx).unwrap();
    assert!(
        app.resolve_color_choice(&ctx, ColorChoice::Preserve)
            .unwrap_err()
            .contains("128")
    );
    assert_eq!(app.entries[0].doc.bytes, before);
    assert!(app.entries[0].painting.is_none());
    let dialog = app.frame_dialog.as_ref().unwrap();
    assert!(dialog.color_confirmation.is_none());
    assert!(!dialog.fix_palette && !dialog.preserve_white);
}

#[test]
fn clipboard_low_color_import_prompts_but_existing_chat_and_five_colors_do_not() {
    let (ctx, mut app) = setup();
    four_color_import(&mut app, &ctx);
    app.frame_dialog.as_mut().unwrap().target = None;
    app.request_apply_framing(&ctx).unwrap();
    assert_eq!(
        app.frame_dialog.as_ref().unwrap().color_confirmation,
        Some(4)
    );
    app.resolve_color_choice(&ctx, ColorChoice::Preserve)
        .unwrap();
    assert_eq!(app.entries.len(), 2);
    assert!(!app.entries[1].auto_palette);
    let doc = app.entries[1].effective_doc().clone();
    app.open_framing(&ctx, doc, Some(app.entries[1].id));
    app.request_apply_framing(&ctx).unwrap();
    assert!(app.frame_dialog.is_none());
    let rgba = (0..(128 * 48))
        .flat_map(|i| [(i % 5 * 40) as u8, 50, 80, 255])
        .collect::<Vec<_>>();
    let doc = engine::document_from_rgba(128, 48, &rgba).unwrap();
    app.open_framing(&ctx, Arc::new(doc), None);
    let dialog = app.frame_dialog.as_mut().unwrap();
    dialog.frame.width = 128;
    dialog.frame.height = 48;
    dialog.frame.scale = 1.0;
    dialog.frame.offset = [0.0, 0.0];
    dialog.author = "Test".into();
    app.request_apply_framing(&ctx).unwrap();
    assert!(app.frame_dialog.is_none());
}

#[test]
fn update_events_leave_document_and_edit_status_unchanged() {
    let (ctx, mut app) = setup();
    add(&mut app, &ctx, "source.png", chat());
    let before = app.entries[0].effective_doc().bytes.clone();
    let status = app.status.clone();
    app.tx
        .send(Event::UpdateChecked(Err("offline".into())))
        .unwrap();
    app.receive(&ctx);
    assert_eq!(app.status, status);
    assert!(!app.error);
    assert!(app.update_release.is_none());
    assert!(app.logs.iter().any(|log| log.contains("업데이트")));
    let release = update::Release {
        version: "9.0.0".parse().unwrap(),
        url: "https://github.com/mabinewb/mabipng-studio/releases/tag/v9.0.0".into(),
    };
    app.tx
        .send(Event::UpdateChecked(Ok(Some(release.clone()))))
        .unwrap();
    app.receive(&ctx);
    assert_eq!(app.update_release, Some(release));
    assert_eq!(app.entries[0].effective_doc().bytes, before);
    assert_eq!(app.status, status);
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1000.0, 740.0),
            )),
            ..Default::default()
        },
        |ui| {
            app.header(ui, &ctx);
            app.update_notice(ui);
        },
    );
    output.textures_delta.clear();
}

#[test]
fn saving_four_color_narrow_chat_is_blocked_until_repaired_to_three_colors() {
    let (ctx, mut app) = setup();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source.png");
    let source = engine::new_chat_sized(96, 1, "Test").unwrap();
    let pixels = (0..96).map(|i| (i % 4) as u8).collect::<Vec<_>>();
    let bytes = engine::paint_indexed(&source, &pixels).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    add(
        &mut app,
        &ctx,
        path.to_str().unwrap(),
        Document::from_bytes(bytes.clone()).unwrap(),
    );
    app.save_current_as(&ctx, SaveChoice::Existing);
    assert_eq!(app.pending, 0);
    assert!(app.error && app.status.contains("128"));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    app.ensure_painting(&ctx).unwrap();
    for pixel in &mut app.entries[0].painting.as_mut().unwrap().canvas.pixels {
        if *pixel == 3 {
            *pixel = 2;
        }
    }
    sync_paint(&mut app.entries[0], &ctx).unwrap();
    app.save_current_as(&ctx, SaveChoice::Existing);
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.pending > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        app.receive(&ctx);
    }
    assert_eq!(app.save_ok, 1);
    assert_eq!(
        engine::validate_game_size(&Document::load(&path).unwrap()).unwrap(),
        3
    );
}

#[test]
fn applying_four_color_narrow_conversion_requires_wider_canvas() {
    let (ctx, mut app) = setup();
    let source = engine::new_chat_sized(96, 1, "Test").unwrap();
    let source = Document::from_bytes(
        engine::paint_indexed(&source, &(0..96).map(|i| (i % 4) as u8).collect::<Vec<_>>())
            .unwrap(),
    )
    .unwrap();
    add(&mut app, &ctx, "source.png", source);
    let before = app.entries[0].effective_doc().bytes.clone();
    app.open_canvas_resize(&ctx);
    assert!(app.apply_framing(&ctx).unwrap_err().contains("128"));
    assert_eq!(app.entries[0].effective_doc().bytes, before);
    app.frame_dialog.as_mut().unwrap().frame.set_size(128, 1);
    app.apply_framing(&ctx).unwrap();
    assert_eq!(
        engine::validate_game_size(app.entries[0].effective_doc()).unwrap(),
        4
    );
}

#[test]
fn short_blank_canvas_and_resize_keep_requested_height() {
    let (ctx, mut app) = setup();
    app.create_blank_sized(&ctx, 96, 1, "Test").unwrap();
    assert_eq!(app.entries[0].effective_doc().image.dimensions(), (96, 1));
    app.open_canvas_resize(&ctx);
    assert_eq!(app.frame_dialog.as_ref().unwrap().frame.height, 1);
    app.apply_framing(&ctx).unwrap();
    assert_eq!(app.entries[0].effective_doc().image.dimensions(), (96, 1));
}

#[test]
fn loaded_ordinary_png_opens_conversion_before_editing_and_cannot_undo_to_raw_source() {
    let (ctx, mut app) = setup();
    app.author = "Test".into();
    let source = ordinary();
    let original = source.bytes.clone();
    app.tx
        .send(Event::Loaded(PathBuf::from("input.png"), Ok(source)))
        .unwrap();
    app.receive(&ctx);
    assert!(app.frame_dialog.as_ref().unwrap().initial_import);
    assert!(app.entries[0].painting.is_none());
    assert!(!app.entries[0].effective_doc().is_chat());
    assert_eq!(app.frame_dialog.as_ref().unwrap().frame.width, 32);
    app.apply_framing(&ctx).unwrap();
    assert_eq!(app.view, 3);
    assert!(app.entries[0].effective_doc().is_chat());
    assert_eq!(app.entries[0].doc.bytes, original);
    assert!(app.entries[0].undo.is_empty());
    app.paint_history(&ctx, false);
    assert!(app.entries[0].effective_doc().is_chat());
}

#[test]
fn loaded_chat_skips_conversion_and_multiple_raw_imports_are_queued() {
    let (ctx, mut app) = setup();
    app.tx
        .send(Event::Loaded(PathBuf::from("existing.png"), Ok(chat())))
        .unwrap();
    app.receive(&ctx);
    assert!(app.frame_dialog.is_none());
    for path in ["first.png", "second.png"] {
        app.tx
            .send(Event::Loaded(PathBuf::from(path), Ok(ordinary())))
            .unwrap();
    }
    app.receive(&ctx);
    let first = app.frame_dialog.as_ref().unwrap().target.unwrap();
    assert_eq!(app.entries.len(), 3);
    app.cancel_framing();
    assert!(!app.entries.iter().any(|e| e.id == first));
    app.receive(&ctx);
    let second = app.frame_dialog.as_ref().unwrap().target.unwrap();
    assert_ne!(first, second);
    app.cancel_framing();
    assert_eq!(app.entries.len(), 1);
    assert!(app.entries[0].effective_doc().is_chat());
}

#[test]
fn batch_conversion_is_separate_from_open_unsaved_documents() {
    let (ctx, mut app) = setup();
    app.create_blank(&ctx, 128, "Editor").unwrap();
    app.set_palette_color(&ctx, 0, [12, 34, 56]).unwrap();
    let editing = app.entries[0].effective_doc().bytes.clone();
    let selected = app.selected;
    let root = tempfile::tempdir().unwrap();
    let one = root.path().join("ordinary.png");
    let two = root.path().join("existing.png");
    std::fs::write(&one, ordinary().bytes).unwrap();
    std::fs::write(&two, chat().bytes).unwrap();
    app.open_batch(&ctx);
    app.add_batch_paths(vec![
        one.clone(),
        two.clone(),
        one,
        root.path().join("ignored.txt"),
    ]);
    assert_eq!(app.batch_dialog.as_ref().unwrap().paths.len(), 2);
    let dialog = app.batch_dialog.as_mut().unwrap();
    dialog.options.output_dir = root.path().join("output");
    dialog.options.author = "Batch".into();
    app.start_batch(&ctx);
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.batch_dialog.as_ref().unwrap().running && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        app.receive(&ctx);
    }
    let dialog = app.batch_dialog.as_ref().unwrap();
    assert!(!dialog.running);
    assert_eq!(dialog.results.len(), 2);
    assert!(dialog.results.iter().all(|r| r.output.is_ok()));
    assert_eq!(
        Document::load(dialog.results[0].output.as_ref().unwrap())
            .unwrap()
            .image
            .dimensions(),
        (256, 96)
    );
    assert_eq!(
        Document::load(dialog.results[1].output.as_ref().unwrap())
            .unwrap()
            .author
            .as_deref(),
        Some("기존닉")
    );
    assert_eq!(app.entries.len(), 1);
    assert_eq!(app.selected, selected);
    assert_eq!(app.entries[0].effective_doc().bytes, editing);
    assert!(app.entries[0].has_unsaved());
    assert!(app.frame_dialog.is_none());
}

#[test]
fn batch_reports_each_error_and_continues_without_overwriting_sources() {
    let (ctx, mut app) = setup();
    let root = tempfile::tempdir().unwrap();
    let valid = root.path().join("valid.png");
    let original = chat().bytes;
    std::fs::write(&valid, &original).unwrap();
    app.open_batch(&ctx);
    app.add_batch_paths(vec![root.path().join("missing.png"), valid.clone()]);
    app.batch_dialog.as_mut().unwrap().options.output_dir = root.path().join("results");
    app.start_batch(&ctx);
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.batch_dialog.as_ref().unwrap().running && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        app.receive(&ctx);
    }
    let results = &app.batch_dialog.as_ref().unwrap().results;
    assert_eq!(results.len(), 2);
    assert!(results[0].output.is_err());
    assert!(results[1].output.is_ok());
    assert_eq!(std::fs::read(&valid).unwrap(), original);
}

#[test]
fn batch_modal_blocks_underlying_paint_and_single_save() {
    let (ctx, mut app) = setup();
    app.create_blank(&ctx, 128, "Test").unwrap();
    drawing_frame(&ctx, &mut app, vec![]);
    let (rect, _) = app.entries[0].painting.as_ref().unwrap().viewport.unwrap();
    app.open_batch(&ctx);
    let before = app.entries[0].effective_doc().bytes.clone();
    drawing_frame(
        &ctx,
        &mut app,
        vec![button(rect.center(), true), button(rect.center(), false)],
    );
    assert_eq!(app.entries[0].effective_doc().bytes, before);
    app.request_save(&ctx);
    assert!(app.save_dialog.is_none());
    assert_eq!(app.pending, 0);
}

#[test]
fn canvas_framing_auto_palette_off_restores_default_colors_not_custom_colors() {
    let (ctx, mut app) = setup();
    app.create_blank(&ctx, 128, "Test").unwrap();
    app.set_palette_color(&ctx, 2, [10, 20, 30]).unwrap();
    app.open_canvas_resize(&ctx);
    app.frame_dialog.as_mut().unwrap().fix_palette = false;
    app.apply_framing(&ctx).unwrap();
    assert_eq!(
        app.entries[0].effective_doc().palette,
        [[255; 3], [192; 3], [10, 20, 30], [0; 3]]
    );
    assert!(!app.fix_palette);
}

#[test]
fn resized_chat_can_overwrite_original_and_undo_after_save_restores_old_dimensions() {
    let (ctx, mut app) = setup();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source.png");
    let doc = engine::new_chat(256, "Test").unwrap();
    std::fs::write(&path, &doc.bytes).unwrap();
    add(&mut app, &ctx, path.to_str().unwrap(), doc);
    app.open_canvas_resize(&ctx);
    let dialog = app.frame_dialog.as_mut().unwrap();
    dialog.frame.set_size(129, 49);
    dialog.frame.fit(dialog.source.image.dimensions(), true);
    app.apply_framing(&ctx).unwrap();
    app.save_current_as(&ctx, SaveChoice::Existing);
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.pending > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        app.receive(&ctx);
    }
    assert_eq!(app.save_ok, 1, "{:?}", app.logs);
    assert_eq!(Document::load(&path).unwrap().image.dimensions(), (129, 49));
    assert_eq!(
        (
            app.entries[0].painting.as_ref().unwrap().canvas.width,
            app.entries[0].painting.as_ref().unwrap().canvas.height
        ),
        (129, 49)
    );
    app.paint_history(&ctx, false);
    assert_eq!(app.entries[0].effective_doc().image.dimensions(), (256, 96));
    assert_eq!(Document::load(&path).unwrap().image.dimensions(), (129, 49));
    assert!(app.entries[0].has_unsaved());
}

#[test]
fn clipboard_result_waits_until_save_or_other_dialog_finishes() {
    let (ctx, mut app) = setup();
    app.pending = 1;
    app.tx.send(Event::Clipboard(Ok(ordinary()))).unwrap();
    app.receive(&ctx);
    assert!(app.frame_dialog.is_none());
    assert!(app.clipboard_waiting.is_some());
    app.pending = 0;
    app.show_new = true;
    app.receive(&ctx);
    assert!(app.frame_dialog.is_none());
    app.show_new = false;
    app.receive(&ctx);
    assert!(app.clipboard_waiting.is_none());
    assert!(app.frame_dialog.is_some());
}

#[test]
fn clipboard_framing_cancel_does_not_replace_or_change_current_document() {
    let (ctx, mut app) = setup();
    app.create_blank(&ctx, 128, "Test").unwrap();
    let id = app.selected;
    let before = app.entries[0].effective_doc().bytes.clone();
    let raw = engine::document_from_rgba(500, 300, &[80, 120, 160, 255].repeat(500 * 300)).unwrap();
    app.tx.send(Event::Clipboard(Ok(raw))).unwrap();
    app.receive(&ctx);
    assert!(app.frame_dialog.is_some());
    assert_eq!(app.entries.len(), 1);
    assert_eq!(app.selected, id);
    app.frame_dialog = None;
    assert_eq!(app.entries[0].effective_doc().bytes, before);
    assert_eq!(app.author, "Test");
}

#[test]
fn canvas_resize_is_one_history_step_and_saved_size_matches_pixels() {
    let (ctx, mut app) = setup();
    app.create_blank(&ctx, 256, "Test").unwrap();
    app.ensure_painting(&ctx).unwrap();
    app.entries[0].painting.as_mut().unwrap().canvas.pixels[10 * 256 + 20] = 3;
    sync_paint(&mut app.entries[0], &ctx).unwrap();
    let old = app.entries[0].effective_doc().bytes.clone();
    let count = app.entries[0].undo.len();
    app.open_canvas_resize(&ctx);
    let d = app.frame_dialog.as_mut().unwrap();
    assert_eq!(d.mode, PaletteMode::Keep);
    d.frame.set_size(128, 48);
    d.frame.offset = [-20.0, -10.0];
    app.apply_framing(&ctx).unwrap();
    assert_eq!(app.entries[0].undo.len(), count + 1);
    assert_eq!(app.entries[0].painting.as_ref().unwrap().canvas.width, 128);
    assert_eq!(
        engine::indexed_pixels(app.entries[0].effective_doc()).unwrap()[0],
        3
    );
    let output = engine::process(
        app.entries[0].effective_doc(),
        &app.settings_for_entry(&app.entries[0]),
    )
    .unwrap();
    assert_eq!(
        Document::from_bytes(output).unwrap().image.dimensions(),
        (128, 48)
    );
    app.paint_history(&ctx, false);
    assert_eq!(app.entries[0].effective_doc().bytes, old);
    assert_eq!(app.entries[0].painting.as_ref().unwrap().canvas.width, 256);
    app.paint_history(&ctx, true);
    assert_eq!(app.entries[0].effective_doc().image.dimensions(), (128, 48));
}

#[test]
fn framing_validation_and_save_lock_leave_editing_document_untouched() {
    let (ctx, mut app) = setup();
    app.create_blank(&ctx, 128, "Test").unwrap();
    let original = app.entries[0].effective_doc().bytes.clone();
    app.open_canvas_resize(&ctx);
    app.frame_dialog.as_mut().unwrap().frame.width = 0;
    assert!(app.apply_framing(&ctx).is_err());
    assert!(app.frame_dialog.is_some());
    assert_eq!(app.entries[0].effective_doc().bytes, original);
    app.frame_dialog.as_mut().unwrap().frame.width = 128;
    app.pending = 1;
    assert!(app.apply_framing(&ctx).is_err());
    assert_eq!(app.entries[0].effective_doc().bytes, original);
    app.pending = 0;
    app.frame_dialog.as_mut().unwrap().author = "invalid\nname".into();
    assert!(app.apply_framing(&ctx).is_err());
    assert_eq!(app.entries[0].effective_doc().bytes, original);
}

#[test]
fn close_unsaved_document_requires_confirmation_and_keeps_disk_file() {
    let (ctx, mut app) = setup();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source.png");
    let doc = chat();
    std::fs::write(&path, &doc.bytes).unwrap();
    add(&mut app, &ctx, path.to_str().unwrap(), doc);
    let id = app.entries[0].id;
    app.author = "Changed".into();
    app.update_author();
    app.request_close_entry(&ctx, id);
    assert!(matches!(app.remove_request, Some(RemoveRequest::Close(_))));
    assert_eq!(app.entries.len(), 1);
    app.remove_request = None;
    assert!(path.is_file());
    app.request_close_entry(&ctx, id);
    app.confirm_remove(&ctx).unwrap();
    assert!(app.entries.is_empty());
    assert!(app.selected.is_none());
    assert!(path.is_file());
}

#[test]
fn disk_delete_is_explicit_and_save_lock_blocks_it() {
    let (ctx, mut app) = setup();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source.png");
    let neighbor = root.path().join("neighbor.png");
    let doc = chat();
    std::fs::write(&path, &doc.bytes).unwrap();
    std::fs::write(&neighbor, &doc.bytes).unwrap();
    add(&mut app, &ctx, path.to_str().unwrap(), doc);
    app.remove_request = Some(RemoveRequest::Delete(path.clone()));
    assert!(path.is_file());
    app.pending = 1;
    assert!(app.confirm_remove(&ctx).is_err());
    assert!(path.is_file());
    app.pending = 0;
    app.confirm_remove(&ctx).unwrap();
    assert!(!path.exists());
    assert!(neighbor.is_file());
    assert!(app.entries.is_empty());
}

#[test]
fn framing_modal_renders_and_drag_changes_output_space_position() {
    let (ctx, mut app) = setup();
    app.create_blank(&ctx, 256, "Test").unwrap();
    app.open_canvas_resize(&ctx);
    let run = |app: &mut Studio, events| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1000.0, 740.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |_| {
                    app.framing_modal(&ctx);
                });
            },
        );
        output.textures_delta.clear();
    };
    for _ in 0..3 {
        run(&mut app, vec![]);
    }
    assert!(app.frame_dialog.as_ref().unwrap().preview.is_some());
    let (rect, scale) = app.frame_dialog.as_ref().unwrap().viewport.unwrap();
    assert!(rect.width() > 100.0 && rect.height() > 40.0);
    let start = rect.center();
    run(
        &mut app,
        vec![egui::Event::PointerMoved(start), button(start, true)],
    );
    run(
        &mut app,
        vec![egui::Event::PointerMoved(start + Vec2::new(20.0, 10.0))],
    );
    let offset = app.frame_dialog.as_ref().unwrap().frame.offset;
    assert!((offset[0] - 20.0 / scale).abs() < 0.1, "{offset:?}");
    assert!((offset[1] - 10.0 / scale).abs() < 0.1);
    run(&mut app, vec![button(start + Vec2::new(20.0, 10.0), false)]);
    assert!(app.frame_dialog.as_ref().unwrap().preview.is_some());
}

#[test]
fn auto_palette_is_reversible_and_keeps_custom_colors_and_pixels() {
    let (ctx, mut app) = setup();
    app.create_blank(&ctx, 96, "테스트").unwrap();
    app.set_palette_color(&ctx, 2, [12, 34, 56]).unwrap();
    let before = engine::indexed_pixels(app.entries[0].effective_doc()).unwrap();
    app.fix_palette = false;
    app.apply_auto_palette(&ctx).unwrap();
    assert_eq!(
        app.entries[0].effective_doc().palette,
        [[255; 3], [192; 3], [12, 34, 56], [0; 3]]
    );
    assert_eq!(
        engine::indexed_pixels(app.entries[0].effective_doc()).unwrap(),
        before
    );
    app.paint_history(&ctx, false);
    assert!(app.fix_palette);
    assert_eq!(app.entries[0].effective_doc().palette[0], [254; 3]);
    app.paint_history(&ctx, true);
    assert!(!app.fix_palette);
    app.fix_palette = true;
    app.apply_auto_palette(&ctx).unwrap();
    assert_eq!(app.entries[0].effective_doc().palette[2], [12, 34, 56]);
    app.fix_palette = false;
    app.create_blank(&ctx, 96, "테스트").unwrap();
    assert_eq!(app.entries[1].effective_doc().palette, RAW_PALETTE);
}

#[test]
fn author_palette_and_pixels_share_one_history_and_clear_redo_on_edit() {
    let (ctx, mut app) = setup();
    add(&mut app, &ctx, "synthetic.png", chat());
    app.ensure_painting(&ctx).unwrap();
    let original = app.entries[0].effective_doc().palette.clone();
    app.author = "변경작성자".into();
    app.update_author();
    app.author = "변경작성자2".into();
    app.update_author();
    assert_eq!(app.entries[0].undo.len(), 1);
    app.set_palette_color(&ctx, 0, [11, 22, 33]).unwrap();
    app.entries[0].painting.as_mut().unwrap().canvas.pixels[0] = 2;
    sync_paint(&mut app.entries[0], &ctx).unwrap();
    app.paint_history(&ctx, false);
    assert_eq!(
        engine::indexed_pixels(app.entries[0].effective_doc()).unwrap()[0],
        0
    );
    assert_eq!(app.entries[0].effective_doc().palette[0], [11, 22, 33]);
    app.paint_history(&ctx, false);
    assert_eq!(app.entries[0].effective_doc().palette, original);
    assert_eq!(app.author, "변경작성자2");
    app.paint_history(&ctx, false);
    assert_eq!(app.author, "기존닉");
    app.paint_history(&ctx, true);
    assert_eq!(app.author, "변경작성자2");
    app.set_palette_color(&ctx, 0, [45, 67, 89]).unwrap();
    assert!(app.entries[0].redo.is_empty());
}

#[test]
fn selection_copy_paste_delete_and_move_are_undoable() {
    let (ctx, mut app) = setup();
    add(&mut app, &ctx, "synthetic.png", chat());
    app.ensure_painting(&ctx).unwrap();
    let p = app.entries[0].painting.as_mut().unwrap();
    p.canvas.pixels[0..3].copy_from_slice(&[1, 2, 3]);
    sync_paint(&mut app.entries[0], &ctx).unwrap();
    app.entries[0].painting.as_mut().unwrap().selection = Some(Region {
        x: 0,
        y: 0,
        width: 3,
        height: 1,
    });
    app.copy_selection(&ctx);
    app.entries[0].painting.as_mut().unwrap().selection = Some(Region {
        x: 4,
        y: 0,
        width: 3,
        height: 1,
    });
    app.paste_selection(&ctx);
    assert_eq!(
        &app.entries[0].painting.as_ref().unwrap().canvas.pixels[4..7],
        &[1, 2, 3]
    );
    app.delete_selection(&ctx);
    assert_eq!(
        &app.entries[0].painting.as_ref().unwrap().canvas.pixels[4..7],
        &[0, 0, 0]
    );
    app.paint_history(&ctx, false);
    assert_eq!(
        &app.entries[0].painting.as_ref().unwrap().canvas.pixels[4..7],
        &[1, 2, 3]
    );
    let p = app.entries[0].painting.as_mut().unwrap();
    p.selection = Some(Region {
        x: 0,
        y: 0,
        width: 3,
        height: 1,
    });
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(320.0, 160.0));
    tool_events(
        p,
        &[
            button(egui::pos2(5.0, 5.0), true),
            button(egui::pos2(15.0, 15.0), false),
        ],
        rect,
        rect,
        Tool::Select,
        (1, false, 1, false),
    );
    app.finish_strokes(&ctx);
    let pixels = engine::indexed_pixels(app.entries[0].effective_doc()).unwrap();
    assert_eq!(&pixels[..3], &[0, 0, 0]);
    assert_eq!(&pixels[33..36], &[1, 2, 3]);
    app.paint_history(&ctx, false);
    assert_eq!(
        &engine::indexed_pixels(app.entries[0].effective_doc()).unwrap()[..3],
        &[1, 2, 3]
    );
}

#[test]
fn shape_drag_previews_then_commits_once_and_escape_cancels() {
    let (ctx, mut app) = setup();
    add(&mut app, &ctx, "synthetic.png", chat());
    app.ensure_painting(&ctx).unwrap();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(320.0, 160.0));
    let p = app.entries[0].painting.as_mut().unwrap();
    tool_events(
        p,
        &[
            button(egui::pos2(15.0, 15.0), true),
            egui::Event::PointerMoved(egui::pos2(65.0, 65.0)),
        ],
        rect,
        rect,
        Tool::Rectangle,
        (2, false, 1, false),
    );
    assert_eq!(p.canvas.pixels[33], 2);
    assert_eq!(engine::indexed_pixels(&p.doc).unwrap()[33], 0);
    tool_events(
        p,
        &[button(egui::pos2(45.0, 45.0), false)],
        rect,
        rect,
        Tool::Rectangle,
        (2, false, 1, false),
    );
    app.finish_strokes(&ctx);
    assert_eq!(app.entries[0].undo.len(), 1);
    assert_eq!(
        engine::indexed_pixels(app.entries[0].effective_doc()).unwrap()[6 * 32 + 6],
        0
    );
    let p = app.entries[0].painting.as_mut().unwrap();
    let before = p.canvas.pixels.clone();
    tool_events(
        p,
        &[
            button(egui::pos2(5.0, 5.0), true),
            egui::Event::PointerMoved(egui::pos2(95.0, 95.0)),
            egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            },
        ],
        rect,
        rect,
        Tool::Ellipse,
        (3, true, 1, false),
    );
    assert_eq!(p.canvas.pixels, before);
    assert!(!p.active);
}

#[test]
fn different_palette_paste_requires_preview_confirmation() {
    let (ctx, mut app) = setup();
    app.create_blank(&ctx, 96, "테스트").unwrap();
    app.ensure_painting(&ctx).unwrap();
    app.fragment = Some(Fragment {
        region: Region {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        },
        pixels: vec![0],
        palette: vec![[0, 0, 0, 255]],
    });
    app.request_selection_paste(&ctx);
    assert!(app.paste_preview.is_some());
    assert_eq!(
        app.entries[0].painting.as_ref().unwrap().canvas.pixels[0],
        0
    );
    app.paste_preview = None;
    app.paste_selection(&ctx);
    assert_eq!(
        app.entries[0].painting.as_ref().unwrap().canvas.pixels[0],
        3
    );
    app.paint_history(&ctx, false);
    assert_eq!(
        app.entries[0].painting.as_ref().unwrap().canvas.pixels[0],
        0
    );
}

fn ordinary() -> Document {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 32, 16);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        let pixels: Vec<u8> = (0..512)
            .flat_map(|i| {
                if i % 32 < 8 {
                    [20, 50, 70]
                } else {
                    [255, 255, 255]
                }
            })
            .collect();
        writer.write_image_data(&pixels).unwrap();
    }
    Document::from_bytes(bytes).unwrap()
}

fn chat() -> Document {
    chat_sized(32, 16)
}

fn chat_sized(width: u32, height: u32) -> Document {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Indexed);
        encoder.set_depth(png::BitDepth::Two);
        encoder.set_palette(vec![255, 255, 250, 20, 50, 70, 70, 180, 190, 180, 200, 230]);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&vec![0; width.div_ceil(4) as usize * height as usize])
            .unwrap();
    }
    let doc = Document::from_bytes(bytes).unwrap();
    Document::from_bytes(
        engine::process(
            &doc,
            &Settings {
                mode: Mode::Author,
                author: Some("기존닉".into()),
                ..Default::default()
            },
        )
        .unwrap(),
    )
    .unwrap()
}

fn setup() -> (egui::Context, Studio) {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let app = Studio::new(&cc, Vec::new(), None, false, None);
    (ctx, app)
}

fn add(app: &mut Studio, ctx: &egui::Context, name: &str, doc: Document) {
    app.next_id += 1;
    let id = app.next_id;
    let image = texture(ctx, name, &doc.image);
    app.entries.push(Entry {
        id,
        path: PathBuf::from(name),
        doc: Arc::new(doc),
        texture: image,
        status: "대기".into(),
        failed: false,
        output: None,
        painting: None,
        draft: false,
        generated: false,
        author_override: None,
        undo: Vec::new(),
        redo: Vec::new(),
        auto_palette: true,
        author_edit: false,
    });
    app.selected = Some(id);
}

fn drawing_frame(ctx: &egui::Context, app: &mut Studio, events: Vec<egui::Event>) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            Vec2::new(700.0, 500.0),
        )),
        events,
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            app.drawing(ui, ctx, 280.0);
        });
        if !ctx.input(|input| input.pointer.primary_down()) {
            app.finish_strokes(ctx);
        }
    });
    output.textures_delta.clear();
}

fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    }
}

#[test]
fn pointer_stroke_edits_palette_indices_and_undo_redo_reaches_saved_bytes() {
    let (ctx, mut app) = setup();
    add(&mut app, &ctx, "existing.png", chat());
    app.paint_color = 1;
    app.brush_size = 1;
    for _ in 0..3 {
        drawing_frame(&ctx, &mut app, vec![]);
    }
    let (rect, scale) = app.entries[0].painting.as_ref().unwrap().viewport.unwrap();
    let start = rect.min + Vec2::new(2.5, 3.5) * scale;
    let middle = rect.min + Vec2::new(10.5, 3.5) * scale;
    let end = rect.min + Vec2::new(13.5, 3.5) * scale;
    drawing_frame(
        &ctx,
        &mut app,
        vec![egui::Event::PointerMoved(start), button(start, true)],
    );
    drawing_frame(&ctx, &mut app, vec![egui::Event::PointerMoved(middle)]);
    drawing_frame(
        &ctx,
        &mut app,
        vec![egui::Event::PointerMoved(end), button(end, false)],
    );
    let painted = engine::indexed_pixels(app.entries[0].effective_doc()).unwrap();
    for x in 2..=13 {
        assert_eq!(painted[3 * 32 + x], 1, "missing brush pixel {x}");
    }
    assert_eq!(
        app.entries[0].effective_doc().author.as_deref(),
        Some("기존닉")
    );
    assert!(app.entries[0].painting.as_ref().unwrap().unsaved);
    app.paint_history(&ctx, false);
    assert!(
        engine::indexed_pixels(app.entries[0].effective_doc())
            .unwrap()
            .iter()
            .all(|&v| v == 0)
    );
    app.paint_history(&ctx, true);
    assert_eq!(
        engine::indexed_pixels(app.entries[0].effective_doc()).unwrap(),
        painted
    );
    let output = engine::process(
        app.entries[0].effective_doc(),
        &app.settings_for(app.entries[0].effective_doc()),
    )
    .unwrap();
    let saved = Document::from_bytes(output).unwrap();
    assert_eq!(engine::indexed_pixels(&saved).unwrap(), painted);
    assert_eq!(saved.author.as_deref(), Some("기존닉"));
}

#[test]
fn quick_press_release_in_one_frame_paints_one_stroke() {
    let (ctx, mut app) = setup();
    add(&mut app, &ctx, "existing.png", chat());
    app.paint_color = 2;
    app.brush_size = 1;
    for _ in 0..3 {
        drawing_frame(&ctx, &mut app, vec![]);
    }
    let (rect, scale) = app.entries[0].painting.as_ref().unwrap().viewport.unwrap();
    let pos = rect.min + Vec2::new(4.5, 5.5) * scale;
    drawing_frame(
        &ctx,
        &mut app,
        vec![
            egui::Event::PointerMoved(pos),
            button(pos, true),
            button(pos, false),
        ],
    );
    assert_eq!(
        engine::indexed_pixels(app.entries[0].effective_doc()).unwrap()[5 * 32 + 4],
        2
    );
    app.paint_history(&ctx, false);
    assert_eq!(
        engine::indexed_pixels(app.entries[0].effective_doc()).unwrap()[5 * 32 + 4],
        0
    );
}

#[test]
fn batched_drag_retains_press_position_and_whole_segment() {
    let (ctx, mut app) = setup();
    add(&mut app, &ctx, "existing.png", chat());
    app.paint_color = 1;
    app.brush_size = 1;
    for _ in 0..3 {
        drawing_frame(&ctx, &mut app, vec![]);
    }
    let (rect, scale) = app.entries[0].painting.as_ref().unwrap().viewport.unwrap();
    let start = rect.min + Vec2::new(2.5, 5.5) * scale;
    let end = rect.min + Vec2::new(18.5, 5.5) * scale;
    drawing_frame(
        &ctx,
        &mut app,
        vec![
            egui::Event::PointerMoved(start),
            button(start, true),
            egui::Event::PointerMoved(end),
            button(end, false),
        ],
    );
    let painted = engine::indexed_pixels(app.entries[0].effective_doc()).unwrap();
    for x in 2..=18 {
        assert_eq!(painted[5 * 32 + x], 1);
    }
    app.paint_history(&ctx, false);
    assert!(
        engine::indexed_pixels(app.entries[0].effective_doc())
            .unwrap()
            .iter()
            .all(|&v| v == 0)
    );
}

#[test]
fn two_taps_with_noop_final_stroke_still_commit_visible_pixels() {
    let (ctx, mut app) = setup();
    add(&mut app, &ctx, "existing.png", chat());
    app.paint_color = 1;
    app.brush_size = 1;
    for _ in 0..3 {
        drawing_frame(&ctx, &mut app, vec![]);
    }
    let (rect, scale) = app.entries[0].painting.as_ref().unwrap().viewport.unwrap();
    let pos = rect.min + Vec2::new(4.5, 5.5) * scale;
    drawing_frame(
        &ctx,
        &mut app,
        vec![
            egui::Event::PointerMoved(pos),
            button(pos, true),
            button(pos, false),
            button(pos, true),
            button(pos, false),
        ],
    );
    assert_eq!(
        engine::indexed_pixels(app.entries[0].effective_doc()).unwrap()[5 * 32 + 4],
        1
    );
    assert!(app.entries[0].painting.as_ref().unwrap().unsaved);
    app.paint_history(&ctx, false);
    assert_eq!(
        engine::indexed_pixels(app.entries[0].effective_doc()).unwrap()[5 * 32 + 4],
        0
    );
}

#[test]
fn default_save_folder_uses_windows_documents_location() {
    let (_, app) = setup();
    let documents = dirs::document_dir().expect("Documents known folder");
    assert_eq!(
        app.output_dir,
        Some(documents.join("마비노기").join("그림대화"))
    );
}

#[test]
fn wide_painting_uses_native_pixel_tiles() {
    let (ctx, mut app) = setup();
    add(&mut app, &ctx, "wide.png", chat_sized(2048, 8));
    app.zoom = 1.0;
    for _ in 0..3 {
        drawing_frame(&ctx, &mut app, vec![]);
    }
    let painting = app.entries[0].painting.as_mut().unwrap();
    painting.canvas.begin_stroke();
    painting.canvas.stroke_to(257, 3, 1, Shape::Square, 1);
    painting.active = true;
    app.finish_strokes(&ctx);
    drawing_frame(&ctx, &mut app, vec![]);
    let painting = app.entries[0].painting.as_ref().unwrap();
    assert_eq!(painting.tiles.get(&(256, 0)).unwrap().size(), [256, 8]);
    assert_eq!(
        engine::indexed_pixels(app.entries[0].effective_doc()).unwrap()[3 * 2048 + 257],
        1
    );
}

#[test]
fn single_save_only_saves_the_selected_document() {
    let (ctx, mut app) = setup();
    add(&mut app, &ctx, "existing.png", chat());
    add(&mut app, &ctx, "second.png", chat());
    let directory = std::env::temp_dir().join(format!(
        "mabipng-ui-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    ));
    app.output_dir = Some(directory.clone());
    app.author = "새닉".into();
    app.save_current_as(&ctx, SaveChoice::New);
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.pending > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        app.receive(&ctx);
    }
    assert_eq!(app.pending, 0);
    assert_eq!(app.save_ok, 1, "{:?}", app.logs);
    assert!(app.entries[0].output.is_none());
    let new = Document::load(app.entries[1].output.as_ref().unwrap()).unwrap();
    assert_eq!(new.author.as_deref(), Some("기존닉"));
    assert_eq!(
        new.authid,
        Some(engine::author_to_authid("기존닉").to_string())
    );
    std::fs::remove_file(app.entries[1].output.as_ref().unwrap()).unwrap();
    std::fs::remove_dir(directory).unwrap();
}

#[test]
fn ordinary_image_cannot_be_saved_without_the_conversion_dialog() {
    let (ctx, mut app) = setup();
    add(&mut app, &ctx, "existing.png", chat());
    add(&mut app, &ctx, "ordinary.png", ordinary());
    assert_eq!(app.entries[1].texture.size(), [32, 16]);
    app.save_current_as(&ctx, SaveChoice::New);
    assert_eq!(app.pending, 0);
    assert!(app.frame_dialog.as_ref().unwrap().initial_import);
    assert!(app.apply_framing(&ctx).is_err());
    assert!(app.entries.iter().all(|entry| entry.output.is_none()));
}

#[test]
fn new_canvas_opens_in_editor_with_default_palette_and_tracks_unsaved_document() {
    let (ctx, mut app) = setup();
    app.create_blank(&ctx, 127, "새작성자").unwrap();
    assert_eq!(app.view, 3);
    assert_eq!(app.paint_color, 3);
    assert!(app.entries[0].generated && app.entries[0].has_unsaved());
    assert_eq!(app.entries[0].doc.image.dimensions(), (127, 96));
    assert_eq!(app.entries[0].doc.palette, engine::DEFAULT_CHAT_PALETTE);
    assert!(
        engine::indexed_pixels(&app.entries[0].doc)
            .unwrap()
            .iter()
            .all(|&index| index == 0)
    );
    drawing_frame(&ctx, &mut app, vec![]);
    assert!(app.entries[0].painting.is_some());
    app.author = "새이름".into();
    app.update_author();
    let bytes = engine::process(
        app.entries[0].effective_doc(),
        &app.settings_for_entry(&app.entries[0]),
    )
    .unwrap();
    let saved = Document::from_bytes(bytes).unwrap();
    assert_eq!(saved.author.as_deref(), Some("새이름"));
    assert_eq!(saved.palette, engine::DEFAULT_CHAT_PALETTE);
}

#[test]
fn custom_palette_survives_drawing_undo_redo_and_save_per_document() {
    let (ctx, mut app) = setup();
    add(&mut app, &ctx, "first.png", chat());
    add(&mut app, &ctx, "second.png", chat());
    let original = app.entries[0].doc.palette.clone();
    app.set_palette_color(&ctx, 1, [203, 41, 86]).unwrap();
    app.set_palette_color(&ctx, 2, [27, 177, 112]).unwrap();
    assert_eq!(app.entries[0].effective_doc().palette, original);
    let painting = app.entries[1].painting.as_mut().unwrap();
    painting.canvas.begin_stroke();
    painting.canvas.stroke_to(3, 4, 3, Shape::Square, 1);
    painting.active = true;
    app.finish_strokes(&ctx);
    for redo in [false, true] {
        app.paint_history(&ctx, redo);
        assert_eq!(app.entries[1].effective_doc().palette[1], [203, 41, 86]);
        assert_eq!(app.entries[1].effective_doc().palette[2], [27, 177, 112]);
    }
    let output = engine::process(
        app.entries[1].effective_doc(),
        &app.settings_for_entry(&app.entries[1]),
    )
    .unwrap();
    let saved = Document::from_bytes(output).unwrap();
    assert_eq!(saved.palette[1], [203, 41, 86]);
    assert_eq!(saved.author.as_deref(), Some("기존닉"));
    assert_eq!(engine::indexed_pixels(&saved).unwrap()[4 * 32 + 3], 1);
}

#[test]
fn clipboard_document_can_be_promoted_to_editable_chat_without_losing_source() {
    let (ctx, mut app) = setup();
    let rgba = [12_u8, 102, 160, 128].repeat(127 * 96);
    let imported = engine::document_from_rgba(127, 96, &rgba).unwrap();
    app.tx.send(Event::Clipboard(Ok(imported))).unwrap();
    app.clipboard_busy = true;
    app.receive(&ctx);
    assert!(!app.clipboard_busy);
    assert!(app.entries.is_empty());
    assert_eq!(
        app.frame_dialog.as_ref().unwrap().source.image.as_raw(),
        &rgba
    );
    assert!(app.apply_framing(&ctx).is_err());
    app.frame_dialog.as_mut().unwrap().author = "붙여넣기".into();
    app.apply_framing(&ctx).unwrap();
    assert_eq!(app.entries.len(), 1);
    assert!(app.entries[0].draft);
    assert!(app.entries[0].effective_doc().is_chat());
    assert_eq!(app.entries[0].doc.image.as_raw(), &rgba);
    assert_eq!(app.entries[0].effective_doc().image.dimensions(), (256, 96));
    app.set_palette_color(&ctx, 0, [30, 40, 50]).unwrap();
    assert_eq!(app.entries[0].effective_doc().palette[0], [30, 40, 50]);
    assert_eq!(
        app.entries[0].effective_doc().author.as_deref(),
        Some("붙여넣기")
    );
}

#[test]
fn palette_hex_input_rejects_incomplete_and_non_ascii_values() {
    assert_eq!(parse_hex_color("#eF1234"), Some([239, 18, 52]));
    assert_eq!(parse_hex_color("010101"), Some([1; 3]));
    for value in ["#123", "#12345G", "한글", "#12345678", ""] {
        assert_eq!(parse_hex_color(value), None);
    }
}

#[test]
fn saving_prevents_replacing_working_document_with_new_preview() {
    let (ctx, mut app) = setup();
    add(&mut app, &ctx, "ordinary.png", ordinary());
    let doc = engine::new_chat(96, "작성자").unwrap();
    app.preview = Some(Preview {
        texture: texture(&ctx, "preview", &doc.image),
        doc,
    });
    app.pending = 1;
    assert!(app.edit_preview(&ctx).is_err());
    assert!(app.preview.is_some());
    assert!(app.entries[0].painting.is_none());
    assert!(!app.entries[0].generated);
}

#[test]
fn canvas_viewport_stays_bounded_and_scrolls_both_axes() {
    let (ctx, _) = setup();
    let mut dimensions = None;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::splat(500.0),
            )),
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let area = canvas_scroll("bounded-test", Vec2::new(240.0, 160.0))
                    .scroll_offset(Vec2::new(220.0, 150.0))
                    .show(ui, |ui| {
                        ui.allocate_exact_size(Vec2::new(1200.0, 800.0), egui::Sense::hover());
                    });
                dimensions = Some((area.inner_rect.size(), area.content_size, area.state.offset));
            });
        },
    );
    output.textures_delta.clear();
    let (viewport, content, offset) = dimensions.unwrap();
    assert!(viewport.x <= 240.0 && viewport.y <= 160.0);
    assert!(content.x >= 1200.0 && content.y >= 800.0);
    assert!(offset.x > 0.0 && offset.y > 0.0);
}

#[test]
fn author_changes_without_checkbox_and_save_choice_controls_overwrite() {
    let (ctx, mut app) = setup();
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original.png");
    let doc = chat();
    let initial = doc.bytes.clone();
    std::fs::write(&original, &initial).unwrap();
    add(&mut app, &ctx, "original.png", doc);
    app.entries[0].path = original.canonicalize().unwrap();
    app.activate_entry(app.entries[0].id);
    assert_eq!(app.author, "기존닉");
    app.author = "변경이름".into();
    app.update_author();
    app.request_save(&ctx);
    assert_eq!(app.save_dialog, Some(true));
    assert_eq!(app.pending, 0);
    assert_eq!(std::fs::read(&original).unwrap(), initial);
    app.save_dialog = None;
    app.save_current_as(&ctx, SaveChoice::Existing);
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.pending > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        app.receive(&ctx);
    }
    assert_eq!(app.save_ok, 1, "{:?}", app.logs);
    assert_eq!(
        Document::load(&original).unwrap().author.as_deref(),
        Some("변경이름")
    );
    let first_saved = std::fs::read(&original).unwrap();
    app.author = "새파일이름".into();
    app.update_author();
    app.output_dir = Some(dir.path().to_owned());
    app.save_current_as(&ctx, SaveChoice::New);
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.pending > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        app.receive(&ctx);
    }
    assert_eq!(app.save_ok, 1, "{:?}", app.logs);
    assert_eq!(std::fs::read(&original).unwrap(), first_saved);
    let saved = app.entries[0].output.as_ref().unwrap();
    assert_ne!(saved, &original.canonicalize().unwrap());
    assert!(display_name(saved).starts_with("chat_"));
    assert_eq!(
        Document::load(saved).unwrap().author.as_deref(),
        Some("새파일이름")
    );
    assert_eq!(&app.entries[0].path, saved);
    assert!(!app.entries[0].has_unsaved());
}

#[test]
fn folder_browser_lists_disk_images_without_opening_them_until_selected() {
    let (ctx, mut app) = setup();
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first.png");
    let second = dir.path().join("second.png");
    std::fs::write(&first, chat().bytes).unwrap();
    std::fs::write(&second, ordinary().bytes).unwrap();
    std::fs::write(dir.path().join("ignore.txt"), b"test").unwrap();
    app.browser_dir = dir.path().to_owned();
    app.refresh_browser(&ctx);
    let deadline = Instant::now() + Duration::from_secs(5);
    while (app.browser_busy
        || app.browser_items.len() != 2
        || app.browser_items.iter().any(|item| item.texture.is_none()))
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
        app.receive(&ctx);
    }
    assert_eq!(app.browser_items.len(), 2);
    assert!(app.browser_items.iter().all(|item| item.texture.is_some()));
    assert!(app.entries.is_empty());
    app.open_browser_item(&ctx, first.clone());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !app.loading.is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        app.receive(&ctx);
    }
    assert_eq!(app.entries.len(), 1);
    assert_eq!(
        app.selected_entry().unwrap().path,
        first.canonicalize().unwrap()
    );
    assert_eq!(app.author, "기존닉");
}

#[test]
fn changing_browser_folder_preserves_current_single_save_target() {
    let (ctx, mut app) = setup();
    add(&mut app, &ctx, "folder-one/a.png", chat());
    add(&mut app, &ctx, "folder-one/b.png", chat());
    app.author = "미저장작성자".into();
    app.update_author();
    let selected = app.selected;
    app.set_browser_directory(PathBuf::from("folder-two"));
    assert_eq!(app.selected, selected);
    assert!(app.entries[1].has_unsaved());
    app.request_save(&ctx);
    assert!(app.save_dialog.is_some());
    assert_eq!(app.pending, 0);
}
