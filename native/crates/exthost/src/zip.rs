//! Reads a zip archive as base/node/zip.ts does through yauzl: stored and deflated entries, CRC checked, and
//! `extract` with its `sourcePath` filter and the check that keeps entries inside the target folder.

use std::io::Read;
use std::path::{Component, Path};

use anyhow::{Context, bail};

pub struct Entry {
    pub name: String,
    /// yauzl's `modeFromEntry`: the Unix permission bits, 0o644 when the archive stores none.
    pub mode: u32,
    method: u16,
    crc: u32,
    compressed_size: usize,
    size: usize,
    header_offset: usize,
}

pub struct Archive {
    bytes: Vec<u8>,
    pub entries: Vec<Entry>,
}

const END_OF_CENTRAL_DIRECTORY: u32 = 0x0605_4b50;
const CENTRAL_DIRECTORY_HEADER: u32 = 0x0201_4b50;
const LOCAL_FILE_HEADER: u32 = 0x0403_4b50;

impl Archive {
    pub fn open(bytes: Vec<u8>) -> anyhow::Result<Archive> {
        // The end record is 22 bytes plus a comment of at most 65535.
        let search_from = bytes.len().saturating_sub(22 + 0xFFFF);
        let end = (search_from..bytes.len().saturating_sub(21)).rev().find(|&at| u32_at(&bytes, at) == END_OF_CENTRAL_DIRECTORY).context("end of central directory record signature not found")?;
        let count = u16_at(&bytes, end + 10) as usize;
        let mut at = u32_at(&bytes, end + 16) as usize;
        if count == 0xFFFF || at == 0xFFFF_FFFF {
            // shortcut: zip64 archives (over 65535 entries or 4 GB) are refused; no VSIX on Open VSX is one.
            bail!("zip64 archives are not supported");
        }
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            if at + 46 > bytes.len() || u32_at(&bytes, at) != CENTRAL_DIRECTORY_HEADER {
                bail!("invalid central directory file header signature");
            }
            let flags = u16_at(&bytes, at + 8);
            if flags & 1 != 0 {
                bail!("encrypted entries are not supported");
            }
            let name_length = u16_at(&bytes, at + 28) as usize;
            let extra_length = u16_at(&bytes, at + 30) as usize;
            let comment_length = u16_at(&bytes, at + 32) as usize;
            let external_attributes = u32_at(&bytes, at + 38);
            let name = bytes.get(at + 46..at + 46 + name_length).context("entry name past the end of the archive")?;
            let unix_mode = match external_attributes >> 16 {
                0 => 0o100644,
                mode => mode,
            };
            entries.push(Entry {
                name: String::from_utf8_lossy(name).into_owned(),
                mode: unix_mode & 0o777,
                method: u16_at(&bytes, at + 10),
                crc: u32_at(&bytes, at + 16),
                compressed_size: u32_at(&bytes, at + 20) as usize,
                size: u32_at(&bytes, at + 24) as usize,
                header_offset: u32_at(&bytes, at + 42) as usize,
            });
            at += 46 + name_length + extra_length + comment_length;
        }
        Ok(Archive { bytes, entries })
    }

    pub fn entry(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.name == name)
    }

    pub fn read(&self, entry: &Entry) -> anyhow::Result<Vec<u8>> {
        let at = entry.header_offset;
        if at + 30 > self.bytes.len() || u32_at(&self.bytes, at) != LOCAL_FILE_HEADER {
            bail!("invalid local file header signature: {}", entry.name);
        }
        let start = at + 30 + u16_at(&self.bytes, at + 26) as usize + u16_at(&self.bytes, at + 28) as usize;
        let data = self.bytes.get(start..start + entry.compressed_size).with_context(|| format!("{} runs past the end of the archive", entry.name))?;
        let contents = match entry.method {
            0 => data.to_vec(),
            8 => {
                let mut contents = Vec::with_capacity(entry.size);
                flate2::read::DeflateDecoder::new(data).read_to_end(&mut contents).with_context(|| format!("inflating {}", entry.name))?;
                contents
            }
            method => bail!("unsupported compression method {method}: {}", entry.name),
        };
        let mut crc = flate2::Crc::new();
        crc.update(&contents);
        if contents.len() != entry.size || crc.sum() != entry.crc {
            bail!("invalid CRC or size: {}", entry.name);
        }
        Ok(contents)
    }

    /// zip.ts `extract`: the entries under `source_path/` written into `target`, without that prefix.
    pub fn extract(&self, source_path: &str, target: &Path) -> anyhow::Result<()> {
        let prefix = format!("{source_path}/");
        for entry in &self.entries {
            // `zip -r` writes the source folder itself as an entry, which names the target.
            let Some(file_name) = entry.name.strip_prefix(&prefix).filter(|name| !name.is_empty()) else { continue };
            if Path::new(file_name).components().any(|component| !matches!(component, Component::Normal(_) | Component::CurDir)) {
                bail!("Error extracting {file_name}. Invalid file.");
            }
            let path = target.join(file_name);
            if file_name.ends_with('/') {
                std::fs::create_dir_all(&path)?;
                continue;
            }
            std::fs::create_dir_all(path.parent().expect("a joined file name has a parent"))?;
            std::fs::write(&path, self.read(entry)?)?;
            std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(entry.mode))?;
        }
        Ok(())
    }
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Writes a stored zip, for tests of the readers of VSIX files.
#[cfg(test)]
pub fn write_stored(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, contents) in files {
        let mut crc = flate2::Crc::new();
        crc.update(contents);
        let offset = out.len() as u32;
        let fields = |out: &mut Vec<u8>| {
            out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            out.extend_from_slice(&crc.sum().to_le_bytes());
            out.extend_from_slice(&(contents.len() as u32).to_le_bytes());
            out.extend_from_slice(&(contents.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
        };
        out.extend_from_slice(&LOCAL_FILE_HEADER.to_le_bytes());
        fields(&mut out);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(contents);
        central.extend_from_slice(&CENTRAL_DIRECTORY_HEADER.to_le_bytes());
        central.extend_from_slice(&[20, 3]);
        fields(&mut central);
        central.extend_from_slice(&[0; 6]);
        central.extend_from_slice(&(0o100755u32 << 16).to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let central_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&END_OF_CENTRAL_DIRECTORY.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_extracts_the_source_path() {
        let bytes = write_stored(&[("extension.vsixmanifest", b"<x/>"), ("extension/", b""), ("extension/package.json", b"{}"), ("extension/bin/", b""), ("extension/bin/tool", b"run")]);
        let archive = Archive::open(bytes).unwrap();
        assert_eq!(archive.read(archive.entry("extension/package.json").unwrap()).unwrap(), b"{}");
        let dir = std::env::temp_dir().join(format!("zip-extract-{}", std::process::id()));
        archive.extract("extension", &dir).unwrap();
        let mode = std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(dir.join("bin/tool")).unwrap().permissions());
        let listed = (dir.join("package.json").is_file(), dir.join("extension.vsixmanifest").exists());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(listed, (true, false));
        assert_eq!(mode & 0o777, 0o755);
    }

    #[test]
    fn refuses_entries_outside_the_target() {
        let archive = Archive::open(write_stored(&[("extension/../../escape", b"x")])).unwrap();
        let error = archive.extract("extension", &std::env::temp_dir().join("zip-escape")).unwrap_err();
        assert_eq!(error.to_string(), "Error extracting ../../escape. Invalid file.");
    }

    #[test]
    fn detects_corruption() {
        let mut bytes = write_stored(&[("extension/a", b"abc")]);
        let at = bytes.windows(3).position(|window| window == b"abc").unwrap();
        bytes[at] = b'x';
        let archive = Archive::open(bytes).unwrap();
        assert!(archive.read(&archive.entries[0]).is_err());
    }
}
