use super::*;

#[test]
fn resized_canvas_retains_duplicate_indices_alpha_and_ancillary_metadata() {
    let doc = indexed(
        5,
        2,
        2,
        &[[12, 34, 56], [12, 34, 56], [128; 3], [255; 3]],
        &[0, 128, 200, 255],
        &[0, 1, 2, 3, 0, 1, 2, 3, 0, 1],
    );
    let doc = Document::from_bytes(insert_before_end(&doc.bytes, b"vpAg", &[1, 2, 3, 4])).unwrap();
    let mut pixels = vec![0; 129 * 49];
    pixels[0] = 1;
    pixels[129 * 49 - 1] = 3;
    let output = resize_indexed(&doc, 129, 49, &pixels).unwrap();
    assert_eq!(
        raw_chunks(&doc.bytes, &[b"IHDR", b"IDAT"]),
        raw_chunks(&output, &[b"IHDR", b"IDAT"])
    );
    let result = Document::from_bytes(output).unwrap();
    assert_eq!(result.image.dimensions(), (129, 49));
    assert_eq!(result.image.get_pixel(0, 0).0, [12, 34, 56, 128]);
    assert_eq!(result.image.get_pixel(1, 0).0, [12, 34, 56, 0]);
    assert_eq!(indexed_pixels(&result).unwrap(), pixels);
    assert!(resize_indexed(&doc, 0, 49, &[]).is_err());
    assert!(resize_indexed(&doc, 129, 49, &pixels[..100]).is_err());
}

fn settings(mode: Mode) -> Settings {
    Settings {
        mode,
        ..Settings::default()
    }
}

fn fixture(name: &str) -> Document {
    if name == "image_18.png" {
        let pixels = (0..96 * 96)
            .flat_map(|i| {
                let (x, y) = (i % 96, i / 96);
                if (16..80).contains(&x) && (16..80).contains(&y) {
                    match x % 3 {
                        0 => [20, 35, 55, 255],
                        1 => [80, 180, 190, 255],
                        _ => [150, 210, 220, 255],
                    }
                } else {
                    [255, 255, 250, 255]
                }
            })
            .collect();
        rgba_image(96, 96, pixels)
    } else {
        indexed(
            17,
            9,
            2,
            &[[255; 3], [192; 3], [128; 3], [0; 3]],
            &[],
            &(0..153).map(|i| (i % 4) as u8).collect::<Vec<_>>(),
        )
    }
}

fn rgba_image(width: u32, height: u32, pixels: Vec<u8>) -> Document {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&pixels).unwrap();
    }
    Document::from_bytes(bytes).unwrap()
}

fn indexed(
    width: u32,
    height: u32,
    depth: u8,
    palette: &[[u8; 3]],
    alpha: &[u8],
    pixels: &[u8],
) -> Document {
    let per_byte = 8 / depth as usize;
    let row_size = (width as usize).div_ceil(per_byte);
    let mut packed = vec![0; row_size * height as usize];
    for (i, &index) in pixels.iter().enumerate() {
        let x = i % width as usize;
        packed[i / width as usize * row_size + x / per_byte] |=
            index << (8 - depth as usize - (x % per_byte) * depth as usize);
    }
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Indexed);
        encoder.set_depth(match depth {
            1 => png::BitDepth::One,
            2 => png::BitDepth::Two,
            4 => png::BitDepth::Four,
            _ => png::BitDepth::Eight,
        });
        encoder.set_palette(palette.iter().flatten().copied().collect::<Vec<_>>());
        if !alpha.is_empty() {
            encoder.set_trns(alpha.to_vec());
        }
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&packed).unwrap();
    }
    Document::from_bytes(edit(&bytes, false, Some("테스트")).unwrap()).unwrap()
}

fn raw_chunks(bytes: &[u8], excluded: &[&[u8]]) -> Vec<Vec<u8>> {
    parse_chunks(bytes)
        .unwrap()
        .into_iter()
        .filter(|c| !excluded.contains(&c.kind))
        .map(|c| c.raw.to_vec())
        .collect()
}

fn insert_before_end(bytes: &[u8], kind: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = bytes[..bytes.len() - 12].to_vec();
    push_chunk(&mut out, kind, data);
    out.extend_from_slice(&bytes[bytes.len() - 12..]);
    out
}

