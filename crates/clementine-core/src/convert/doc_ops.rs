use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;

use pulldown_cmark::{html, Event, Parser, Tag, TagEnd};

use super::{ClementineError, Result};

// ---------------------------------------------------------------- PDF 1.4

pub struct PdfPage {
    pub w: f32,
    pub h: f32,
    /// Page content stream.
    pub content: Vec<u8>,
    /// Optional JPEG painted as `/Im0`: (bytes, pixel width, pixel height).
    pub jpeg: Option<(Vec<u8>, u32, u32)>,
}

/// Minimal PDF 1.4 writer: catalog, page tree, one Helvetica font, one page
/// object per `PdfPage`.
pub fn build_pdf(pages: &[PdfPage]) -> Vec<u8> {
    let mut out: Vec<u8> = b"%PDF-1.4\n".to_vec();
    let mut offsets: Vec<usize> = Vec::new();
    let mut obj = |out: &mut Vec<u8>, body: &[u8]| {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    // Object ids: 1 catalog, 2 page tree, 3 font, then per page 2-3 objects.
    let mut next_id = 4;
    let mut kids = String::new();
    let mut ids = Vec::new();
    for p in pages {
        let page_id = next_id;
        let content_id = next_id + 1;
        let image_id = p.jpeg.as_ref().map(|_| next_id + 2);
        next_id += if image_id.is_some() { 3 } else { 2 };
        kids.push_str(&format!("{page_id} 0 R "));
        ids.push((page_id, content_id, image_id));
    }
    obj(&mut out, b"<< /Type /Catalog /Pages 2 0 R >>");
    obj(
        &mut out,
        format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", pages.len()).as_bytes(),
    );
    obj(&mut out, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>");
    for (p, (_, content_id, image_id)) in pages.iter().zip(&ids) {
        let xobj = image_id.map(|i| format!(" /XObject << /Im0 {i} 0 R >>")).unwrap_or_default();
        obj(
            &mut out,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Contents {content_id} 0 R /Resources << /Font << /F1 3 0 R >>{xobj} >> >>",
                p.w, p.h
            )
            .as_bytes(),
        );
        let mut c = format!("<< /Length {} >>\nstream\n", p.content.len()).into_bytes();
        c.extend_from_slice(&p.content);
        c.extend_from_slice(b"\nendstream");
        obj(&mut out, &c);
        if let Some((jpeg, w, h)) = &p.jpeg {
            let mut c = format!(
                "<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
                jpeg.len()
            )
            .into_bytes();
            c.extend_from_slice(jpeg);
            c.extend_from_slice(b"\nendstream");
            obj(&mut out, &c);
        }
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for o in &offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    out
}

fn pdf_escape(s: &str) -> Vec<u8> {
    let mut v = Vec::new();
    for c in s.chars() {
        match c {
            '(' | ')' | '\\' => {
                v.push(b'\\');
                v.push(c as u8);
            }
            c if (c as u32) < 256 && (c as u32) >= 32 => v.push(c as u32 as u8),
            '\t' => v.extend_from_slice(b"    "),
            _ => v.push(b'?'),
        }
    }
    v
}

/// Text to an A4 multi-page PDF, wrapping at 90 columns.
fn text_pdf(text: &str) -> Vec<u8> {
    const COLS: usize = 90;
    const LINES: usize = 50;
    let mut wrapped: Vec<String> = Vec::new();
    for line in text.lines() {
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            wrapped.push(String::new());
        }
        for chunk in chars.chunks(COLS) {
            wrapped.push(chunk.iter().collect());
        }
    }
    if wrapped.is_empty() {
        wrapped.push(String::new());
    }
    let pages: Vec<PdfPage> = wrapped
        .chunks(LINES)
        .map(|lines| {
            let mut c = b"BT /F1 11 Tf 14 TL 50 792 Td\n".to_vec();
            for l in lines {
                c.push(b'(');
                c.extend(pdf_escape(l));
                c.extend_from_slice(b") Tj T*\n");
            }
            c.extend_from_slice(b"ET");
            PdfPage { w: 595.0, h: 842.0, content: c, jpeg: None }
        })
        .collect();
    build_pdf(&pages)
}

// ------------------------------------------------------------------ DOCX

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn docx(text: &str) -> Result<Vec<u8>> {
    let mut body = String::new();
    for l in text.lines() {
        body.push_str(&format!(
            "<w:p><w:r><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>",
            xml_escape(l)
        ));
    }
    let doc = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>{body}</w:body></w:document>"
    );
    let types = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/></Types>";
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/></Relationships>";
    let ze = |e: zip::result::ZipError| ClementineError::ArchiveError(e.to_string());
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in [
        ("[Content_Types].xml", types),
        ("_rels/.rels", rels),
        ("word/document.xml", doc.as_str()),
    ] {
        z.start_file(name, o).map_err(ze)?;
        z.write_all(data.as_bytes())?;
    }
    Ok(z.finish().map_err(ze)?.into_inner())
}

