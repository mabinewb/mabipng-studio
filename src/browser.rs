use anyhow::{Context, Result};
use image::{
    RgbaImage,
    imageops::{self, FilterType},
};
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

/// Lists only regular PNG files directly inside the requested game folder.
pub fn list_pngs(folder: &Path) -> Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("Cannot list {}", folder.display()));
        }
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
            || !entry.file_type()?.is_file()
        {
            continue;
        }
        files.push((path, entry.metadata()?.modified().ok()));
    }
    // Missing modification times sort last; filename order also resolves ties.
    files.sort_by(|(a, a_time), (b, b_time)| {
        b_time
            .cmp(a_time)
            .then_with(|| a.file_name().cmp(&b.file_name()))
    });
    Ok(files.into_iter().map(|(path, _)| path).collect())
}

pub struct Thumbnail {
    pub image: RgbaImage,
    pub original_size: (u32, u32),
}

/// Loads through the engine's bounded PNG validation without writing the source.
pub fn thumbnail(path: &Path) -> Result<Thumbnail> {
    let document = crate::engine::Document::load(path)?;
    let original_size = document.image.dimensions();
    let (width, height) = original_size;
    let image = if width <= 112 && height <= 64 {
        document.image
    } else {
        let (new_width, new_height) = if u64::from(width) * 64 >= u64::from(height) * 112 {
            (
                112,
                (u64::from(height) * 112 / u64::from(width)).max(1) as u32,
            )
        } else {
            (
                (u64::from(width) * 64 / u64::from(height)).max(1) as u32,
                64,
            )
        };
        imageops::resize(&document.image, new_width, new_height, FilterType::Lanczos3)
    };
    Ok(Thumbnail {
        image,
        original_size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;
    use std::time::{Duration, SystemTime};

    fn png(path: &Path, width: u32, height: u32) {
        RgbaImage::from_pixel(width, height, Rgba([30, 90, 150, 77]))
            .save(path)
            .unwrap();
    }

    fn set_modified(path: &Path, seconds: u64) {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(
                fs::FileTimes::new()
                    .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)),
            )
            .unwrap();
    }

    #[test]
    fn lists_only_direct_regular_pngs_newest_first_with_filename_ties() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["z.png", "b.PnG", "a.PNG"] {
            png(&dir.path().join(name), 2, 2);
        }
        fs::write(dir.path().join("ignore.txt"), b"not png").unwrap();
        fs::write(dir.path().join("ignore.png.bak"), b"not png").unwrap();
        fs::create_dir(dir.path().join("subdir.png")).unwrap();
        png(&dir.path().join("subdir.png/nested.png"), 2, 2);
        set_modified(&dir.path().join("z.png"), 1_700_000_100);
        set_modified(&dir.path().join("a.PNG"), 1_700_000_000);
        set_modified(&dir.path().join("b.PnG"), 1_700_000_000);
        let expected: Vec<_> = ["z.png", "a.PNG", "b.PnG"]
            .into_iter()
            .map(|name| dir.path().join(name))
            .collect();
        assert_eq!(list_pngs(dir.path()).unwrap(), expected);
        assert_eq!(list_pngs(dir.path()).unwrap(), expected);
    }

    #[test]
    fn missing_folder_is_empty_but_regular_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list_pngs(&dir.path().join("missing")).unwrap().is_empty());
        let path = dir.path().join("file");
        fs::write(&path, b"file").unwrap();
        assert!(list_pngs(&path).is_err());
    }

    #[test]
    fn thumbnail_dimensions_preserve_aspect_and_never_upscale() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("image.png");
        for (source, expected) in [
            ((224, 128), (112, 64)),
            ((400, 100), (112, 28)),
            ((100, 400), (16, 64)),
            ((20, 10), (20, 10)),
            ((112, 64), (112, 64)),
            ((1, 1000), (1, 64)),
            ((1000, 1), (112, 1)),
        ] {
            png(&path, source.0, source.1);
            let before = fs::read(&path).unwrap();
            let result = thumbnail(&path).unwrap();
            assert_eq!(result.original_size, source);
            assert_eq!(result.image.dimensions(), expected);
            assert_eq!(fs::read(&path).unwrap(), before);
        }
    }

    #[test]
    fn thumbnail_retains_alpha_with_and_without_resize() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("alpha.png");
        for (width, height) in [(8, 4), (224, 128)] {
            png(&path, width, height);
            let result = thumbnail(&path).unwrap();
            assert!(result.image.pixels().all(|pixel| pixel[3] == 77));
        }
        let mut image = RgbaImage::new(2, 1);
        image.put_pixel(0, 0, Rgba([10, 20, 30, 0]));
        image.put_pixel(1, 0, Rgba([40, 50, 60, 255]));
        image.save(&path).unwrap();
        assert_eq!(thumbnail(&path).unwrap().image, image);
    }

    #[test]
    fn thumbnail_rejects_missing_invalid_and_corrupt_pngs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.png");
        assert!(thumbnail(&path).is_err());
        fs::write(&path, b"not a PNG").unwrap();
        assert!(thumbnail(&path).is_err());
        png(&path, 2, 2);
        let mut bytes = fs::read(&path).unwrap();
        bytes[29] ^= 1; // Corrupt the IHDR CRC, which the engine validates.
        fs::write(&path, &bytes).unwrap();
        assert!(thumbnail(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}