#[test]
fn four_color_promotes_one_bit_preserving_indices_rgba_and_metadata() {
    let source = [0, 1, 1, 0, 1, 0, 0, 1, 1, 1, 0, 1, 0, 0, 1, 1, 0, 1];
    let doc = indexed(9, 2, 1, &[[12, 34, 56]; 2], &[0, 128], &source);
    let bytes = insert_before_end(&doc.bytes, b"vpAg", &[1, 2, 3]);
    let bytes = insert_before_end(&bytes, b"tEXt", b"Comment\0keep me");
    let doc = Document::from_bytes(bytes).unwrap();
    let result = four_color_document(&doc).unwrap();
    assert_eq!(result.bit_depth, 2);
    assert_eq!(
        result.palette,
        [[12, 34, 56], [12, 34, 56], [129; 3], [1; 3]]
    );
    assert_eq!(indexed_pixels(&result).unwrap(), source);
    assert_eq!(result.image, doc.image);
    assert_eq!(result.author, doc.author);
    assert_eq!(result.authid, doc.authid);
    assert_eq!(
        raw_chunks(&result.bytes, &[b"IHDR", b"PLTE", b"IDAT"]),
        raw_chunks(&doc.bytes, &[b"IHDR", b"PLTE", b"IDAT"])
    );
    assert_eq!(
        palette_rgba(&result)[2..],
        [[129, 129, 129, 255], [1, 1, 1, 255]]
    );
}

#[test]
fn four_color_two_bit_changes_only_palette_including_split_idat() {
    for count in [1, 2, 3] {
        let palette = [[10, 20, 30], [10, 20, 30], [80; 3]];
        let source: Vec<_> = (0..10).map(|i| (i % count) as u8).collect();
        let doc = indexed(5, 2, 2, &palette[..count], &[128], &source);
        let mut bytes = SIGNATURE.to_vec();
        for chunk in parse_chunks(&doc.bytes).unwrap() {
            if chunk.kind == b"IDAT" {
                let mid = chunk.data.len() / 2;
                push_chunk(&mut bytes, b"IDAT", &chunk.data[..mid]);
                push_chunk(&mut bytes, b"IDAT", &[]);
                push_chunk(&mut bytes, b"IDAT", &chunk.data[mid..]);
            } else {
                bytes.extend_from_slice(chunk.raw);
            }
        }
        let doc = Document::from_bytes(insert_before_end(&bytes, b"vpAg", &[42])).unwrap();
        let result = four_color_document(&doc).unwrap();
        assert_eq!(result.bit_depth, 2);
        assert_eq!(&result.palette[..count], &palette[..count]);
        assert_eq!(&result.palette[count..], &DEFAULT_CHAT_PALETTE[count..]);
        assert_eq!(result.image, doc.image);
        assert_eq!(indexed_pixels(&result).unwrap(), source);
        assert_eq!(
            raw_chunks(&result.bytes, &[b"PLTE"]),
            raw_chunks(&doc.bytes, &[b"PLTE"])
        );
    }
}

#[test]
fn four_color_is_byte_exact_when_already_four_colors_and_requires_chat() {
    let doc = new_chat(96, "test").unwrap();
    assert_eq!(four_color_document(&doc).unwrap().bytes, doc.bytes);
    let rgba = rgba_image(1, 1, vec![0, 0, 0, 255]);
    assert!(four_color_document(&rgba).is_err());
    let no_author = Document::from_bytes(raw_chunks(&doc.bytes, &[b"zTXt"]).into_iter().fold(
        SIGNATURE.to_vec(),
        |mut out, raw| {
            out.extend(raw);
            out
        },
    ))
    .unwrap();
    assert!(four_color_document(&no_author).is_err());
}

#[test]
fn four_color_rejects_apng_when_transformation_is_needed() {
    for (depth, count) in [(1, 2), (2, 1), (2, 3)] {
        for kind in [b"acTL", b"fcTL", b"fdAT"] {
            let doc = indexed(1, 1, depth, &DEFAULT_CHAT_PALETTE[..count], &[], &[0]);
            let bytes = insert_before_end(&doc.bytes, kind, &[]);
            let fake = Document { bytes, ..doc };
            assert!(
                four_color_document(&fake)
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("APNG")
            );
        }
    }
}

#[test]
fn new_chat_has_fixed_height_four_colors_zero_indices_and_author_metadata() {
    assert_eq!(
        DEFAULT_CHAT_PALETTE,
        [[0xfe; 3], [0xc1; 3], [0x81; 3], [0x01; 3]]
    );
    for width in [96, 97, 127, 255, 256] {
        let author = "Test Author";
        let doc = new_chat(width, author).unwrap();
        assert_eq!(doc.image.dimensions(), (width, 96));
        assert_eq!((doc.color_type, doc.bit_depth), (3, 2));
        assert_eq!(doc.palette, DEFAULT_CHAT_PALETTE);
        assert_eq!(doc.author.as_deref(), Some(author));
        assert_eq!(doc.authid.as_deref(), Some("-416727624"));
        assert!(doc.is_chat());
        assert_eq!(indexed_pixels(&doc).unwrap(), vec![0; width as usize * 96]);
        assert!(doc.image.pixels().all(|p| p.0 == [254, 254, 254, 255]));
        let chunks = parse_chunks(&doc.bytes).unwrap();
        assert_eq!(chunks.iter().filter(|c| c.kind == b"PLTE").count(), 1);
        assert_eq!(
            chunks
                .iter()
                .find(|c| c.kind == b"PLTE")
                .unwrap()
                .data
                .len(),
            12
        );
        for key in [b"author".as_slice(), b"authid"] {
            assert_eq!(
                chunks.iter().filter(|c| text_key(c) == Some(key)).count(),
                1
            );
        }
    }
}

