use anyhow::{Context, Result, ensure};
use encoding_rs::EUC_KR;
use flate2::{Compression, Decompress, FlushDecompress, Status, write::ZlibEncoder};
use image::{RgbaImage, imageops::FilterType};
use std::{
    collections::VecDeque,
    fs,
    io::{Cursor, Write},
    path::Path,
};

const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const MAX_FILE_SIZE: usize = 128 * 1024 * 1024;
const MAX_PIXELS: u64 = 32 * 1024 * 1024;
const MAX_TEXT_SIZE: usize = 1024 * 1024;

pub const DEFAULT_CHAT_PALETTE: [[u8; 3]; 4] = [[254; 3], [193; 3], [129; 3], [1; 3]];

pub struct Document {
    pub bytes: Vec<u8>,
    pub image: RgbaImage,
    pub author: Option<String>,
    pub authid: Option<String>,
    pub palette: Vec<[u8; 3]>,
    pub bit_depth: u8,
    pub color_type: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Auto,
    Convert,
    Palette,
    Author,
}

#[derive(Clone)]
pub struct Settings {
    pub mode: Mode,
    pub author: Option<String>,
    pub fix_palette: bool,
    pub preserve_white_background: bool,
    pub resize: Option<(u32, u32)>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: Mode::Auto,
            author: None,
            fix_palette: true,
            preserve_white_background: true,
            resize: None,
        }
    }
}

#[derive(Clone, Copy)]
struct Chunk<'a> {
    kind: &'a [u8],
    data: &'a [u8],
    raw: &'a [u8],
}

fn parse_chunks(bytes: &[u8]) -> Result<Vec<Chunk<'_>>> {
    ensure!(
        bytes.len() <= MAX_FILE_SIZE,
        "PNG 파일이 너무 큽니다. 128 MiB 이하 파일을 선택해 주세요."
    );
    ensure!(
        bytes.starts_with(SIGNATURE),
        "PNG 파일이 아닙니다. PNG 파일을 선택해 주세요."
    );
    let mut chunks = Vec::new();
    let mut pos = 8usize;
    let mut palette_len = None;
    let mut idat_seen = false;
    let mut idat_ended = false;
    let mut ended = false;
    let mut color_type = 0;
    let mut depth = 0;
    while pos < bytes.len() {
        ensure!(
            bytes.len() - pos >= 12,
            "PNG 청크가 잘렸습니다. 원본 파일을 확인해 주세요."
        );
        let len = u32::from_be_bytes(bytes[pos..pos + 4].try_into()?) as usize;
        ensure!(len <= 0x7fff_ffff, "PNG 청크 길이가 올바르지 않습니다.");
        let end = pos
            .checked_add(12)
            .and_then(|v| v.checked_add(len))
            .context("PNG 청크 길이가 올바르지 않습니다.")?;
        ensure!(
            end <= bytes.len(),
            "PNG 청크 길이가 파일 범위를 벗어납니다."
        );
        let kind = &bytes[pos + 4..pos + 8];
        ensure!(
            kind.iter().all(u8::is_ascii_alphabetic) && kind[2].is_ascii_uppercase(),
            "PNG 청크 이름이 올바르지 않습니다."
        );
        let data = &bytes[pos + 8..end - 4];
        let expected = u32::from_be_bytes(bytes[end - 4..end].try_into()?);
        ensure!(
            crc32fast::hash(&bytes[pos + 4..end - 4]) == expected,
            "PNG {} 청크의 CRC가 일치하지 않습니다. 파일 손상 여부를 확인해 주세요.",
            String::from_utf8_lossy(kind)
        );
        if chunks.is_empty() {
            ensure!(
                kind == b"IHDR" && len == 13,
                "PNG의 첫 IHDR 청크가 올바르지 않습니다."
            );
            let width = u32::from_be_bytes(data[..4].try_into()?);
            let height = u32::from_be_bytes(data[4..8].try_into()?);
            ensure!(
                width > 0 && height > 0 && u64::from(width) * u64::from(height) <= MAX_PIXELS,
                "이미지 크기가 올바르지 않거나 너무 큽니다. 32 메가픽셀 이하 이미지를 선택해 주세요."
            );
            depth = data[8];
            color_type = data[9];
            let valid = match color_type {
                0 => matches!(depth, 1 | 2 | 4 | 8 | 16),
                2 | 4 | 6 => matches!(depth, 8 | 16),
                3 => matches!(depth, 1 | 2 | 4 | 8),
                _ => false,
            };
            ensure!(
                valid && data[10] == 0 && data[11] == 0 && data[12] <= 1,
                "지원하지 않거나 올바르지 않은 PNG 헤더입니다."
            );
        } else {
            ensure!(kind != b"IHDR", "PNG에 IHDR 청크가 중복되어 있습니다.");
        }
        match kind {
            b"PLTE" => {
                ensure!(
                    palette_len.is_none()
                        && !idat_seen
                        && len > 0
                        && len <= 768
                        && len.is_multiple_of(3),
                    "PNG 팔레트 청크가 올바르지 않습니다."
                );
                ensure!(
                    !matches!(color_type, 0 | 4),
                    "회색조 PNG에는 PLTE 청크를 사용할 수 없습니다."
                );
                if color_type == 3 {
                    ensure!(
                        len / 3 <= 1usize << depth,
                        "PNG 팔레트가 비트 깊이 범위를 벗어납니다."
                    );
                }
                palette_len = Some(len / 3);
            }
            b"IDAT" => {
                ensure!(!idat_ended, "PNG IDAT 청크가 연속되어 있지 않습니다.");
                ensure!(
                    color_type != 3 || palette_len.is_some(),
                    "인덱스 PNG의 팔레트가 없습니다."
                );
                idat_seen = true;
            }
            b"IEND" => {
                ensure!(
                    len == 0 && idat_seen,
                    "PNG 종료 청크 또는 이미지 데이터가 올바르지 않습니다."
                );
                ensure!(
                    end == bytes.len(),
                    "PNG 종료 청크 뒤에 불필요한 데이터가 있습니다."
                );
                ended = true;
            }
            b"IHDR" => {}
            _ => ensure!(
                kind[0].is_ascii_lowercase(),
                "지원하지 않는 PNG 필수 청크입니다: {}",
                String::from_utf8_lossy(kind)
            ),
        }
        if idat_seen && kind != b"IDAT" {
            idat_ended = true;
        }
        chunks.push(Chunk {
            kind,
            data,
            raw: &bytes[pos..end],
        });
        pos = end;
        if ended {
            break;
        }
    }
    ensure!(
        ended,
        "PNG 종료 청크가 없습니다. 원본 파일을 확인해 주세요."
    );
    Ok(chunks)
}