// ------------------------------------------------------------- Markdown

fn md_to_html(md: &str) -> String {
    let mut s = String::new();
    html::push_html(&mut s, Parser::new(md));
    format!("<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"></head><body>\n{s}</body></html>\n")
}

fn md_to_text(md: &str) -> String {
    let mut s = String::new();
    for ev in Parser::new(md) {
        match ev {
            Event::Text(t) | Event::Code(t) => s.push_str(&t),
            Event::SoftBreak | Event::HardBreak => s.push('\n'),
            Event::Start(Tag::Item) => s.push_str("- "),
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_)) => s.push_str("\n\n"),
            Event::End(TagEnd::Item | TagEnd::CodeBlock) => s.push('\n'),
            _ => {}
        }
    }
    s.trim_end().to_string() + "\n"
}

fn txt_to_html(txt: &str) -> String {
    let body: String = txt
        .replace("\r\n", "\n")
        .split("\n\n")
        .filter(|p| !p.trim().is_empty())
        .map(|p| format!("<p>{}</p>\n", xml_escape(p.trim()).replace('\n', "<br>\n")))
        .collect();
    format!("<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"></head><body>\n{body}</body></html>\n")
}

// ------------------------------------------------------------ Subtitles

struct Cue {
    timing: String,
    text: Vec<String>,
}

fn parse_cues(src: &str) -> Vec<Cue> {
    let norm = src.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let mut cues = Vec::new();
    for block in norm.split("\n\n") {
        let lines: Vec<&str> = block.lines().collect();
        let Some(i) = lines.iter().position(|l| l.contains("-->")) else { continue };
        let mut it = lines[i].split_whitespace();
        let (Some(a), Some(_), Some(b)) = (it.next(), it.next(), it.next()) else { continue };
        cues.push(Cue {
            timing: format!("{a} --> {b}"),
            text: lines[i + 1..].iter().map(|s| s.to_string()).collect(),
        });
    }
    cues
}

/// Normalizes `hh:mm:ss,mmm` / `mm:ss.mmm` to the target separator.
fn fix_time(t: &str, sep: char) -> String {
    let t = t.replace([',', '.'], ".");
    let (hms, ms) = t.split_once('.').unwrap_or((&t, "000"));
    let mut parts: Vec<&str> = hms.split(':').collect();
    while parts.len() < 3 {
        parts.insert(0, "00");
    }
    format!("{}{sep}{ms}", parts.join(":"))
}

fn fix_timing(timing: &str, sep: char) -> String {
    let (a, b) = timing.split_once(" --> ").unwrap_or((timing, timing));
    format!("{} --> {}", fix_time(a, sep), fix_time(b, sep))
}

fn subtitles(src: &str, from: &str, to: &str) -> Result<String> {
    let cues = parse_cues(src);
    let mut out = String::new();
    match to {
        "vtt" => {
            out.push_str("WEBVTT\n\n");
            for c in &cues {
                out.push_str(&format!("{}\n{}\n\n", fix_timing(&c.timing, '.'), c.text.join("\n")));
            }
        }
        "srt" => {
            for (i, c) in cues.iter().enumerate() {
                out.push_str(&format!("{}\n{}\n{}\n\n", i + 1, fix_timing(&c.timing, ','), c.text.join("\n")));
            }
        }
        "txt" => {
            for c in &cues {
                out.push_str(&c.text.join("\n"));
                out.push('\n');
            }
        }
        _ => {
            return Err(ClementineError::UnsupportedConversion { from: from.into(), to: to.into() })
        }
    }
    Ok(out)
}

// ----------------------------------------------------------- Spreadsheets

fn read_rows(text: &str, delim: u8) -> Result<Vec<Vec<String>>> {
    csv::ReaderBuilder::new()
        .delimiter(delim)
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes())
        .records()
        .map(|r| {
            r.map(|r| r.iter().map(String::from).collect())
                .map_err(|e| ClementineError::InvalidInput(e.to_string()))
        })
        .collect()
}