#[test]
fn new_chat_rejects_out_of_range_widths_and_invalid_authors() {
    for width in [0, 257, u32::MAX] {
        assert!(new_chat(width, "test").is_err(), "width {width}");
    }
    for width in [1, 2, 3, 48, 95, 96, 127, 256] {
        let doc = new_chat(width, "Test").unwrap();
        assert_eq!(doc.image.dimensions(), (width, 96));
        assert_eq!(indexed_pixels(&doc).unwrap().len(), width as usize * 96);
    }
    for author in ["", "  ", "(none)", "a\0b", "a\nb", "\u{1f600}"] {
        assert!(new_chat(96, author).is_err(), "author {author:?}");
    }
    let doc = new_chat(96, " test ").unwrap();
    assert_eq!(doc.author.as_deref(), Some(" test "));
    assert_eq!(doc.authid, Some(author_to_authid(" test ").to_string()));
}

#[test]
fn custom_palette_changes_only_plte_preserving_split_idat_alpha_and_metadata() {
    let source = [0, 1, 2, 3, 1, 0, 3, 2, 1, 0];
    let doc = indexed(
        5,
        2,
        2,
        &[[12, 34, 56], [12, 34, 56], [80; 3], [255; 3]],
        &[0, 128, 255, 100],
        &source,
    );
    let mut bytes = SIGNATURE.to_vec();
    for chunk in parse_chunks(&doc.bytes).unwrap() {
        if chunk.kind == b"IDAT" {
            let mid = chunk.data.len() / 2;
            push_chunk(&mut bytes, b"IDAT", &chunk.data[..mid]);
            push_chunk(&mut bytes, b"IDAT", &[]);
            push_chunk(&mut bytes, b"IDAT", &chunk.data[mid..]);
        } else {
            bytes.extend_from_slice(chunk.raw);
        }
    }
    let bytes = insert_before_end(&bytes, b"vpAg", &[1, 2, 3, 4]);
    let bytes = insert_before_end(&bytes, b"tEXt", b"Comment\0retain me");
    let doc = Document::from_bytes(bytes).unwrap();
    let colors = [[255; 3], [192; 3], [128; 3], [0; 3]];
    let output = replace_palette(&doc, &colors).unwrap();
    assert_eq!(
        raw_chunks(&doc.bytes, &[b"PLTE"]),
        raw_chunks(&output, &[b"PLTE"])
    );
    let result = Document::from_bytes(output).unwrap();
    assert_eq!(result.palette, colors);
    assert_eq!(indexed_pixels(&result).unwrap(), source);
    assert_eq!(result.author, doc.author);
    assert_eq!(result.authid, doc.authid);
    assert_eq!(
        palette_rgba(&result),
        [
            [255, 255, 255, 0],
            [192, 192, 192, 128],
            [128, 128, 128, 255],
            [0, 0, 0, 100],
        ]
    );
    assert_eq!(replace_palette(&doc, &doc.palette).unwrap(), doc.bytes);

    let duplicates = [[10, 20, 30]; 4];
    let result = Document::from_bytes(replace_palette(&doc, &duplicates).unwrap()).unwrap();
    assert_eq!(result.palette, duplicates);
    assert_eq!(indexed_pixels(&result).unwrap(), source);
    assert_eq!(result.image.get_pixel(0, 0).0, [10, 20, 30, 0]);
    assert_eq!(result.image.get_pixel(1, 0).0, [10, 20, 30, 128]);
}

#[test]
fn custom_palette_supports_all_indexed_depths_and_256_entries_without_chat_metadata() {
    for (depth, count) in [(1, 2), (2, 4), (4, 16), (8, 256)] {
        let palette: Vec<_> = (0..count).map(|i| [i as u8; 3]).collect();
        let source: Vec<_> = (0..count).map(|i| i as u8).collect();
        let doc = indexed(count, 1, depth, &palette, &[], &source);
        let mut bytes = SIGNATURE.to_vec();
        for chunk in parse_chunks(&doc.bytes).unwrap() {
            if !matches!(text_key(&chunk), Some(b"author" | b"authid")) {
                bytes.extend_from_slice(chunk.raw);
            }
        }
        let doc = Document::from_bytes(bytes).unwrap();
        assert!(!doc.is_chat());
        let colors: Vec<_> = (0..count).map(|i| [255 - i as u8, 42, i as u8]).collect();
        let output = replace_palette(&doc, &colors).unwrap();
        assert_eq!(
            raw_chunks(&doc.bytes, &[b"PLTE"]),
            raw_chunks(&output, &[b"PLTE"])
        );
        let result = Document::from_bytes(output).unwrap();
        assert_eq!(result.bit_depth, depth);
        assert_eq!(result.palette, colors);
        assert!(result.author.is_none());
        assert!(result.authid.is_none());
        for (pixel, color) in result.image.pixels().zip(colors) {
            assert_eq!(pixel.0, [color[0], color[1], color[2], 255]);
        }
    }
}

