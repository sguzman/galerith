//! URL input-file loading and successful-entry mutation.
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputFileAction {
    ReadOnly,
    Comment,
    Delete,
}

#[derive(Debug, Clone)]
pub struct InputUrl {
    pub url: String,
    pub mutation: Option<(PathBuf, InputFileAction)>,
}

pub fn parse_urls(content: &str, path: Option<&Path>, action: InputFileAction) -> Vec<InputUrl> {
    content
        .lines()
        .filter_map(|line| {
            let url = line.trim();
            if url.is_empty() || url.starts_with('#') {
                return None;
            }
            Some(InputUrl {
                url: url.to_owned(),
                mutation: path
                    .filter(|_| action != InputFileAction::ReadOnly)
                    .map(|path| (path.to_owned(), action)),
            })
        })
        .collect()
}

/// Mark the first still-active matching line after its URL has succeeded.
/// Re-reading on each update preserves unrelated edits and handles duplicate
/// URLs one occurrence at a time.
pub fn mark_success(path: &Path, url: &str, action: InputFileAction) -> io::Result<()> {
    if action == InputFileAction::ReadOnly {
        return Ok(());
    }
    let path = fs::canonicalize(path)?;
    let original = fs::read(&path)?;
    let text = std::str::from_utf8(&original)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let mut updated = Vec::with_capacity(original.len());
    let mut matched = false;
    for line in text.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let body = body.strip_suffix('\r').unwrap_or(body);
        if !matched && body.trim() == url {
            matched = true;
            match action {
                InputFileAction::ReadOnly => updated.extend_from_slice(line.as_bytes()),
                InputFileAction::Comment => {
                    updated.push(b'#');
                    updated.extend_from_slice(line.as_bytes());
                }
                InputFileAction::Delete => {}
            }
        } else {
            updated.extend_from_slice(line.as_bytes());
        }
    }
    if !matched {
        return Ok(());
    }
    replace_file_atomically(&path, &updated)
}

fn replace_file_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let filename = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "input file has no name"))?;
    let permissions = fs::metadata(path)?.permissions();
    let (tmp, mut file) = loop {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut name = filename.to_os_string();
        name.push(format!(".galerith.{}.{}.tmp", std::process::id(), sequence));
        let candidate = parent.join(name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => break (candidate, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let result = (|| {
        file.write_all(bytes)?;
        file.set_permissions(permissions)?;
        file.sync_all()?;
        Ok(())
    })();
    drop(file);
    if let Err(error) = result {
        let _ = fs::remove_file(tmp);
        return Err(error);
    }
    if let Err(error) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(tmp);
        return Err(error);
    }
    if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("galerith-input-{name}-{}", std::process::id()))
    }

    #[test]
    fn parse_filters_blank_and_comment_lines_and_tracks_only_mutable_files() {
        let parsed = parse_urls(
            "\n # existing comment\n  https://example.org/a.jpg  \nhttps://example.org/b.jpg\n",
            Some(Path::new("input.txt")),
            InputFileAction::Comment,
        );
        assert_eq!(
            parsed
                .iter()
                .map(|item| item.url.as_str())
                .collect::<Vec<_>>(),
            ["https://example.org/a.jpg", "https://example.org/b.jpg"]
        );
        assert!(parsed.iter().all(|item| {
            item.mutation
                .as_ref()
                .is_some_and(|(_, action)| *action == InputFileAction::Comment)
        }));
        assert!(
            parse_urls("https://example.org/a.jpg", None, InputFileAction::ReadOnly)[0]
                .mutation
                .is_none()
        );
    }

    #[test]
    fn success_comments_only_the_first_matching_line_and_preserves_line_endings() {
        let path = temp_file("comment");
        fs::write(
            &path,
            b"https://example.org/a.jpg\r\nkeep\nhttps://example.org/a.jpg\n",
        )
        .unwrap();
        mark_success(&path, "https://example.org/a.jpg", InputFileAction::Comment).unwrap();
        assert_eq!(
            fs::read(&path).unwrap(),
            b"#https://example.org/a.jpg\r\nkeep\nhttps://example.org/a.jpg\n"
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn success_deletes_only_the_first_matching_line() {
        let path = temp_file("delete");
        fs::write(
            &path,
            b"https://example.org/a.jpg\nkeep\nhttps://example.org/a.jpg\n",
        )
        .unwrap();
        mark_success(&path, "https://example.org/a.jpg", InputFileAction::Delete).unwrap();
        assert_eq!(
            fs::read(&path).unwrap(),
            b"keep\nhttps://example.org/a.jpg\n"
        );
        fs::remove_file(path).unwrap();
    }
}