fn text_key<'a>(chunk: &Chunk<'a>) -> Option<&'a [u8]> {
    if !matches!(chunk.kind, b"zTXt" | b"tEXt" | b"iTXt") {
        return None;
    }
    chunk.data.split(|&b| b == 0).next()
}

fn inflate_text(data: &[u8]) -> Result<Vec<u8>> {
    let mut decoder = Decompress::new(true);
    let mut decoded = vec![0; MAX_TEXT_SIZE + 1];
    let status = decoder
        .decompress(data, &mut decoded, FlushDecompress::Finish)
        .context("PNG 작성자 메타데이터의 압축을 풀 수 없습니다.")?;
    ensure!(
        decoder.total_out() <= MAX_TEXT_SIZE as u64,
        "PNG 작성자 메타데이터가 너무 큽니다."
    );
    ensure!(
        status == Status::StreamEnd && decoder.total_in() == data.len() as u64,
        "PNG 작성자 메타데이터의 압축 데이터가 잘렸거나 올바르지 않습니다."
    );
    decoded.truncate(decoder.total_out() as usize);
    Ok(decoded)
}

fn read_text(chunk: &Chunk<'_>) -> Result<String> {
    let zero = chunk
        .data
        .iter()
        .position(|&b| b == 0)
        .context("PNG 텍스트 구분자가 없습니다.")?;
    ensure!(zero > 0 && zero <= 79, "PNG 텍스트 키가 올바르지 않습니다.");
    let rest = &chunk.data[zero + 1..];
    let raw = match chunk.kind {
        b"zTXt" => {
            ensure!(
                rest.first() == Some(&0),
                "PNG 텍스트 압축 방식이 올바르지 않습니다."
            );
            inflate_text(&rest[1..])?
        }
        b"iTXt" => {
            ensure!(
                rest.len() >= 4 && rest[0] <= 1 && rest[1] == 0,
                "PNG 국제 텍스트 헤더가 올바르지 않습니다."
            );
            let lang_end = rest[2..]
                .iter()
                .position(|&b| b == 0)
                .context("PNG 언어 구분자가 없습니다.")?
                + 2;
            let name_end = rest[lang_end + 1..]
                .iter()
                .position(|&b| b == 0)
                .context("PNG 번역 키 구분자가 없습니다.")?
                + lang_end
                + 1;
            let text = &rest[name_end + 1..];
            let text = if rest[0] == 1 {
                inflate_text(text)?
            } else {
                text.to_vec()
            };
            ensure!(
                text.len() <= MAX_TEXT_SIZE,
                "PNG 작성자 메타데이터가 너무 큽니다."
            );
            return String::from_utf8(text).context("PNG 국제 텍스트가 올바른 UTF-8이 아닙니다.");
        }
        _ => rest.to_vec(),
    };
    ensure!(
        raw.len() <= MAX_TEXT_SIZE,
        "PNG 작성자 메타데이터가 너무 큽니다."
    );
    let (text, errors) = EUC_KR.decode_without_bom_handling(&raw);
    ensure!(
        !errors,
        "PNG 작성자 메타데이터를 EUC-KR로 읽을 수 없습니다."
    );
    Ok(text.into_owned())
}

