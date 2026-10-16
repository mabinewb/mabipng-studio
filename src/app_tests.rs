use super::*;

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
        checked: true,
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
fn mixed_batch_preserves_existing_author_and_uses_new_author_for_conversion() {
    let (ctx, mut app) = setup();
    add(&mut app, &ctx, "existing.png", chat());
    add(&mut app, &ctx, "ordinary.png", ordinary());
    let directory = std::env::temp_dir().join(format!(
        "mabipng-ui-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    ));
    app.output_dir = Some(directory.clone());
    app.author = "새닉".into();
    app.save_checked_as(&ctx, false, SaveChoice::New);
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.pending > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        app.receive(&ctx);
    }
    assert_eq!(app.pending, 0);
    assert_eq!(app.save_ok, 2, "{:?}", app.logs);
    let old = Document::load(app.entries[0].output.as_ref().unwrap()).unwrap();
    let new = Document::load(app.entries[1].output.as_ref().unwrap()).unwrap();
    assert_eq!(old.author.as_deref(), Some("기존닉"));
    assert_eq!(new.author.as_deref(), Some("새닉"));
    assert_eq!(
        new.authid,
        Some(engine::author_to_authid("새닉").to_string())
    );
    for entry in &app.entries {
        std::fs::remove_file(entry.output.as_ref().unwrap()).unwrap();
    }
    std::fs::remove_dir(directory).unwrap();
}

#[test]
fn missing_nickname_prevents_partial_batch_save_and_texture_keeps_aspect_ratio() {
    let (ctx, mut app) = setup();
    add(&mut app, &ctx, "existing.png", chat());
    add(&mut app, &ctx, "ordinary.png", ordinary());
    assert_eq!(app.entries[1].texture.size(), [32, 16]);
    app.save_checked_as(&ctx, false, SaveChoice::New);
    assert_eq!(app.pending, 0);
    assert!(app.error && app.author_focus);
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
    assert_eq!(app.entries.len(), 1);
    assert_eq!(app.entries[0].doc.image.as_raw(), &rgba);
    assert!(app.entries[0].draft && app.author_focus);
    app.author = "붙여넣기".into();
    app.start_preview(&ctx);
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.preview_busy && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        app.receive(&ctx);
    }
    assert!(app.preview.is_some(), "{:?}", app.preview_error);
    app.edit_preview(&ctx).unwrap();
    assert!(app.entries[0].effective_doc().is_chat());
    assert_eq!(app.entries[0].doc.image.as_raw(), &rgba);
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
    app.request_save(&ctx, true);
    assert_eq!(app.save_dialog, Some(true));
    assert_eq!(app.pending, 0);
    assert_eq!(std::fs::read(&original).unwrap(), initial);
    app.save_dialog = None;
    app.save_checked_as(&ctx, true, SaveChoice::Existing);
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
    app.save_checked_as(&ctx, true, SaveChoice::New);
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
fn changing_browser_folder_clears_hidden_batch_save_targets() {
    let (ctx, mut app) = setup();
    add(&mut app, &ctx, "folder-one/a.png", chat());
    add(&mut app, &ctx, "folder-one/b.png", chat());
    app.author = "미저장작성자".into();
    app.update_author();
    assert!(app.entries.iter().all(|entry| entry.checked));
    app.set_browser_directory(PathBuf::from("folder-two"));
    assert!(app.entries.iter().all(|entry| !entry.checked));
    assert!(app.entries[1].has_unsaved());
    app.request_save(&ctx, false);
    assert!(app.save_dialog.is_none());
    assert_eq!(app.pending, 0);
}
