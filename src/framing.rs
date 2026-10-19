//! Output-space framing: bounded output buffers even for very large sources or zoom levels.
use crate::engine::{self, Document, Settings};
use anyhow::{Result, ensure};
use image::{GenericImageView, Pixel, Rgba, RgbaImage};

struct Matte<'a> {
    image: &'a RgbaImage,
    background: Rgba<u8>,
}

impl GenericImageView for Matte<'_> {
    type Pixel = Rgba<u8>;
    fn dimensions(&self) -> (u32, u32) {
        self.image.dimensions()
    }
    fn get_pixel(&self, x: u32, y: u32) -> Rgba<u8> {
        let mut pixel = self.background;
        pixel.blend(self.image.get_pixel(x, y));
        pixel
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sampling {
    Nearest,
    Smooth,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteMode {
    Keep,
    Adaptive,
    Default,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub offset: [f32; 2],
    pub sampling: Sampling,
    pub background: [u8; 3],
    pub background_index: u8,
}

impl Frame {
    pub fn new(source: &Document) -> Self {
        let mut result = Self {
            width: 256,
            height: 96,
            scale: 1.0,
            offset: [0.0; 2],
            sampling: if source.is_chat() {
                Sampling::Nearest
            } else {
                Sampling::Smooth
            },
            background: [255; 3],
            background_index: 0,
        };
        result.fit(source.image.dimensions(), false);
        result
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=256).contains(&self.width) && (1..=96).contains(&self.height),
            "출력 크기는 가로 1~256, 세로 1~96픽셀이어야 합니다."
        );
        ensure!(
            self.scale.is_finite()
                && (0.0000001..=64.0).contains(&self.scale)
                && self.offset.iter().all(|v| v.is_finite() && v.abs() <= 1e9),
            "이미지 배율 또는 위치가 올바르지 않습니다."
        );
        Ok(())
    }

    pub fn center(&mut self, source: (u32, u32)) {
        self.offset = [
            (self.width as f32 - source.0 as f32 * self.scale) / 2.0,
            (self.height as f32 - source.1 as f32 * self.scale) / 2.0,
        ];
    }

    pub fn fit(&mut self, source: (u32, u32), cover: bool) {
        let x = self.width as f32 / source.0 as f32;
        let y = self.height as f32 / source.1 as f32;
        self.scale = if cover { x.max(y) } else { x.min(y) }.clamp(0.0000001, 64.0);
        self.center(source);
    }

    pub fn set_size(&mut self, width: u32, height: u32) {
        self.offset[0] += (width as f32 - self.width as f32) / 2.0;
        self.offset[1] += (height as f32 - self.height as f32) / 2.0;
        self.width = width;
        self.height = height;
    }

    pub fn zoom(&mut self, scale: f32) {
        self.zoom_at(scale, [self.width as f32 / 2.0, self.height as f32 / 2.0]);
    }

    pub fn zoom_at(&mut self, scale: f32, anchor: [f32; 2]) {
        let scale = scale.clamp(0.0000001, 64.0);
        let ratio = scale / self.scale;
        self.offset = [
            anchor[0] - (anchor[0] - self.offset[0]) * ratio,
            anchor[1] - (anchor[1] - self.offset[1]) * ratio,
        ];
        self.scale = scale;
    }

    fn source_point(&self, x: u32, y: u32) -> (f32, f32) {
        (
            (x as f32 + 0.5 - self.offset[0]) / self.scale,
            (y as f32 + 0.5 - self.offset[1]) / self.scale,
        )
    }

    pub fn rgba(&self, source: &RgbaImage) -> Result<RgbaImage> {
        self.validate()?;
        let matte = Matte {
            image: source,
            background: Rgba([
                self.background[0],
                self.background[1],
                self.background[2],
                255,
            ]),
        };
        // Prefilter only the visible source window; never allocate a zoom-sized image.
        let filtered = if self.sampling == Sampling::Smooth && self.scale < 1.0 {
            let pad = 3.0 / self.scale + 1.0;
            let x0 = (-self.offset[0] / self.scale - pad)
                .floor()
                .clamp(0.0, source.width() as f32) as u32;
            let y0 = (-self.offset[1] / self.scale - pad)
                .floor()
                .clamp(0.0, source.height() as f32) as u32;
            let x1 = ((self.width as f32 - self.offset[0]) / self.scale + pad)
                .ceil()
                .clamp(0.0, source.width() as f32) as u32;
            let y1 = ((self.height as f32 - self.offset[1]) / self.scale + pad)
                .ceil()
                .clamp(0.0, source.height() as f32) as u32;
            if x1 > x0 && y1 > y0 {
                let crop = image::imageops::crop_imm(&matte, x0, y0, x1 - x0, y1 - y0);
                Some((
                    image::imageops::resize(
                        &*crop,
                        ((x1 - x0) as f32 * self.scale).ceil().max(1.0) as u32,
                        ((y1 - y0) as f32 * self.scale).ceil().max(1.0) as u32,
                        image::imageops::FilterType::Lanczos3,
                    ),
                    x0,
                    y0,
                    x1 - x0,
                    y1 - y0,
                ))
            } else {
                None
            }
        } else {
            None
        };
        let mut output = RgbaImage::from_pixel(
            self.width,
            self.height,
            Rgba([
                self.background[0],
                self.background[1],
                self.background[2],
                255,
            ]),
        );
        for (x, y, pixel) in output.enumerate_pixels_mut() {
            let (sx, sy) = self.source_point(x, y);
            if sx < 0.0 || sy < 0.0 || sx >= source.width() as f32 || sy >= source.height() as f32 {
                continue;
            }
            let sample = if self.sampling == Sampling::Nearest {
                *source.get_pixel(sx.floor() as u32, sy.floor() as u32)
            } else if let Some((image, x0, y0, width, height)) = &filtered {
                image::imageops::sample_bilinear(
                    image,
                    ((sx - *x0 as f32) / *width as f32).clamp(0.0, 1.0),
                    ((sy - *y0 as f32) / *height as f32).clamp(0.0, 1.0),
                )
                .unwrap()
            } else {
                image::imageops::sample_bilinear(
                    &matte,
                    (sx / source.width() as f32).clamp(0.0, 1.0),
                    (sy / source.height() as f32).clamp(0.0, 1.0),
                )
                .unwrap_or(*source.get_pixel(sx.floor() as u32, sy.floor() as u32))
            };
            pixel.blend(&sample);
        }
        Ok(output)
    }

    /// Checks every source pixel intersecting the selected region, before resampling.
    pub fn selected_colors(&self, source: &RgbaImage) -> Result<Option<Vec<[u8; 3]>>> {
        self.validate()?;
        let left = -self.offset[0] / self.scale;
        let top = -self.offset[1] / self.scale;
        let right = (self.width as f32 - self.offset[0]) / self.scale;
        let bottom = (self.height as f32 - self.offset[1]) / self.scale;
        let x0 = left.floor().clamp(0.0, source.width() as f32) as u32;
        let y0 = top.floor().clamp(0.0, source.height() as f32) as u32;
        let x1 = right.ceil().clamp(0.0, source.width() as f32) as u32;
        let y1 = bottom.ceil().clamp(0.0, source.height() as f32) as u32;
        let mut colors = Vec::new();
        if left < 0.0
            || top < 0.0
            || right > source.width() as f32
            || bottom > source.height() as f32
        {
            colors.push(self.background);
        }
        let matte = Matte {
            image: source,
            background: Rgba([
                self.background[0],
                self.background[1],
                self.background[2],
                255,
            ]),
        };
        for y in y0..y1 {
            for x in x0..x1 {
                let pixel = matte.get_pixel(x, y);
                let color = [pixel[0], pixel[1], pixel[2]];
                if !colors.contains(&color) {
                    colors.push(color);
                    if colors.len() > 4 {
                        return Ok(None);
                    }
                }
            }
        }
        Ok(Some(colors))
    }

    pub fn indexed(&self, source: &Document) -> Result<Document> {
        self.validate()?;
        ensure!(
            (self.background_index as usize) < source.palette.len(),
            "여백 색상이 팔레트 범위를 벗어납니다."
        );
        let input = engine::indexed_pixels(source)?;
        let mut pixels = vec![self.background_index; (self.width * self.height) as usize];
        for y in 0..self.height {
            for x in 0..self.width {
                let (sx, sy) = self.source_point(x, y);
                if sx >= 0.0
                    && sy >= 0.0
                    && sx < source.image.width() as f32
                    && sy < source.image.height() as f32
                {
                    pixels[(y * self.width + x) as usize] = input
                        [sy.floor() as usize * source.image.width() as usize + sx.floor() as usize];
                }
            }
        }
        Document::from_bytes(engine::resize_indexed(
            source,
            self.width,
            self.height,
            &pixels,
        )?)
    }

    pub fn render(
        &self,
        source: &Document,
        mode: PaletteMode,
        settings: &Settings,
    ) -> Result<Document> {
        if mode == PaletteMode::Keep {
            ensure!(
                source.is_chat(),
                "원본 팔레트 유지는 그림대화 PNG에서만 사용할 수 있습니다."
            );
            return self.indexed(source);
        }
        let rgba = self.rgba(&source.image)?;
        if mode == PaletteMode::Default {
            let author = settings.author.as_deref().unwrap_or("Test");
            let base = engine::new_chat(128, author)?;
            let palette = if settings.fix_palette {
                engine::DEFAULT_CHAT_PALETTE
            } else {
                [[255; 3], [192; 3], [128; 3], [0; 3]]
            };
            let base = Document::from_bytes(engine::replace_palette(&base, &palette)?)?;
            let pixels = rgba
                .pixels()
                .map(|pixel| {
                    palette
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, color)| {
                            (0..3)
                                .map(|c| (i32::from(pixel[c]) - i32::from(color[c])).pow(2))
                                .sum::<i32>()
                        })
                        .unwrap()
                        .0 as u8
                })
                .collect::<Vec<_>>();
            return Document::from_bytes(engine::resize_indexed(
                &base,
                self.width,
                self.height,
                &pixels,
            )?);
        }
        let raw = engine::document_from_rgba(self.width, self.height, rgba.as_raw())?;
        Document::from_bytes(engine::process(&raw, settings)?)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn selected_colors_scan_crop_before_sampling_and_include_visible_matte() {
        use super::*;
        let colors = [
            [20, 30, 40, 255],
            [50, 60, 70, 255],
            [80, 90, 100, 255],
            [110, 120, 130, 255],
            [140, 150, 160, 255],
        ];
        let mut source = RgbaImage::from_fn(20, 4, |x, _| Rgba(colors[(x % 4) as usize]));
        source.put_pixel(19, 0, Rgba(colors[4]));
        let doc = engine::document_from_rgba(20, 4, source.as_raw()).unwrap();
        let mut frame = Frame::new(&doc);
        frame.width = 16;
        frame.height = 4;
        frame.scale = 1.0;
        frame.offset = [0.0, 0.0];
        assert_eq!(frame.selected_colors(&source).unwrap().unwrap().len(), 4);
        frame.width = 20;
        assert!(frame.selected_colors(&source).unwrap().is_none());
        // A rare fifth source color must not disappear from detection during shrink.
        frame.width = 10;
        frame.height = 2;
        frame.scale = 0.5;
        assert!(frame.selected_colors(&source).unwrap().is_none());
        frame.width = 16;
        frame.height = 4;
        frame.scale = 1.0;
        frame.offset = [1.0, 0.0];
        assert!(frame.selected_colors(&source).unwrap().is_none());
        frame.background = [20, 30, 40];
        assert_eq!(frame.selected_colors(&source).unwrap().unwrap().len(), 4);
        for pixel in source.pixels_mut() {
            pixel[3] = 0;
        }
        assert_eq!(
            frame.selected_colors(&source).unwrap(),
            Some(vec![[20, 30, 40]])
        );
        frame.offset = [-100.0, -100.0];
        assert_eq!(
            frame.selected_colors(&source).unwrap(),
            Some(vec![[20, 30, 40]])
        );
    }

    use super::*;

    #[test]
    fn narrow_canvases_render_in_every_palette_mode() {
        let source = engine::new_chat(256, "Test").unwrap();
        let settings = Settings {
            author: Some("Test".into()),
            ..Default::default()
        };
        for width in [1, 2, 3, 48, 95, 96, 127] {
            let mut frame = Frame::new(&source);
            frame.set_size(width, 48);
            frame.fit(source.image.dimensions(), false);
            for mode in [
                PaletteMode::Keep,
                PaletteMode::Adaptive,
                PaletteMode::Default,
            ] {
                let doc = frame.render(&source, mode, &settings).unwrap();
                assert_eq!(doc.image.dimensions(), (width, 48));
                assert_eq!(
                    engine::indexed_pixels(&doc).unwrap().len(),
                    width as usize * 48
                );
            }
        }
    }

    #[test]
    fn short_canvases_render_in_every_palette_mode() {
        let source = engine::new_chat(96, "Test").unwrap();
        let settings = Settings {
            author: Some("Test".into()),
            ..Default::default()
        };
        for (width, height) in [(1, 1), (96, 1), (96, 16), (96, 47)] {
            let mut frame = Frame::new(&source);
            frame.set_size(width, height);
            frame.fit(source.image.dimensions(), false);
            for mode in [
                PaletteMode::Keep,
                PaletteMode::Adaptive,
                PaletteMode::Default,
            ] {
                let doc = frame.render(&source, mode, &settings).unwrap();
                assert_eq!(doc.image.dimensions(), (width, height));
                assert_eq!(
                    engine::indexed_pixels(&doc).unwrap().len(),
                    (width * height) as usize
                );
            }
        }
        let mut frame = Frame::new(&source);
        frame.height = 0;
        assert!(frame.validate().is_err());
    }

    #[test]
    fn smooth_downscale_filters_aliasing_and_composites_alpha_before_filtering() {
        let rgba = RgbaImage::from_fn(1024, 384, |x, _| {
            if x % 2 == 0 {
                Rgba([255, 255, 255, 255])
            } else {
                Rgba([0, 0, 0, 0])
            }
        });
        let source = engine::document_from_rgba(1024, 384, rgba.as_raw()).unwrap();
        let frame = Frame::new(&source);
        let output = frame.rgba(&rgba).unwrap();
        assert!(output.pixels().all(|p| p.0 == [255; 4]));
        let stripes = RgbaImage::from_fn(1024, 384, |x, _| {
            Rgba(if x % 2 == 0 { [255; 4] } else { [0, 0, 0, 255] })
        });
        let output = frame.rgba(&stripes).unwrap();
        let center = output.get_pixel(128, 48)[0];
        assert!(
            (120..=136).contains(&center),
            "downsample aliased: {center}"
        );
    }
    #[test]
    fn indexed_crop_keeps_palette_indices_author_and_transparency() {
        let source = engine::new_chat(256, "Test").unwrap();
        let mut pixels = vec![0; 256 * 96];
        pixels[10 * 256 + 20] = 3;
        let source =
            Document::from_bytes(engine::paint_indexed(&source, &pixels).unwrap()).unwrap();
        let mut frame = Frame::new(&source);
        frame.set_size(128, 48);
        frame.scale = 1.0;
        frame.offset = [-20.0, -10.0];
        let output = frame.indexed(&source).unwrap();
        assert_eq!(output.image.dimensions(), (128, 48));
        assert_eq!(engine::indexed_pixels(&output).unwrap()[0], 3);
        assert_eq!(source.palette, output.palette);
        assert_eq!(source.author, output.author);
        assert_eq!(source.authid, output.authid);
    }
    #[test]
    fn fitting_and_zoom_keep_center_and_bound_output_memory() {
        let source = engine::new_chat(128, "Test").unwrap();
        let mut frame = Frame::new(&source);
        frame.fit((10000, 5000), true);
        assert_eq!(frame.rgba(&source.image).unwrap().dimensions(), (256, 96));
        let center = [
            (128.0 - frame.offset[0]) / frame.scale,
            (48.0 - frame.offset[1]) / frame.scale,
        ];
        frame.zoom(frame.scale * 2.0);
        assert!(((128.0 - frame.offset[0]) / frame.scale - center[0]).abs() < 0.01);
        assert!(((48.0 - frame.offset[1]) / frame.scale - center[1]).abs() < 0.01);
        frame.scale = f32::NAN;
        assert!(frame.rgba(&source.image).is_err());
        frame.scale = 1.0;
        frame.width = 257;
        assert!(frame.indexed(&source).is_err());
    }
    #[test]
    fn rgba_identity_and_default_palette_conversion_are_exact() {
        let rgba = RgbaImage::from_fn(128, 48, |x, y| Rgba([x as u8, y as u8, 255, 255]));
        let source = engine::document_from_rgba(128, 48, rgba.as_raw()).unwrap();
        let mut frame = Frame::new(&source);
        frame.set_size(128, 48);
        frame.scale = 1.0;
        frame.offset = [0.0; 2];
        for sampling in [Sampling::Nearest, Sampling::Smooth] {
            frame.sampling = sampling;
            assert_eq!(frame.rgba(&rgba).unwrap(), rgba);
        }
        let settings = Settings {
            author: Some("Test".into()),
            ..Default::default()
        };
        let output = frame
            .render(&source, PaletteMode::Default, &settings)
            .unwrap();
        assert!(output.is_chat());
        assert_eq!(output.palette, engine::DEFAULT_CHAT_PALETTE);
        assert_eq!(output.image.dimensions(), (128, 48));
    }
}