impl Document {
    pub fn load(path: &Path) -> Result<Self> {
        ensure!(
            fs::metadata(path)
                .context("파일 정보를 읽을 수 없습니다.")?
                .len()
                <= MAX_FILE_SIZE as u64,
            "PNG 파일이 너무 큽니다. 128 MiB 이하 파일을 선택해 주세요."
        );
        Self::from_bytes(
            fs::read(path)
                .with_context(|| format!("파일을 읽을 수 없습니다: {}", path.display()))?,
        )
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        let chunks = parse_chunks(&bytes)?;
        let mut author = None;
        let mut authid = None;
        let mut palette = Vec::new();
        for chunk in &chunks {
            match text_key(chunk) {
                Some(b"author") => author = Some(read_text(chunk)?),
                Some(b"authid") => authid = Some(read_text(chunk)?),
                _ => {}
            }
            if chunk.kind == b"PLTE" {
                palette = chunk
                    .data
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|c| [c[0], c[1], c[2]])
                    .collect();
            }
        }
        let bit_depth = chunks[0].data[8];
        let color_type = chunks[0].data[9];
        let image = image::ImageReader::with_format(Cursor::new(&bytes), image::ImageFormat::Png)
            .decode()
            .context("PNG 이미지 데이터를 읽을 수 없습니다. 파일을 확인해 주세요.")?
            .to_rgba8();
        Ok(Self {
            bytes,
            image,
            author,
            authid,
            palette,
            bit_depth,
            color_type,
        })
    }

    pub fn is_chat(&self) -> bool {
        self.color_type == 3
            && matches!(self.bit_depth, 1 | 2)
            && !self.palette.is_empty()
            && self.palette.len() <= 4
            && self.author.as_deref().is_some_and(|s| s != "(none)")
            && self
                .authid
                .as_deref()
                .is_some_and(|s| s.parse::<i32>().is_ok())
    }
}

/// Creates a blank 2-bit picture chat with background index zero and author metadata.
pub fn new_chat(width: u32, author: &str) -> Result<Document> {
    ensure!(
        (1..=256).contains(&width),
        "새 그림대화 너비는 1 이상 256 이하로 입력해 주세요."
    );
    encode_author(author)?;
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, 96);
        encoder.set_color(png::ColorType::Indexed);
        encoder.set_depth(png::BitDepth::Two);
        encoder.set_palette(
            DEFAULT_CHAT_PALETTE
                .into_iter()
                .flatten()
                .collect::<Vec<_>>(),
        );
        let mut writer = encoder
            .write_header()
            .context("새 그림대화 PNG 헤더를 저장할 수 없습니다.")?;
        writer
            .write_image_data(&vec![0; (width as usize).div_ceil(4) * 96])
            .context("새 그림대화 이미지 데이터를 저장할 수 없습니다.")?;
        writer
            .finish()
            .context("새 그림대화 저장을 완료할 수 없습니다.")?;
    }
    Document::from_bytes(edit(&bytes, false, Some(author))?)
}

/// Replaces only PLTE and its CRC; indices, transparency and all other chunks stay intact.
pub fn replace_palette(doc: &Document, colors: &[[u8; 3]]) -> Result<Vec<u8>> {
    let chunks = parse_chunks(&doc.bytes)?;
    ensure!(
        chunks[0].data[9] == 3,
        "팔레트 편집은 인덱스 PNG에서 사용할 수 있습니다."
    );
    let palette = chunks
        .iter()
        .find(|c| c.kind == b"PLTE")
        .context("PNG 팔레트가 없습니다.")?;
    ensure!(
        colors.len() == palette.data.len() / 3,
        "새 팔레트 색상 수가 원본 팔레트와 일치하지 않습니다."
    );
    let data: Vec<_> = colors.iter().flatten().copied().collect();
    let mut out = SIGNATURE.to_vec();
    for chunk in chunks {
        if chunk.kind == b"PLTE" {
            push_chunk(&mut out, b"PLTE", &data);
        } else {
            out.extend_from_slice(chunk.raw);
        }
    }
    Ok(out)
}