#[test]
fn custom_palette_requires_exact_original_length_valid_chunks_and_indexed_format() {
    let mut doc = new_chat(96, "test").unwrap();
    for count in [0, 1, 3, 5, 256, 257] {
        assert!(replace_palette(&doc, &vec![[1, 2, 3]; count]).is_err());
    }
    doc.palette.clear();
    doc.color_type = 6;
    assert!(replace_palette(&doc, &DEFAULT_CHAT_PALETTE).is_ok());
    assert!(replace_palette(&doc, &[]).is_err());
    doc.bytes[40] ^= 1;
    assert!(replace_palette(&doc, &DEFAULT_CHAT_PALETTE).is_err());
    let mut rgba = rgba_image(1, 1, vec![0, 0, 0, 255]);
    rgba.color_type = 3;
    rgba.palette = DEFAULT_CHAT_PALETTE.to_vec();
    assert!(replace_palette(&rgba, &DEFAULT_CHAT_PALETTE).is_err());
}

#[test]
fn clipboard_rgba_preserves_all_channels_alpha_and_dimensions_without_quantization() {
    let pixels = [
        12, 34, 56, 0, 100, 110, 120, 1, 1, 2, 3, 127, 255, 128, 64, 128, 0, 255, 0, 254, 255, 255,
        255, 255,
    ];
    let doc = document_from_rgba(3, 2, &pixels).unwrap();
    assert_eq!(doc.image.dimensions(), (3, 2));
    assert_eq!(doc.image.as_raw(), &pixels);
    assert_eq!((doc.color_type, doc.bit_depth), (6, 8));
    assert!(doc.palette.is_empty());
    assert!(doc.author.is_none());
    assert!(doc.authid.is_none());
    assert!(!doc.is_chat());
    let reloaded = Document::from_bytes(doc.bytes).unwrap();
    assert_eq!(reloaded.image.as_raw(), &pixels);
    let single = document_from_rgba(1, 1, &[1, 2, 3, 0]).unwrap();
    assert_eq!(single.image.get_pixel(0, 0).0, [1, 2, 3, 0]);
}

#[test]
fn clipboard_rgba_rejects_malformed_lengths_and_invalid_or_oversize_dimensions() {
    for length in [0, 1, 3, 4, 7, 9, 12] {
        assert!(
            document_from_rgba(2, 1, &vec![0; length]).is_err(),
            "length {length}"
        );
    }
    for (width, height) in [
        (0, 0),
        (0, 1),
        (1, 0),
        (MAX_PIXELS as u32 + 1, 1),
        (1, MAX_PIXELS as u32 + 1),
        (8192, 4097),
        (u32::MAX, 1),
        (u32::MAX, u32::MAX),
    ] {
        let error = document_from_rgba(width, height, &[]).err().unwrap();
        assert!(
            error.to_string().contains("32 메가픽셀"),
            "{width}x{height}: {error}"
        );
    }
    // The inclusive pixel limit passes dimension validation without allocating 128 MiB.
    let error = document_from_rgba(8192, 4096, &[]).err().unwrap();
    assert!(error.to_string().contains("RGBA"));
}

#[test]
fn author_hash_vectors_utf16_nul_and_signed_sentinel() {
    for (author, expected) in [
        ("", 5381),
        (" ", 177605),
        ("  ", 5860997),
        ("abc", 193485963),
        ("Test Author", -416727624),
        ("\u{1f600}", 7743522),
        ("a\u{1f600}z", -2142848451),
        ("\u{f00e}\u{13}\u{12}\u{7}", 0),
        ("abc\0ignored", 193485963),
    ] {
        assert_eq!(author_to_authid(author), expected, "{author:?}");
    }
}

#[test]
fn synthetic_fixtures_are_deterministic_without_external_files() {
    for name in ["chat_sample.png", "image_18.png"] {
        assert_eq!(fixture(name).bytes, fixture(name).bytes);
    }
}

