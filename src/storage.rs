use anyhow::{Context, Result, bail};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn safe_name(name: &str) -> String {
    let value: String = name
        .chars()
        .take(64)
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let value = value.trim_end_matches([' ', '.']);
    if value.is_empty() {
        "author".to_owned()
    } else {
        value.to_owned()
    }
}

/// Reserve the destination with create_new so concurrent saves cannot overwrite a file.
pub fn save_unique(directory: &Path, filename: &str, bytes: &[u8]) -> Result<PathBuf> {
    if Path::new(filename).file_name().and_then(|s| s.to_str()) != Some(filename) {
        bail!("저장할 파일 이름이 올바르지 않습니다.");
    }
    fs::create_dir_all(directory).context("저장 폴더를 만들 수 없습니다.")?;
    let stem = Path::new(filename)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let ext = Path::new(filename)
        .extension()
        .unwrap_or_default()
        .to_string_lossy();
    for index in 0..100_000 {
        let name = if index == 0 {
            filename.to_owned()
        } else if ext.is_empty() {
            format!("{stem}_{index}")
        } else {
            format!("{stem}_{index}.{ext}")
        };
        let path = directory.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
                    drop(file);
                    let _ = fs::remove_file(&path);
                    return Err(error).context("파일을 저장하지 못했습니다.");
                }
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("파일을 저장하지 못했습니다."),
        }
    }
    bail!("같은 이름의 파일이 너무 많습니다. 다른 폴더를 선택해 주시기 바랍니다.");
}

/// Replace an existing regular file only after its sibling temporary file is synced.
pub fn save_existing(path: &Path, bytes: &[u8]) -> Result<PathBuf> {
    let metadata = fs::symlink_metadata(path).context("기존 파일을 확인할 수 없습니다.")?;
    if !metadata.is_file() {
        bail!("덮어쓸 대상이 일반 파일이 아닙니다.");
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).context("임시 파일을 만들 수 없습니다.")?;
    temporary
        .write_all(bytes)
        .context("파일을 저장하지 못했습니다.")?;
    temporary.flush().context("파일을 저장하지 못했습니다.")?;
    temporary
        .as_file()
        .sync_all()
        .context("파일을 저장하지 못했습니다.")?;
    // Check again before replacement in case the target disappeared while writing.
    if !fs::symlink_metadata(path)
        .context("기존 파일을 확인할 수 없습니다.")?
        .is_file()
    {
        bail!("덮어쓸 대상이 일반 파일이 아닙니다.");
    }
    temporary.persist(path).map_err(|error| {
        // Dropping PersistError's temporary file removes it on replacement failure.
        anyhow::Error::new(error.error).context("기존 파일을 교체하지 못했습니다.")
    })?;
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_existing_file_at_same_path() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("edited.png");
        fs::write(&path, b"original longer contents").unwrap();

        assert_eq!(save_existing(&path, b"edited").unwrap(), path);
        assert_eq!(fs::read(&path).unwrap(), b"edited");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn rejects_missing_target_without_creating_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("missing.png");

        assert!(save_existing(&path, b"edited").is_err());
        assert!(!path.exists());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn rejects_directory_without_changing_contents() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("directory.png");
        fs::create_dir(&path).unwrap();
        let original = path.join("original");
        fs::write(&original, b"original").unwrap();

        assert!(save_existing(&path, b"edited").is_err());
        assert!(path.is_dir());
        assert_eq!(fs::read(original).unwrap(), b"original");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn failed_replacement_preserves_original_and_cleans_temporary_file() {
        use std::os::windows::fs::OpenOptionsExt;

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("locked.png");
        fs::write(&path, b"original").unwrap();
        let locked = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();

        assert!(save_existing(&path, b"edited").is_err());
        drop(locked);
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn saves_without_overwriting_and_rejects_traversal() {
        let root = std::env::temp_dir().join(format!(
            "mabipng-save-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let first = save_unique(&root, "same.png", b"first").unwrap();
        let second = save_unique(&root, "same.png", b"second").unwrap();
        assert_ne!(first, second);
        assert_eq!(fs::read(&first).unwrap(), b"first");
        assert_eq!(fs::read(&second).unwrap(), b"second");
        assert!(save_unique(&root, "../escape.png", b"bad").is_err());
        fs::remove_file(first).unwrap();
        fs::remove_file(second).unwrap();
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn sanitizes_only_filename_not_author_metadata() {
        assert_eq!(safe_name("이름/테스트:*"), "이름_테스트__");
        assert_eq!(safe_name("..."), "author");
    }
}