/// Expands a picture chat to four palette entries without changing existing indices.
pub fn four_color_document(doc: &Document) -> Result<Document> {
    ensure!(
        doc.is_chat(),
        "Four-color editing requires a picture chat PNG."
    );
    if doc.palette.len() == 4 && doc.bit_depth == 2 {
        return Document::from_bytes(doc.bytes.clone());
    }
    let chunks = drawable_chunks(doc)?;
    let mut palette = doc.palette.clone();
    palette.extend_from_slice(&DEFAULT_CHAT_PALETTE[palette.len()..]);
    let palette_data: Vec<_> = palette.iter().flatten().copied().collect();
    let mut encoded = Vec::new();
    if doc.bit_depth == 1 {
        let pixels = indexed_pixels(doc)?;
        let ihdr = chunks[0].data;
        let width = u32::from_be_bytes(ihdr[..4].try_into()?);
        let height = u32::from_be_bytes(ihdr[4..8].try_into()?);
        let row_bytes = (width as usize).div_ceil(4);
        let mut packed = vec![0u8; row_bytes * height as usize];
        for (i, &index) in pixels.iter().enumerate() {
            let x = i % width as usize;
            packed[i / width as usize * row_bytes + x / 4] |= index << (6 - (x % 4) * 2);
        }
        let mut encoder = png::Encoder::new(&mut encoded, width, height);
        encoder.set_color(png::ColorType::Indexed);
        encoder.set_depth(png::BitDepth::Two);
        encoder.set_palette(palette_data.clone());
        let mut writer = encoder
            .write_header()
            .context("Cannot encode four-color PNG header.")?;
        writer
            .write_image_data(&packed)
            .context("Cannot encode four-color PNG pixels.")?;
        writer.finish().context("Cannot finish four-color PNG.")?;
    }
    let replacement = if encoded.is_empty() {
        Vec::new()
    } else {
        parse_chunks(&encoded)?
    };
    let mut out = SIGNATURE.to_vec();
    let mut inserted = false;
    for chunk in chunks {
        match chunk.kind {
            b"PLTE" => push_chunk(&mut out, b"PLTE", &palette_data),
            b"IHDR" if doc.bit_depth == 1 => {
                let header = replacement.iter().find(|c| c.kind == b"IHDR").unwrap();
                out.extend_from_slice(header.raw);
            }
            b"IDAT" if doc.bit_depth == 1 => {
                if !inserted {
                    for new_chunk in replacement.iter().filter(|c| c.kind == b"IDAT") {
                        out.extend_from_slice(new_chunk.raw);
                    }
                    inserted = true;
                }
            }
            _ => out.extend_from_slice(chunk.raw),
        }
    }
    Document::from_bytes(out)
}

/// Imports unmodified RGBA clipboard pixels without quantization or alpha compositing.
pub fn document_from_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Document> {
    let pixels = u64::from(width) * u64::from(height);
    ensure!(
        width > 0 && height > 0 && pixels <= MAX_PIXELS,
        "이미지 크기가 올바르지 않거나 너무 큽니다. 32 메가픽셀 이하 이미지를 선택해 주세요."
    );
    ensure!(
        rgba.len() as u64 == pixels * 4,
        "RGBA 데이터 길이가 이미지 크기와 일치하지 않습니다."
    );
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .context("클립보드 PNG 헤더를 저장할 수 없습니다.")?;
        writer
            .write_image_data(rgba)
            .context("클립보드 PNG 이미지 데이터를 저장할 수 없습니다.")?;
        writer
            .finish()
            .context("클립보드 PNG 저장을 완료할 수 없습니다.")?;
    }
    Document::from_bytes(bytes)
}

pub fn author_to_authid(author: &str) -> i32 {
    let mut hash = 5381u32;
    for unit in author.encode_utf16().take_while(|&unit| unit != 0) {
        hash = hash.wrapping_mul(33).wrapping_add(u32::from(unit));
    }
    if hash == u32::MAX { 0 } else { hash as i32 }
}

fn encode_author(author: &str) -> Result<Vec<u8>> {
    ensure!(
        !author.trim().is_empty() && author != "(none)",
        "사용할 작성자 닉네임을 입력해 주세요."
    );
    ensure!(
        !author.chars().any(char::is_control),
        "닉네임에 줄바꿈이나 제어문자를 사용할 수 없습니다."
    );
    let (bytes, _, errors) = EUC_KR.encode(author);
    ensure!(
        !errors,
        "닉네임에 EUC-KR로 저장할 수 없는 문자가 있습니다. 다른 닉네임을 입력해 주세요."
    );
    ensure!(bytes.len() <= MAX_TEXT_SIZE, "작성자 닉네임이 너무 깁니다.");
    Ok(bytes.into_owned())
}

fn push_chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = crc32fast::Hasher::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.finalize().to_be_bytes());
}