#[test]
fn generated_chats_preserve_idat_and_other_chunks() {
    for width in 96..=140 {
        let name = format!("synthetic-{width}");
        let doc = new_chat(width, "Test Author").unwrap();
        assert!(doc.is_chat());
        let output = process(&doc, &settings(Mode::Auto)).unwrap();
        assert_eq!(
            raw_chunks(&doc.bytes, &[b"PLTE"]),
            raw_chunks(&output, &[b"PLTE"]),
            "{name}"
        );
        let edited = Document::from_bytes(output).unwrap();
        assert_eq!(doc.author, edited.author);
        assert_eq!(doc.authid, edited.authid);
        let mut indices = indexed_pixels(&doc).unwrap();
        if doc.palette.len() > 1 {
            indices[0] = (indices[0] + 1) % doc.palette.len() as u8;
        }
        let painted = paint_indexed(&doc, &indices).unwrap();
        assert_eq!(
            raw_chunks(&doc.bytes, &[b"IDAT", b"IHDR"]),
            raw_chunks(&painted, &[b"IDAT", b"IHDR"]),
            "paint: {name}"
        );
        assert_eq!(
            indexed_pixels(&Document::from_bytes(painted).unwrap()).unwrap(),
            indices,
            "paint: {name}"
        );
    }
}

#[test]
fn no_op_is_byte_identical_and_palette_replacement_is_exact() {
    let palette = [[255; 3], [192; 3], [128; 3], [0; 3]];
    let doc = indexed(
        5,
        2,
        2,
        &palette,
        &[255, 128, 0],
        &[0, 1, 2, 3, 0, 3, 2, 1, 0, 1],
    );
    let mut options = settings(Mode::Palette);
    options.fix_palette = false;
    assert_eq!(process(&doc, &options).unwrap(), doc.bytes);
    options.fix_palette = true;
    let output = process(&doc, &options).unwrap();
    assert_eq!(
        raw_chunks(&doc.bytes, &[b"PLTE"]),
        raw_chunks(&output, &[b"PLTE"])
    );
    let result = Document::from_bytes(output).unwrap();
    assert_eq!(result.palette, [[254; 3], [193; 3], [129; 3], [1; 3]]);
    assert_eq!(
        palette_rgba(&result)
            .iter()
            .map(|p| p[3])
            .collect::<Vec<_>>(),
        [255, 128, 0, 255]
    );
}

#[test]
fn author_edit_preserves_pixels_alpha_and_unrelated_text() {
    let doc = indexed(3, 1, 2, &[[255; 3], [0; 3]], &[0, 128], &[0, 1, 0]);
    let bytes = insert_before_end(&doc.bytes, b"vpAg", &[1, 2, 3, 4]);
    let bytes = insert_before_end(&bytes, b"tEXt", b"Comment\0retain me");
    let doc = Document::from_bytes(bytes).unwrap();
    let options = Settings {
        author: Some("Test Author".into()),
        resize: Some((1, 1)),
        ..settings(Mode::Author)
    };
    let output = process(&doc, &options).unwrap();
    let source: Vec<_> = parse_chunks(&doc.bytes)
        .unwrap()
        .into_iter()
        .filter(|c| !matches!(text_key(c), Some(b"author" | b"authid")))
        .map(|c| c.raw.to_vec())
        .collect();
    let target: Vec<_> = parse_chunks(&output)
        .unwrap()
        .into_iter()
        .filter(|c| !matches!(text_key(c), Some(b"author" | b"authid")))
        .map(|c| c.raw.to_vec())
        .collect();
    assert_eq!(source, target);
    let result = Document::from_bytes(output).unwrap();
    assert_eq!(result.image, doc.image);
    assert_eq!(result.authid.as_deref(), Some("-416727624"));
    assert_eq!(result.author, options.author);
}

#[test]
fn detection_requires_metadata_and_low_depth_indexed_but_allows_old_blank_author() {
    let doc = fixture("chat_sample.png");
    for author in ["", " ", "  "] {
        let mut bytes = SIGNATURE.to_vec();
        for chunk in parse_chunks(&doc.bytes).unwrap() {
            if text_key(&chunk) == Some(b"author") {
                let mut data = b"author\0".to_vec();
                data.extend_from_slice(author.as_bytes());
                push_chunk(&mut bytes, b"tEXt", &data);
            } else {
                bytes.extend_from_slice(chunk.raw);
            }
        }
        let existing = Document::from_bytes(bytes).unwrap();
        assert!(existing.is_chat());
        assert_eq!(
            raw_chunks(&existing.bytes, &[b"PLTE"]),
            raw_chunks(
                &process(&existing, &settings(Mode::Auto)).unwrap(),
                &[b"PLTE"]
            )
        );
    }
    let rgba = rgba_image(1, 1, vec![0, 0, 0, 255]);
    assert!(
        !Document::from_bytes(edit(&rgba.bytes, false, Some("테스트")).unwrap())
            .unwrap()
            .is_chat()
    );
    let eight = indexed(1, 1, 8, &[[0; 3], [255; 3]], &[], &[0]);
    assert!(!eight.is_chat());
    let mut missing = fixture("chat_sample.png");
    missing.author = None;
    assert!(!missing.is_chat());
    missing.author = Some("(none)".into());
    assert!(!missing.is_chat());
}