fn write_rows(rows: &[Vec<String>], delim: u8) -> Result<String> {
    let mut w = csv::WriterBuilder::new().delimiter(delim).flexible(true).from_writer(Vec::new());
    for r in rows {
        w.write_record(r).map_err(|e| ClementineError::InvalidInput(e.to_string()))?;
    }
    let bytes = w.into_inner().map_err(|e| ClementineError::InvalidInput(e.to_string()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn rows_html(rows: &[Vec<String>]) -> String {
    let mut s = String::from("<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"></head><body>\n<table>\n");
    for (i, r) in rows.iter().enumerate() {
        let tag = if i == 0 { "th" } else { "td" };
        s.push_str("<tr>");
        for c in r {
            s.push_str(&format!("<{tag}>{}</{tag}>", xml_escape(c)));
        }
        s.push_str("</tr>\n");
    }
    s.push_str("</table>\n</body></html>\n");
    s
}

fn rows_md(rows: &[Vec<String>]) -> String {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
    let line = |r: &Vec<String>| {
        let cells: Vec<String> =
            (0..cols).map(|i| r.get(i).map(|c| c.replace('|', "\\|")).unwrap_or_default()).collect();
        format!("| {} |\n", cells.join(" | "))
    };
    let mut s = String::new();
    for (i, r) in rows.iter().enumerate() {
        s.push_str(&line(r));
        if i == 0 {
            s.push_str(&format!("|{}\n", " --- |".repeat(cols)));
        }
    }
    s
}

fn sheet(text: &str, src: &str, target: &str) -> Result<Vec<u8>> {
    let rows = read_rows(text, if src == "tsv" { b'\t' } else { b',' })?;
    Ok(match target {
        "csv" => write_rows(&rows, b',')?.into_bytes(),
        "tsv" => write_rows(&rows, b'\t')?.into_bytes(),
        "html" => rows_html(&rows).into_bytes(),
        "md" => rows_md(&rows).into_bytes(),
        "pdf" => {
            let t: String = rows.iter().map(|r| r.join(" | ") + "\n").collect();
            text_pdf(&t)
        }
        _ => {
            return Err(ClementineError::UnsupportedConversion { from: src.into(), to: target.into() })
        }
    })
}

// ----------------------------------------------------------------- entry

pub fn convert(path: &Path, src: &str, target: &str, out: &Path) -> Result<()> {
    let text = String::from_utf8_lossy(&fs::read(path)?).into_owned();
    let unsupported = || ClementineError::UnsupportedConversion { from: src.into(), to: target.into() };
    let bytes: Vec<u8> = match src {
        "srt" | "vtt" => subtitles(&text, src, target)?.into_bytes(),
        "csv" | "tsv" => sheet(&text, src, target)?,
        "md" | "txt" => {
            let plain = || if src == "md" { md_to_text(&text) } else { text.clone() };
            match target {
                "html" => (if src == "md" { md_to_html(&text) } else { txt_to_html(&text) }).into_bytes(),
                "txt" if src == "md" => md_to_text(&text).into_bytes(),
                "md" if src == "txt" => text.clone().into_bytes(),
                "pdf" => text_pdf(&plain()),
                "docx" => docx(&plain())?,
                _ => return Err(unsupported()),
            }
        }
        _ => return Err(unsupported()),
    };
    fs::write(out, bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::execute_conversion;
    use std::fs;
    use std::io::Read;

    fn run(dir: &std::path::Path, name: &str, content: &str, to: &str) -> String {
        let p = dir.join(name);
        fs::write(&p, content).unwrap();
        fs::read_to_string(execute_conversion(&p, to).unwrap()).unwrap()
    }

    const SRT: &str = "1\n00:00:01,000 --> 00:00:02,500\nHello\nthere\n\n2\n00:00:03,000 --> 00:00:04,000\nBye\n";

    #[test]
    fn srt_vtt_txt() {
        let d = tempfile::tempdir().unwrap();
        let vtt = run(d.path(), "a.srt", SRT, "vtt");
        assert!(vtt.starts_with("WEBVTT\n\n00:00:01.000 --> 00:00:02.500\nHello\nthere\n"));
        let srt = run(d.path(), "a.vtt", &vtt, "srt");
        assert_eq!(srt.trim_end(), SRT.trim_end());
        let txt = run(d.path(), "b.srt", SRT, "txt");
        assert_eq!(txt, "Hello\nthere\nBye\n");
        let short = run(d.path(), "c.vtt", "WEBVTT\n\n01.000 --> 02.000 align:start\nHi\n", "srt");
        assert_eq!(short, "1
00:00:01,000 --> 00:00:02,000
Hi

");
    }

    #[test]
    fn csv_tsv_html_md() {
        let d = tempfile::tempdir().unwrap();
        let csv = "name,note\nann,\"a,b\"\nbob,\"say \"\"hi\"\"\"\n";
        let tsv = run(d.path(), "t.csv", csv, "tsv");
        assert_eq!(tsv, "name\tnote\nann\ta,b\nbob\t\"say \"\"hi\"\"\"\n");
        let back = run(d.path(), "t.tsv", &tsv, "csv");
        assert_eq!(back, csv);
        let html = run(d.path(), "u.csv", csv, "html");
        assert!(html.contains("<th>name</th>") && html.contains("<td>a,b</td>"));
        let md = run(d.path(), "v.csv", csv, "md");
        assert!(md.starts_with("| name | note |\n| --- | --- |\n"));
    }

    #[test]
    fn md_to_html_pdf_docx() {
        let d = tempfile::tempdir().unwrap();
        let md = "# Title\n\nSome *text* (with parens).\n";
        let html = run(d.path(), "n.md", md, "html");
        assert!(html.contains("<h1>Title</h1>") && html.contains("<em>text</em>"));
        let txt = run(d.path(), "n.md", md, "txt");
        assert_eq!(txt, "Title\n\nSome text (with parens).\n");

        let p = d.path().join("n.md");
        let pdf = fs::read(execute_conversion(&p, "pdf").unwrap()).unwrap();
        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert!(pdf.windows(13).any(|w| w == b"(Title) Tj T*"));
        assert!(String::from_utf8_lossy(&pdf).contains("\\(with parens\\)"));

        let docx = execute_conversion(&p, "docx").unwrap();
        let mut z = zip::ZipArchive::new(fs::File::open(docx).unwrap()).unwrap();
        for name in ["[Content_Types].xml", "_rels/.rels", "word/document.xml"] {
            assert!(z.by_name(name).is_ok(), "{name}");
        }
        let mut doc = String::new();
        z.by_name("word/document.xml").unwrap().read_to_string(&mut doc).unwrap();
        assert!(doc.contains(">Title<"));
    }

    #[test]
    fn txt_to_html_pdf_long() {
        let d = tempfile::tempdir().unwrap();
        let html = run(d.path(), "x.txt", "a < b\n\nsecond", "html");
        assert!(html.contains("<p>a &lt; b</p>") && html.contains("<p>second</p>"));
        let long: String = (0..120).map(|i| format!("line {i}\n")).collect();
        let p = d.path().join("l.txt");
        fs::write(&p, long).unwrap();
        let pdf = fs::read(execute_conversion(&p, "pdf").unwrap()).unwrap();
        assert!(String::from_utf8_lossy(&pdf).contains("/Count 3"));
    }
}