fn author_chunks(author: &str) -> Result<Vec<u8>> {
    let encoded = encode_author(author)?;
    let authid = author_to_authid(author).to_string();
    let mut out = Vec::new();
    for (key, text) in [
        (b"authid".as_slice(), authid.as_bytes()),
        (b"author".as_slice(), encoded.as_slice()),
    ] {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(text)
            .context("작성자 메타데이터를 압축할 수 없습니다.")?;
        let mut data = key.to_vec();
        data.extend_from_slice(&[0, 0]);
        data.extend(
            encoder
                .finish()
                .context("작성자 메타데이터 압축을 완료할 수 없습니다.")?,
        );
        push_chunk(&mut out, b"zTXt", &data);
    }
    Ok(out)
}

fn fixed_color(color: [u8; 3]) -> [u8; 3] {
    match color {
        [255, 255, 255] => [254, 254, 254],
        [192, 192, 192] => [193, 193, 193],
        [128, 128, 128] => [129, 129, 129],
        [0, 0, 0] => [1, 1, 1],
        _ => color,
    }
}

fn edit(bytes: &[u8], fix_palette: bool, author: Option<&str>) -> Result<Vec<u8>> {
    let chunks = parse_chunks(bytes)?;
    let metadata = author.map(author_chunks).transpose()?;
    let mut out = SIGNATURE.to_vec();
    let mut inserted = false;
    for chunk in chunks {
        if metadata.is_some() && matches!(text_key(&chunk), Some(b"author" | b"authid")) {
            continue;
        }
        if chunk.kind == b"IEND" && !inserted {
            if let Some(metadata) = &metadata {
                out.extend_from_slice(metadata);
            }
            inserted = true;
        }
        if fix_palette && chunk.kind == b"PLTE" {
            let data: Vec<_> = chunk
                .data
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|c| fixed_color([c[0], c[1], c[2]]))
                .collect();
            push_chunk(&mut out, b"PLTE", &data);
        } else {
            out.extend_from_slice(chunk.raw);
        }
        if chunk.kind == b"PLTE" && !inserted {
            if let Some(metadata) = &metadata {
                out.extend_from_slice(metadata);
            }
            inserted = true;
        }
    }
    Ok(out)
}

pub fn process(doc: &Document, settings: &Settings) -> Result<Vec<u8>> {
    if let Some(author) = &settings.author {
        encode_author(author)?;
    }
    let mode = match settings.mode {
        Mode::Auto if doc.is_chat() => Mode::Palette,
        Mode::Auto => Mode::Convert,
        mode => mode,
    };
    match mode {
        Mode::Convert => convert(doc, settings),
        Mode::Palette => {
            let chunks = parse_chunks(&doc.bytes)?;
            ensure!(
                chunks[0].data[9] == 3,
                "팔레트 편집은 인덱스 PNG에서 사용할 수 있습니다. 변환 모드를 선택해 주세요."
            );
            edit(&doc.bytes, settings.fix_palette, settings.author.as_deref())
        }
        Mode::Author => {
            let author = settings
                .author
                .as_deref()
                .context("변경할 작성자 닉네임을 입력해 주세요.")?;
            edit(&doc.bytes, false, Some(author))
        }
        Mode::Auto => unreachable!(),
    }
}

fn border_mask(pixels: &[[u8; 3]], width: usize, height: usize) -> Vec<bool> {
    let mut mask = vec![false; pixels.len()];
    let mut queue = VecDeque::new();
    let enqueue = |i: usize, mask: &mut [bool], queue: &mut VecDeque<usize>| {
        if !mask[i] && pixels[i].iter().all(|&v| v >= 245) {
            mask[i] = true;
            queue.push_back(i);
        }
    };
    for x in 0..width {
        enqueue(x, &mut mask, &mut queue);
        enqueue((height - 1) * width + x, &mut mask, &mut queue);
    }
    for y in 0..height {
        enqueue(y * width, &mut mask, &mut queue);
        enqueue(y * width + width - 1, &mut mask, &mut queue);
    }
    while let Some(i) = queue.pop_front() {
        if i >= width {
            enqueue(i - width, &mut mask, &mut queue);
        }
        if i + width < pixels.len() {
            enqueue(i + width, &mut mask, &mut queue);
        }
        if i % width > 0 {
            enqueue(i - 1, &mut mask, &mut queue);
        }
        if i % width + 1 < width {
            enqueue(i + 1, &mut mask, &mut queue);
        }
    }
    mask
}

fn nearest(color: [u8; 3], palette: &[[u8; 3]]) -> usize {
    palette
        .iter()
        .enumerate()
        .min_by_key(|(_, candidate)| {
            (0..3)
                .map(|i| (i32::from(color[i]) - i32::from(candidate[i])).pow(2))
                .sum::<i32>()
        })
        .map(|(i, _)| i)
        .unwrap_or(0)
}