#[test]
fn invalid_author_and_required_author_are_errors() {
    let doc = fixture("chat_sample.png");
    for author in ["", "  ", "\0", "a\nb", "닉\u{1f600}", "(none)"] {
        let options = Settings {
            author: Some(author.into()),
            ..settings(Mode::Author)
        };
        assert!(process(&doc, &options).is_err(), "{author:?}");
    }
    assert!(process(&doc, &settings(Mode::Author)).is_err());
    let input = rgba_image(1, 1, vec![0, 0, 0, 255]);
    assert!(process(&input, &settings(Mode::Convert)).is_err());
}

#[test]
fn corruption_truncation_and_oversize_are_rejected_without_panics() {
    let doc = fixture("chat_sample.png");
    for length in 0..doc.bytes.len() {
        assert!(
            Document::from_bytes(doc.bytes[..length].to_vec()).is_err(),
            "length {length}"
        );
    }
    let mut crc = doc.bytes.clone();
    crc[40] ^= 1;
    assert!(
        Document::from_bytes(crc)
            .err()
            .unwrap()
            .to_string()
            .contains("CRC")
    );
    let mut length = doc.bytes.clone();
    length[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(Document::from_bytes(length).is_err());
    let mut trailing = doc.bytes.clone();
    trailing.push(0);
    assert!(Document::from_bytes(trailing).is_err());
    let mut huge = SIGNATURE.to_vec();
    let mut header = parse_chunks(&doc.bytes).unwrap()[0].data.to_vec();
    header[..4].copy_from_slice(&u32::MAX.to_be_bytes());
    push_chunk(&mut huge, b"IHDR", &header);
    assert!(Document::from_bytes(huge).is_err());
}

#[test]
fn malformed_metadata_and_text_bombs_are_errors() {
    let doc = fixture("chat_sample.png");
    for bad in [
        b"author\0\x01bad".as_slice(),
        b"author\0\0bad",
        b"author\0\0",
    ] {
        let bytes = insert_before_end(&doc.bytes, b"zTXt", bad);
        assert!(Document::from_bytes(bytes).is_err());
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&vec![b'a'; MAX_TEXT_SIZE + 1]).unwrap();
    let mut data = b"author\0\0".to_vec();
    data.extend(encoder.finish().unwrap());
    assert!(Document::from_bytes(insert_before_end(&doc.bytes, b"zTXt", &data)).is_err());
}

#[test]
fn duplicate_target_text_types_are_replaced_without_touching_other_text() {
    let doc = fixture("chat_sample.png");
    let bytes = insert_before_end(&doc.bytes, b"tEXt", b"author\0old");
    let bytes = insert_before_end(&bytes, b"iTXt", b"authid\0\0\0\0\0-1");
    let doc = Document::from_bytes(bytes).unwrap();
    let output = process(
        &doc,
        &Settings {
            author: Some(" 새 닉네임 ".into()),
            ..settings(Mode::Author)
        },
    )
    .unwrap();
    let chunks = parse_chunks(&output).unwrap();
    assert_eq!(
        chunks
            .iter()
            .filter(|c| text_key(c) == Some(b"author"))
            .count(),
        1
    );
    assert_eq!(
        chunks
            .iter()
            .filter(|c| text_key(c) == Some(b"authid"))
            .count(),
        1
    );
    let result = Document::from_bytes(output).unwrap();
    assert_eq!(result.author.as_deref(), Some(" 새 닉네임 "));
    assert_eq!(
        result.authid,
        Some(author_to_authid(" 새 닉네임 ").to_string())
    );
}

#[test]
fn conversion_composites_alpha_onto_white_and_packs_odd_widths() {
    let input = rgba_image(
        5,
        2,
        vec![
            255, 0, 0, 128, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 255, 255, 0, 0, 128,
        ]
        .repeat(2),
    );
    let options = Settings {
        author: Some("테스트".into()),
        fix_palette: false,
        preserve_white_background: false,
        ..settings(Mode::Convert)
    };
    let result = Document::from_bytes(process(&input, &options).unwrap()).unwrap();
    assert!(result.is_chat());
    assert_eq!(result.bit_depth, 2);
    assert_eq!(result.color_type, 3);
    assert_eq!(result.image.dimensions(), (5, 2));
    assert_eq!(result.image.get_pixel(0, 0).0, [255, 127, 127, 255]);
    assert_eq!(result.image.get_pixel(1, 0).0, [255, 255, 255, 255]);
    assert!(result.image.pixels().all(|p| p[3] == 255));
    assert_eq!(indexed_pixels(&result).unwrap().len(), 10);
}

#[test]
fn border_background_preserved_without_spending_foreground_color_on_inner_white() {
    let mut pixels = vec![255u8; 20 * 20 * 4];
    for y in 3..17 {
        for x in 3..17 {
            let color = if y == 3 || y == 16 || x == 3 || x == 16 {
                [25, 30, 35, 255]
            } else if x < 7 {
                [80, 120, 150, 255]
            } else if x < 11 {
                [130, 200, 210, 255]
            } else {
                [255, 255, 255, 255]
            };
            pixels[(y * 20 + x) * 4..(y * 20 + x + 1) * 4].copy_from_slice(&color);
        }
    }
    let input = rgba_image(20, 20, pixels);
    let options = Settings {
        author: Some("테스트".into()),
        ..settings(Mode::Convert)
    };
    let result = Document::from_bytes(process(&input, &options).unwrap()).unwrap();
    assert_eq!(result.palette[0], [255, 255, 250]);
    assert_eq!(result.palette.len(), 4);
    assert!(
        result.palette[1..]
            .iter()
            .all(|c| c.iter().any(|&v| v < 245))
    );
    assert_eq!(result.image.get_pixel(0, 0).0, [255, 255, 250, 255]);
    assert_eq!(result.image.get_pixel(14, 10).0, [255, 255, 250, 255]);
}

#[test]
fn conversion_is_deterministic_and_resize_is_opt_in_fit_only() {
    let input = rgba_image(1024, 200, [90, 130, 180, 255].repeat(1024 * 200));
    let mut options = Settings {
        author: Some("테스트".into()),
        ..settings(Mode::Convert)
    };
    assert_eq!(
        Document::from_bytes(process(&input, &options).unwrap())
            .unwrap()
            .image
            .dimensions(),
        (1024, 200)
    );
    options.resize = Some((256, 96));
    let output = process(&input, &options).unwrap();
    assert_eq!(output, process(&input, &options).unwrap());
    assert_eq!(
        Document::from_bytes(output).unwrap().image.dimensions(),
        (256, 50)
    );
    options.resize = Some((0, 96));
    assert!(process(&input, &options).is_err());
    options.resize = Some((2048, 2048));
    assert_eq!(
        Document::from_bytes(process(&input, &options).unwrap())
            .unwrap()
            .image
            .dimensions(),
        (1024, 200)
    );
}

#[test]
fn synthetic_conversion_keeps_background_and_three_foreground_colors() {
    let input = fixture("image_18.png");
    let options = Settings {
        author: Some("테스트".into()),
        ..settings(Mode::Convert)
    };
    let output = process(&input, &options).unwrap();
    let result = Document::from_bytes(output.clone()).unwrap();
    assert_eq!(result.image.dimensions(), input.image.dimensions());
    assert_eq!(result.palette[0], [255, 255, 250]);
    assert_eq!(result.palette.len(), 4);
    assert!(
        result.palette[1..]
            .iter()
            .all(|c| c.iter().any(|&v| v < 245))
    );
    assert!(
        result
            .palette
            .iter()
            .any(|c| u32::from(c[0]) + u32::from(c[1]) + u32::from(c[2]) < 250)
    );
    assert!(
        result
            .palette
            .iter()
            .any(|c| c[1] > 150 && c[2] > 150 && c[0] < 200)
    );
    assert_eq!(output, process(&input, &options).unwrap());
}

#[test]
fn indexed_paint_preserves_duplicate_palette_entries_alpha_and_metadata() {
    let source = [0, 1, 2, 3, 0, 3, 2, 1, 0, 1];
    let doc = indexed(
        5,
        2,
        2,
        &[[12, 34, 56], [12, 34, 56], [80; 3], [255; 3]],
        &[0, 128, 255, 100],
        &source,
    );
    assert_eq!(indexed_pixels(&doc).unwrap(), source);
    assert_eq!(
        palette_rgba(&doc),
        [
            [12, 34, 56, 0],
            [12, 34, 56, 128],
            [80, 80, 80, 255],
            [255, 255, 255, 100]
        ]
    );
    let mut changed = source;
    changed[4] = 1;
    changed[5] = 0;
    let output = paint_indexed(&doc, &changed).unwrap();
    assert_eq!(
        raw_chunks(&doc.bytes, &[b"IDAT"]),
        raw_chunks(&output, &[b"IDAT"])
    );
    let result = Document::from_bytes(output).unwrap();
    assert_eq!(indexed_pixels(&result).unwrap(), changed);
    assert_eq!(result.image.get_pixel(4, 0).0, [12, 34, 56, 128]);
    assert_eq!(result.author, doc.author);
    assert_eq!(result.authid, doc.authid);
    assert_eq!(result.palette, doc.palette);
}

#[test]
fn one_bit_paint_uses_row_padding_and_rejects_bad_input() {
    let source = [0, 1, 0, 1, 0, 1, 1, 0, 1, 0, 1, 0, 1, 1, 0, 1, 0, 1];
    let doc = indexed(9, 2, 1, &[[0; 3], [255; 3]], &[], &source);
    assert_eq!(indexed_pixels(&doc).unwrap(), source);
    let changed: Vec<_> = source.iter().map(|&i| 1 - i).collect();
    let result = Document::from_bytes(paint_indexed(&doc, &changed).unwrap()).unwrap();
    assert_eq!(result.bit_depth, 1);
    assert_eq!(indexed_pixels(&result).unwrap(), changed);
    assert!(paint_indexed(&doc, &changed[..17]).is_err());
    let mut bad = changed;
    bad[3] = 2;
    assert!(paint_indexed(&doc, &bad).is_err());
    let nonchat = rgba_image(1, 1, vec![0, 0, 0, 255]);
    assert!(indexed_pixels(&nonchat).is_err());
    assert!(paint_indexed(&nonchat, &[0]).is_err());
}

#[test]
fn apng_paint_is_explicitly_rejected() {
    let doc = fixture("chat_sample.png");
    let mut control = 1u32.to_be_bytes().to_vec();
    control.extend_from_slice(&0u32.to_be_bytes());
    let mut out = SIGNATURE.to_vec();
    for chunk in parse_chunks(&doc.bytes).unwrap() {
        out.extend_from_slice(chunk.raw);
        if chunk.kind == b"IHDR" {
            push_chunk(&mut out, b"acTL", &control);
        }
    }
    // Image decoding may reject an incomplete APNG, but the drawing guard must reject it first.
    let fake = Document { bytes: out, ..doc };
    assert!(
        indexed_pixels(&fake)
            .unwrap_err()
            .to_string()
            .contains("APNG")
    );
    assert!(
        paint_indexed(&fake, &[])
            .unwrap_err()
            .to_string()
            .contains("APNG")
    );
}

#[test]
fn adam7_original_indices_are_decoded_and_paint_changes_only_interlace_and_idat() {
    let doc = indexed(2, 2, 1, &[[0; 3], [255; 3]], &[0, 255], &[0, 1, 1, 0]);
    // Adam7 2x2: pass 1=(0,0), pass 6=(1,0), pass 7=(0,1),(1,1).
    let mut compressor = ZlibEncoder::new(Vec::new(), Compression::default());
    compressor.write_all(&[0, 0x00, 0, 0x80, 0, 0x80]).unwrap();
    let compressed = compressor.finish().unwrap();
    let mut bytes = SIGNATURE.to_vec();
    for chunk in parse_chunks(&doc.bytes).unwrap() {
        match chunk.kind {
            b"IHDR" => {
                let mut header = chunk.data.to_vec();
                header[12] = 1;
                push_chunk(&mut bytes, b"IHDR", &header);
            }
            b"IDAT" => push_chunk(&mut bytes, b"IDAT", &compressed),
            _ => bytes.extend_from_slice(chunk.raw),
        }
    }
    let interlaced = Document::from_bytes(bytes).unwrap();
    assert_eq!(indexed_pixels(&interlaced).unwrap(), [0, 1, 1, 0]);
    let expanded = four_color_document(&interlaced).unwrap();
    assert_eq!(expanded.bit_depth, 2);
    assert_eq!(expanded.palette.len(), 4);
    assert_eq!(parse_chunks(&expanded.bytes).unwrap()[0].data[12], 0);
    assert_eq!(expanded.image, interlaced.image);
    assert_eq!(indexed_pixels(&expanded).unwrap(), [0, 1, 1, 0]);
    assert_eq!(
        raw_chunks(&expanded.bytes, &[b"IHDR", b"PLTE", b"IDAT"]),
        raw_chunks(&interlaced.bytes, &[b"IHDR", b"PLTE", b"IDAT"])
    );
    let painted = paint_indexed(&interlaced, &[1, 0, 0, 1]).unwrap();
    assert_eq!(
        raw_chunks(&interlaced.bytes, &[b"IHDR", b"IDAT"]),
        raw_chunks(&painted, &[b"IHDR", b"IDAT"])
    );
    assert_eq!(parse_chunks(&painted).unwrap()[0].data[12], 0);
    assert_eq!(
        indexed_pixels(&Document::from_bytes(painted).unwrap()).unwrap(),
        [1, 0, 0, 1]
    );
}
