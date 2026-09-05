use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, bail};
use flate2::read::GzDecoder;
use serde::Serialize;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_ARCHIVE_COMPRESSED_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ARCHIVE_EXPANDED_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 10_000;
const MAX_LISTED_BYTES: u64 = 24 * 1024 * 1024;
pub const MAX_PREVIEW_BYTES: usize = 256 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileList {
    pub snapshot_at: Option<String>,
    pub files: Vec<ArchiveFile>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveFile {
    pub path: String,
    pub byte_length: u64,
    pub kind: FileKind,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    Text,
    Binary,
}

pub struct ArchiveReader {
    path: PathBuf,
    modified: SystemTime,
}

pub struct ReadFile {
    pub bytes: Vec<u8>,
    pub total_length: u64,
    pub kind: FileKind,
    pub truncated: bool,
}

impl ArchiveReader {
    pub fn open(path: PathBuf) -> anyhow::Result<Self> {
        let metadata =
            std::fs::symlink_metadata(&path).context("stopped-home snapshot is missing")?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            bail!("stopped-home snapshot is missing");
        }
        if metadata.len() > MAX_ARCHIVE_COMPRESSED_BYTES {
            bail!("stopped-home snapshot exceeds the compressed size limit");
        }
        Ok(Self {
            path,
            modified: metadata.modified()?,
        })
    }

    pub fn list(&self) -> anyhow::Result<FileList> {
        let mut files = Vec::new();
        let mut listed_bytes = 0_u64;
        let mut archive = self.archive()?;
        let mut entry_count = 0_usize;
        for entry in archive.entries()? {
            let mut entry = entry?;
            entry_count += 1;
            if entry_count > MAX_ARCHIVE_ENTRIES {
                bail!("stopped-home snapshot exceeds the entry limit");
            }
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let Some(path) = safe_path(&entry.path()?) else {
                continue;
            };
            let size = entry.header().size()?;
            if size > MAX_FILE_BYTES {
                continue;
            }
            listed_bytes = listed_bytes
                .checked_add(size)
                .context("archive content size overflow")?;
            if listed_bytes > MAX_LISTED_BYTES {
                bail!("stopped-home snapshot exceeds the listing limit");
            }
            let kind = sniff_kind(&mut entry, size)?;
            files.push(ArchiveFile {
                path: path_string(&path),
                byte_length: size,
                kind,
            });
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(FileList {
            snapshot_at: OffsetDateTime::from(self.modified).format(&Rfc3339).ok(),
            files,
        })
    }

    pub fn read(&self, requested: &str, download: bool) -> anyhow::Result<ReadFile> {
        let requested = safe_path(Path::new(requested)).context("invalid archive path")?;
        let mut archive = self.archive()?;
        let mut entry_count = 0_usize;
        let mut found = None;
        for entry in archive.entries()? {
            let mut entry = entry?;
            entry_count += 1;
            if entry_count > MAX_ARCHIVE_ENTRIES {
                bail!("stopped-home snapshot exceeds the entry limit");
            }
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let Some(path) = safe_path(&entry.path()?) else {
                continue;
            };
            if path != requested {
                continue;
            }
            if found.is_some() {
                bail!("archive contains duplicate file paths");
            }
            let size = entry.header().size()?;
            if size > MAX_FILE_BYTES {
                bail!("archive file exceeds the size limit");
            }
            let mut bytes = Vec::with_capacity(size as usize);
            entry.read_to_end(&mut bytes)?;
            let kind = if std::str::from_utf8(&bytes).is_ok() {
                FileKind::Text
            } else {
                FileKind::Binary
            };
            if !download && kind == FileKind::Binary {
                bail!("binary files require download=1");
            }
            let truncated = !download && bytes.len() > MAX_PREVIEW_BYTES;
            if truncated {
                let text = std::str::from_utf8(&bytes).expect("text kind is valid UTF-8");
                let mut boundary = MAX_PREVIEW_BYTES;
                while !text.is_char_boundary(boundary) {
                    boundary -= 1;
                }
                bytes.truncate(boundary);
            }
            found = Some(ReadFile {
                truncated,
                bytes,
                total_length: size,
                kind,
            });
        }
        found.context("archive file not found")
    }

    fn archive(&self) -> anyhow::Result<tar::Archive<ExpandedLimit<GzDecoder<std::fs::File>>>> {
        let decoder = GzDecoder::new(std::fs::File::open(&self.path)?);
        Ok(tar::Archive::new(ExpandedLimit::new(
            decoder,
            MAX_ARCHIVE_EXPANDED_BYTES,
        )))
    }
}

struct ExpandedLimit<R> {
    inner: R,
    remaining: u64,
}

impl<R> ExpandedLimit<R> {
    fn new(inner: R, limit: u64) -> Self {
        Self {
            inner,
            remaining: limit,
        }
    }
}

impl<R: Read> Read for ExpandedLimit<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            let mut extra = [0_u8; 1];
            return match self.inner.read(&mut extra)? {
                0 => Ok(0),
                _ => Err(io::Error::other("archive exceeds the expanded size limit")),
            };
        }
        let allowed = buffer.len().min(self.remaining as usize);
        let read = self.inner.read(&mut buffer[..allowed])?;
        self.remaining -= read as u64;
        Ok(read)
    }
}

pub fn is_unsafe_browser_path(path: &str) -> bool {
    matches!(
        Path::new(path)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("html" | "htm" | "xhtml" | "svg" | "xml" | "js" | "mjs" | "css" | "pdf")
    )
}

fn safe_path(path: &Path) -> Option<PathBuf> {
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => continue,
            Component::Normal(value) => {
                let value = value.to_str()?;
                if value.is_empty() || value.starts_with('.') {
                    return None;
                }
                clean.push(value);
            }
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    let first = clean.components().next()?.as_os_str().to_str()?;
    if first == "society" {
        return None;
    }
    Some(clean)
}

fn sniff_kind(reader: &mut impl Read, size: u64) -> anyhow::Result<FileKind> {
    let mut content = Vec::with_capacity(size as usize);
    reader
        .take(size)
        .read_to_end(&mut content)
        .context("failed to inspect archive entry")?;
    Ok(if std::str::from_utf8(&content).is_ok() {
        FileKind::Text
    } else {
        FileKind::Binary
    })
}

fn path_string(path: &Path) -> String {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::safe_path;
    use std::path::{Path, PathBuf};

    #[test]
    fn path_normalization_rejects_traversal_and_internal_paths() {
        assert_eq!(
            safe_path(Path::new("./work/notes.txt")),
            Some(PathBuf::from("work/notes.txt"))
        );
        assert_eq!(safe_path(Path::new("../secret")), None);
        assert_eq!(safe_path(Path::new("/etc/passwd")), None);
        assert_eq!(safe_path(Path::new("work/.git/config")), None);
        assert_eq!(safe_path(Path::new("society/agent.key")), None);
    }
}