fn quantize(samples: &[[u8; 3]], count: usize) -> Vec<[u8; 3]> {
    let mut unique = Vec::new();
    for &pixel in samples {
        if !unique.contains(&pixel) {
            unique.push(pixel);
        }
        if unique.len() > count {
            break;
        }
    }
    if unique.len() <= count {
        return unique;
    }
    let rgba: Vec<_> = samples
        .iter()
        .flat_map(|p| [p[0], p[1], p[2], 255])
        .collect();
    let network = color_quant::NeuQuant::new(1, 64, &rgba);
    let candidates: Vec<_> = network
        .color_map_rgb()
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| [p[0], p[1], p[2]])
        .collect();
    let mut weights = vec![0u64; candidates.len()];
    for &pixel in samples {
        weights[nearest(pixel, &candidates)] += 1;
    }
    let first = weights
        .iter()
        .enumerate()
        .max_by_key(|&(i, weight)| (*weight, std::cmp::Reverse(i)))
        .unwrap()
        .0;
    let mut palette = vec![candidates[first]];
    // Reduce the library's supported 64-color map to weighted, well-separated seeds.
    while palette.len() < count {
        let best = candidates
            .iter()
            .enumerate()
            .max_by_key(|&(i, color)| {
                let closest = palette[nearest(*color, &palette)];
                let distance: u64 = (0..3)
                    .map(|c| (i32::from(color[c]) - i32::from(closest[c])).pow(2) as u64)
                    .sum();
                (weights[i] * distance, std::cmp::Reverse(i))
            })
            .unwrap()
            .0;
        if palette.contains(&candidates[best]) {
            break;
        }
        palette.push(candidates[best]);
    }
    // NeuQuant supplies the domain algorithm; a bounded Lloyd refinement reduces small-palette error.
    for _ in 0..12 {
        let mut sums = vec![[0u64; 4]; palette.len()];
        for &pixel in samples {
            let sum = &mut sums[nearest(pixel, &palette)];
            for c in 0..3 {
                sum[c] += u64::from(pixel[c]);
            }
            sum[3] += 1;
        }
        let mut next = palette.clone();
        for (color, sum) in next.iter_mut().zip(sums) {
            for c in 0..3 {
                if let Some(value) = (sum[c] + sum[3] / 2).checked_div(sum[3]) {
                    color[c] = value as u8;
                }
            }
        }
        if next == palette {
            break;
        }
        palette = next;
    }
    let mut groups = vec![Vec::new(); palette.len()];
    for &pixel in samples {
        groups[nearest(pixel, &palette)].push(pixel);
    }
    for (color, group) in palette.iter_mut().zip(groups.iter_mut()) {
        if group.is_empty() {
            continue;
        }
        if u32::from(color[0]) * 299 + u32::from(color[1]) * 587 + u32::from(color[2]) * 114
            >= 170_000
        {
            for c in 0..3 {
                group.sort_unstable_by_key(|pixel| pixel[c]);
                let median = if group.len() % 2 == 0 {
                    (f64::from(group[group.len() / 2 - 1][c])
                        + f64::from(group[group.len() / 2][c]))
                        / 2.0
                } else {
                    f64::from(group[group.len() / 2][c])
                };
                color[c] = (f64::from(color[c]) * 0.25 + median * 0.75).round() as u8;
            }
        }
    }
    palette.sort_unstable();
    palette.dedup();
    palette
}

