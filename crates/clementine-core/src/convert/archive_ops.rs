use std::fs::{self, File};
use std::io::{Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;

use super::{ClementineError, Result};

fn ae(e: impl std::fmt::Display) -> ClementineError {
    ClementineError::ArchiveError(e.to_string())
}

struct Entry {
    path: String,
    data: Vec<u8>,
    is_dir: bool,
}

/// Rejects absolute paths and `..` so an archive cannot write outside `out`.
fn safe_rel(name: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    let mut rel = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => rel.push(s),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!rel.as_os_str().is_empty()).then_some(rel)
}

// ponytail: whole archive is held in memory; stream if multi-GB archives matter.
fn read_entries(path: &Path, ext: &str) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    match ext {
        "zip" => {
            let mut z = zip::ZipArchive::new(File::open(path)?).map_err(ae)?;
            for i in 0..z.len() {
                let mut f = z.by_index(i).map_err(ae)?;
                let mut data = Vec::new();
                if !f.is_dir() {
                    f.read_to_end(&mut data)?;
                }
                entries.push(Entry { path: f.name().to_string(), data, is_dir: f.is_dir() });
            }
        }
        "tar" | "gz" => {
            let file = File::open(path)?;
            let reader: Box<dyn Read> =
                if ext == "gz" { Box::new(GzDecoder::new(file)) } else { Box::new(file) };
            let mut ar = tar::Archive::new(reader);
            for e in ar.entries().map_err(ae)? {
                let mut e = e.map_err(ae)?;
                let t = e.header().entry_type();
                if !(t.is_file() || t.is_dir()) {
                    continue;
                }
                let p = e.path().map_err(ae)?.to_string_lossy().into_owned();
                let mut data = Vec::new();
                if t.is_file() {
                    e.read_to_end(&mut data)?;
                }
                entries.push(Entry { path: p, data, is_dir: t.is_dir() });
            }
        }
        other => return Err(ae(format!("cannot read .{other}"))),
    }
    Ok(entries)
}

fn write_tar<W: Write>(w: W, entries: &[Entry]) -> Result<W> {
    let mut b = tar::Builder::new(w);
    for e in entries {
        let mut h = tar::Header::new_gnu();
        h.set_mode(if e.is_dir { 0o755 } else { 0o644 });
        h.set_mtime(0);
        if e.is_dir {
            h.set_entry_type(tar::EntryType::Directory);
            h.set_size(0);
            b.append_data(&mut h, e.path.trim_end_matches('/'), std::io::empty()).map_err(ae)?;
        } else {
            h.set_size(e.data.len() as u64);
            b.append_data(&mut h, &e.path, e.data.as_slice()).map_err(ae)?;
        }
    }
    b.into_inner().map_err(ae)
}

fn write_entries(entries: &[Entry], target: &str, out: &Path) -> Result<()> {
    match target {
        "zip" => {
            let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for e in entries {
                if e.is_dir {
                    z.add_directory(e.path.clone(), opts).map_err(ae)?;
                } else {
                    z.start_file(e.path.clone(), opts).map_err(ae)?;
                    z.write_all(&e.data)?;
                }
            }
            fs::write(out, z.finish().map_err(ae)?.into_inner())?;
        }
        "tar" => {
            write_tar(File::create(out)?, entries)?;
        }
        "gz" => {
            let gz = write_tar(GzEncoder::new(File::create(out)?, Compression::default()), entries)?;
            gz.finish()?;
        }
        other => {
            return Err(ClementineError::UnsupportedConversion {
                from: "archive".into(),
                to: other.into(),
            })
        }
    }
    Ok(())
}

pub fn convert(path: &Path, src: &str, target: &str, out: &Path) -> Result<()> {
    write_entries(&read_entries(path, src)?, target, out)
}

/// Unpacks into the already-created folder `out`.
pub fn extract(path: &Path, ext: &str, out: &Path) -> Result<()> {
    for e in read_entries(path, ext)? {
        let rel = safe_rel(&e.path)
            .ok_or_else(|| ae(format!("unsafe path in archive: {}", e.path)))?;
        let dest = out.join(rel);
        if e.is_dir {
            fs::create_dir_all(&dest)?;
        } else {
            if let Some(p) = dest.parent() {
                fs::create_dir_all(p)?;
            }
            fs::write(dest, e.data)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::test_util::names;
    use super::super::{execute_conversion, execute_tool, ToolParams};
    use super::*;

    fn make_zip(dir: &Path) -> PathBuf {
        let p = dir.join("bundle.zip");
        let mut z = zip::ZipWriter::new(File::create(&p).unwrap());
        let o = zip::write::SimpleFileOptions::default();
        z.start_file("a.txt", o).unwrap();
        z.write_all(b"alpha").unwrap();
        z.add_directory("sub/", o).unwrap();
        z.start_file("sub/b.txt", o).unwrap();
        z.write_all(b"beta").unwrap();
        z.finish().unwrap();
        p
    }

    #[test]
    fn zip_tar_gz_roundtrip_and_extract() {
        let d = tempfile::tempdir().unwrap();
        let zp = make_zip(d.path());
        let tar = execute_conversion(&zp, "tar").unwrap();
        assert_eq!(tar.file_name().unwrap().to_string_lossy(), "bundle.tar");
        let tgz = execute_conversion(&tar, "gz").unwrap();
        assert_eq!(tgz.file_name().unwrap().to_string_lossy(), "bundle.tar.gz");
        assert_eq!(&fs::read(&tgz).unwrap()[..2], &[0x1F, 0x8B]);

        let zip2 = execute_conversion(&tgz, "zip").unwrap();
        assert_eq!(zip2.file_name().unwrap().to_string_lossy(), "bundle 2.zip");

        let out = execute_tool(&[zip2], "extractArchive", &ToolParams::default()).unwrap();
        assert_eq!(fs::read(out.join("a.txt")).unwrap(), b"alpha");
        assert_eq!(fs::read(out.join("sub/b.txt")).unwrap(), b"beta");

        let out2 = execute_tool(&[tgz], "extractArchive", &ToolParams::default()).unwrap();
        assert_eq!(fs::read(out2.join("sub/b.txt")).unwrap(), b"beta");
        let out3 = execute_tool(&[tar], "extractArchive", &ToolParams::default()).unwrap();
        assert_eq!(fs::read(out3.join("a.txt")).unwrap(), b"alpha");
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(safe_rel("../evil").is_none());
        assert!(safe_rel("/abs").is_none());
        assert_eq!(safe_rel("./a/b").unwrap(), PathBuf::from("a/b"));
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("evil.zip");
        let mut z = zip::ZipWriter::new(File::create(&p).unwrap());
        z.start_file("../evil.txt", zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(b"x").unwrap();
        z.finish().unwrap();
        assert!(execute_tool(&[p], "extractArchive", &ToolParams::default()).is_err());
        assert_eq!(names(d.path()), vec!["evil.zip"]);
    }
}
