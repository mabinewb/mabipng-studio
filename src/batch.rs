//! Disk-only batch conversion, independent from open editor documents.
use crate::{
    engine::{self, Document, Mode, Settings},
    framing::{Frame, PaletteMode, Sampling},
    storage,
};
use anyhow::{Result, ensure};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Options {
    pub output_dir: PathBuf,
    pub author: String,
    pub replace_existing_author: bool,
    pub fix_palette: bool,
    pub width: u32,
    pub height: u32,
    pub cover: bool,
    pub sampling: Sampling,
    pub palette: PaletteMode,
    pub preserve_white: bool,
    pub resize_existing: bool,
}

pub struct ItemResult {
    pub source: PathBuf,
    pub output: Result<PathBuf, String>,
}

pub fn convert_one(path: &Path, options: &Options) -> Result<PathBuf> {
    ensure!(
        path.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("png")),
        "PNG 파일을 선택해 주시기 바랍니다."
    );
    let source = Document::load(path)?;
    let settings = Settings {
        mode: Mode::Auto,
        author: if !source.is_chat() || options.replace_existing_author {
            Some(options.author.clone())
        } else {
            None
        },
        fix_palette: options.fix_palette,
        preserve_white_background: options.preserve_white,
        resize: None,
    };
    let output = if source.is_chat() && !options.resize_existing {
        Document::from_bytes(engine::process(&source, &settings)?)?
    } else {
        let mut frame = Frame::new(&source);
        frame.set_size(options.width, options.height);
        frame.fit(source.image.dimensions(), options.cover);
        frame.sampling = options.sampling;
        let mode = if source.is_chat() {
            PaletteMode::Keep
        } else {
            options.palette
        };
        let doc = frame.render(&source, mode, &settings)?;
        Document::from_bytes(engine::process(&doc, &settings)?)?
    };
    let filename = format!(
        "chat_{}_{}.png",
        chrono::Local::now().format("%Y%m%d_%H%M%S"),
        storage::safe_name(output.author.as_deref().unwrap_or("author"))
    );
    storage::save_unique(&options.output_dir, &filename, &output.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn options(output: PathBuf) -> Options {
        Options {
            output_dir: output,
            author: "New Author".into(),
            replace_existing_author: false,
            fix_palette: true,
            width: 128,
            height: 48,
            cover: false,
            sampling: Sampling::Smooth,
            palette: PaletteMode::Adaptive,
            preserve_white: true,
            resize_existing: false,
        }
    }

    #[test]
    fn ordinary_batch_conversion_accepts_narrow_widths() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("input.png");
        let image =
            engine::document_from_rgba(96, 96, &[40, 80, 120, 255].repeat(96 * 96)).unwrap();
        std::fs::write(&source, image.bytes).unwrap();
        let mut settings = options(root.path().join("output"));
        for width in [1, 96] {
            settings.width = width;
            let output = convert_one(&source, &settings).unwrap();
            assert_eq!(
                Document::load(&output).unwrap().image.dimensions(),
                (width, 48)
            );
        }
    }
    #[test]
    fn mixed_inputs_preserve_originals_and_existing_author() {
        let root = tempfile::tempdir().unwrap();
        let chat_path = root.path().join("source.png");
        let regular_path = root.path().join("input.png");
        let chat = engine::new_chat(256, "Existing Author").unwrap();
        let regular =
            engine::document_from_rgba(400, 200, &[10, 90, 160, 255].repeat(400 * 200)).unwrap();
        std::fs::write(&chat_path, &chat.bytes).unwrap();
        std::fs::write(&regular_path, &regular.bytes).unwrap();
        let settings = options(root.path().join("output"));
        let one = Document::load(&convert_one(&chat_path, &settings).unwrap()).unwrap();
        let two = Document::load(&convert_one(&regular_path, &settings).unwrap()).unwrap();
        assert_eq!(one.author.as_deref(), Some("Existing Author"));
        assert_eq!(one.image.dimensions(), (256, 96));
        assert_eq!(two.author.as_deref(), Some("New Author"));
        assert_eq!(two.image.dimensions(), (128, 48));
        assert_eq!(std::fs::read(&chat_path).unwrap(), chat.bytes);
        assert_eq!(std::fs::read(&regular_path).unwrap(), regular.bytes);
    }
    #[test]
    fn resized_existing_chat_and_repeated_names_remain_safe() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.png");
        std::fs::write(&source, engine::new_chat(256, "Existing").unwrap().bytes).unwrap();
        let mut settings = options(root.path().to_owned());
        settings.resize_existing = true;
        settings.replace_existing_author = true;
        let first = convert_one(&source, &settings).unwrap();
        let second = convert_one(&source, &settings).unwrap();
        assert_ne!(first, second);
        let doc = Document::load(&first).unwrap();
        assert_eq!(doc.image.dimensions(), (128, 48));
        assert_eq!(doc.author.as_deref(), Some("New Author"));
        assert_eq!(
            Document::load(&source).unwrap().image.dimensions(),
            (256, 96)
        );
    }
}