fn convert(doc: &Document, settings: &Settings) -> Result<Vec<u8>> {
    let author = settings
        .author
        .as_deref()
        .or(doc.author.as_deref())
        .context("새 그림대화에 사용할 작성자 닉네임을 입력해 주세요.")?;
    encode_author(author)?;
    let (width, height) = doc.image.dimensions();
    ensure!(
        width > 0 && height > 0 && u64::from(width) * u64::from(height) <= MAX_PIXELS,
        "변환할 이미지 크기가 올바르지 않거나 너무 큽니다."
    );
    // Composite before resizing so hidden RGB values cannot bleed into translucent edges.
    let mut opaque = doc.image.clone();
    for p in opaque.pixels_mut() {
        let a = u32::from(p[3]);
        for c in 0..3 {
            p[c] = ((u32::from(p[c]) * a + 255 * (255 - a) + 127) / 255) as u8;
        }
        p[3] = 255;
    }
    if let Some((max_width, max_height)) = settings.resize {
        ensure!(
            max_width > 0 && max_height > 0,
            "최대 너비와 높이를 1 이상으로 입력해 주세요."
        );
        if width > max_width || height > max_height {
            let (new_width, new_height) = if u64::from(max_width) * u64::from(height)
                <= u64::from(max_height) * u64::from(width)
            {
                (
                    max_width,
                    (u64::from(height) * u64::from(max_width) / u64::from(width)).max(1) as u32,
                )
            } else {
                (
                    (u64::from(width) * u64::from(max_height) / u64::from(height)).max(1) as u32,
                    max_height,
                )
            };
            opaque = image::imageops::resize(&opaque, new_width, new_height, FilterType::Lanczos3);
        }
    }
    let (width, height) = opaque.dimensions();
    let pixels: Vec<_> = opaque.pixels().map(|p| [p[0], p[1], p[2]]).collect();
    let mask = if settings.preserve_white_background {
        border_mask(&pixels, width as usize, height as usize)
    } else {
        vec![false; pixels.len()]
    };
    let preserve =
        settings.preserve_white_background && mask.iter().filter(|&&v| v).count() * 4 >= mask.len();
    let stride = pixels.len().div_ceil(65536).max(1);
    let mut samples: Vec<_> = pixels
        .iter()
        .zip(&mask)
        .enumerate()
        .filter(|(i, (pixel, background))| {
            i % stride == 0 && !(preserve && (**background || pixel.iter().all(|&v| v >= 245)))
        })
        .map(|(_, (&p, _))| p)
        .collect();
    // A sparse foreground must not disappear between regular sampling positions.
    if samples.is_empty() {
        if let Some((&pixel, _)) = pixels.iter().zip(&mask).find(|(pixel, background)| {
            !(preserve && (**background || pixel.iter().all(|&v| v >= 245)))
        }) {
            samples.push(pixel);
        } else if let Some((&pixel, _)) = pixels
            .iter()
            .zip(&mask)
            .find(|(_, background)| !(preserve && **background))
        {
            samples.push(pixel);
        }
    }
    let mut palette = if preserve {
        vec![[255, 255, 250]]
    } else {
        Vec::new()
    };
    if !samples.is_empty() {
        palette.extend(quantize(&samples, if preserve { 3 } else { 4 }));
    }
    ensure!(
        !palette.is_empty(),
        "변환할 이미지 색상을 찾을 수 없습니다."
    );
    if settings.fix_palette {
        for color in &mut palette {
            *color = fixed_color(*color);
        }
    }
    let row_bytes = (width as usize).div_ceil(4);
    let mut packed = vec![0u8; row_bytes * height as usize];
    for (i, &pixel) in pixels.iter().enumerate() {
        let index = if preserve && mask[i] {
            0
        } else {
            nearest(pixel, &palette)
        };
        let x = i % width as usize;
        packed[i / width as usize * row_bytes + x / 4] |= (index as u8) << (6 - (x % 4) * 2);
    }
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Indexed);
        encoder.set_depth(png::BitDepth::Two);
        encoder.set_palette(palette.iter().flatten().copied().collect::<Vec<_>>());
        let mut writer = encoder
            .write_header()
            .context("PNG 헤더를 저장할 수 없습니다.")?;
        writer
            .write_image_data(&packed)
            .context("PNG 이미지 데이터를 저장할 수 없습니다.")?;
        writer.finish().context("PNG 저장을 완료할 수 없습니다.")?;
    }
    edit(&bytes, false, Some(author))
}

fn drawable_chunks(doc: &Document) -> Result<Vec<Chunk<'_>>> {
    ensure!(
        doc.is_chat(),
        "그리기는 4색 이하의 1·2비트 그림대화 PNG에서 사용할 수 있습니다."
    );
    let chunks = parse_chunks(&doc.bytes)?;
    ensure!(
        chunks[0].data[9] == 3 && matches!(chunks[0].data[8], 1 | 2),
        "그림대화 PNG의 형식이 올바르지 않습니다."
    );
    ensure!(
        !chunks
            .iter()
            .any(|c| matches!(c.kind, b"acTL" | b"fcTL" | b"fdAT")),
        "애니메이션 PNG(APNG)에는 그릴 수 없습니다. 일반 PNG로 변환해 주세요."
    );
    Ok(chunks)
}

/// Returns palette colors with their original tRNS alpha, defaulting to opaque.
pub fn palette_rgba(doc: &Document) -> Vec<[u8; 4]> {
    let chunks = parse_chunks(&doc.bytes).unwrap_or_default();
    let alpha = chunks
        .iter()
        .find(|c| c.kind == b"tRNS")
        .map(|c| c.data)
        .unwrap_or(&[]);
    doc.palette
        .iter()
        .enumerate()
        .map(|(i, c)| [c[0], c[1], c[2], alpha.get(i).copied().unwrap_or(255)])
        .collect()
}

/// Returns original, unpacked palette indices; duplicate palette entries stay distinct.
pub fn indexed_pixels(doc: &Document) -> Result<Vec<u8>> {
    let chunks = drawable_chunks(doc)?;
    let palette_len = chunks
        .iter()
        .find(|c| c.kind == b"PLTE")
        .context("PNG 팔레트가 없습니다.")?
        .data
        .len()
        / 3;
    let mut decoder = png::Decoder::new(Cursor::new(&doc.bytes));
    decoder.set_transformations(png::Transformations::IDENTITY);
    decoder.set_ignore_text_chunk(true);
    let mut reader = decoder
        .read_info()
        .context("PNG 인덱스 정보를 읽을 수 없습니다.")?;
    let mut buffer = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buffer)
        .context("PNG 인덱스 데이터를 읽을 수 없습니다.")?;
    ensure!(
        info.color_type == png::ColorType::Indexed
            && matches!(info.bit_depth, png::BitDepth::One | png::BitDepth::Two),
        "PNG 인덱스 형식이 올바르지 않습니다."
    );
    let depth = info.bit_depth as usize;
    let per_byte = 8 / depth;
    let index_mask = (1u8 << depth) - 1;
    let mut pixels = Vec::with_capacity(info.width as usize * info.height as usize);
    for y in 0..info.height as usize {
        for x in 0..info.width as usize {
            let index = (buffer[y * info.line_size + x / per_byte]
                >> (8 - depth - (x % per_byte) * depth))
                & index_mask;
            ensure!(
                (index as usize) < palette_len,
                "PNG 이미지에 팔레트 범위를 벗어난 인덱스가 있습니다."
            );
            pixels.push(index);
        }
    }
    Ok(pixels)
}

/// Replaces image samples only, retaining raw palette, transparency and ancillary chunks.
pub fn paint_indexed(doc: &Document, pixels: &[u8]) -> Result<Vec<u8>> {
    resize_indexed(doc, doc.image.width(), doc.image.height(), pixels)
}

/// Changes canvas dimensions and samples, preserving palette, transparency and metadata.
pub fn resize_indexed(doc: &Document, width: u32, height: u32, pixels: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        width > 0 && height > 0 && u64::from(width) * u64::from(height) <= MAX_PIXELS,
        "캔버스 크기가 올바르지 않습니다."
    );
    let chunks = drawable_chunks(doc)?;
    let ihdr = chunks[0].data;
    let depth = ihdr[8] as usize;
    ensure!(
        pixels.len() == width as usize * height as usize,
        "그리기 픽셀 수가 이미지 크기와 일치하지 않습니다."
    );
    let palette = chunks
        .iter()
        .find(|c| c.kind == b"PLTE")
        .context("PNG 팔레트가 없습니다.")?
        .data;
    ensure!(
        pixels
            .iter()
            .all(|&index| (index as usize) < palette.len() / 3),
        "선택한 색상 인덱스가 팔레트 범위를 벗어납니다."
    );
    let per_byte = 8 / depth;
    let row_bytes = (width as usize).div_ceil(per_byte);
    let mut packed = vec![0u8; row_bytes * height as usize];
    for (i, &index) in pixels.iter().enumerate() {
        let x = i % width as usize;
        packed[i / width as usize * row_bytes + x / per_byte] |=
            index << (8 - depth - (x % per_byte) * depth);
    }
    let mut encoded = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut encoded, width, height);
        encoder.set_color(png::ColorType::Indexed);
        encoder.set_depth(if depth == 1 {
            png::BitDepth::One
        } else {
            png::BitDepth::Two
        });
        encoder.set_palette(palette.to_vec());
        let mut writer = encoder
            .write_header()
            .context("그린 PNG의 헤더를 저장할 수 없습니다.")?;
        writer
            .write_image_data(&packed)
            .context("그린 PNG의 이미지 데이터를 저장할 수 없습니다.")?;
        writer
            .finish()
            .context("그린 PNG 저장을 완료할 수 없습니다.")?;
    }
    let replacement = parse_chunks(&encoded)?;
    let mut out = SIGNATURE.to_vec();
    let mut inserted = false;
    for chunk in chunks {
        if chunk.kind == b"IDAT" {
            if !inserted {
                for new_chunk in replacement.iter().filter(|c| c.kind == b"IDAT") {
                    out.extend_from_slice(new_chunk.raw);
                }
                inserted = true;
            }
        } else if chunk.kind == b"IHDR" {
            let mut header = ihdr.to_vec();
            header[..4].copy_from_slice(&width.to_be_bytes());
            header[4..8].copy_from_slice(&height.to_be_bytes());
            header[12] = 0;
            push_chunk(&mut out, b"IHDR", &header);
        } else {
            out.extend_from_slice(chunk.raw);
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "../tests/engine/mod.rs"]
mod tests;
