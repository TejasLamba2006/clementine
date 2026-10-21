use std::collections::HashMap;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use flate2::read::{DeflateDecoder, ZlibDecoder};
use image::{DynamicImage, Rgba, RgbaImage};
use pulldown_cmark::{html, Event, Parser, Tag, TagEnd};

use super::{ClementineError, Result};

// ---------------------------------------------------------------- PDF 1.4

pub struct PdfPage {
    pub w: f32,
    pub h: f32,
    /// Page content stream (uncompressed).
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
pub fn text_pdf(text: &str) -> Vec<u8> {
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

// ------------------------------------------------------------------ DOCX / EPUB / PPTX / ODS

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

fn docx_to_text(bytes: &[u8]) -> Result<String> {
    let Ok(mut z) = zip::ZipArchive::new(Cursor::new(bytes)) else {
        return Ok(String::from_utf8_lossy(bytes).into_owned());
    };
    let mut xml = String::new();
    for candidate in ["word/document.xml", "index.xml", "QuickLook/Preview.txt"] {
        if let Ok(mut f) = z.by_name(candidate) {
            let _ = f.read_to_string(&mut xml);
            if !xml.is_empty() {
                break;
            }
        }
    }
    if xml.is_empty() {
        return Err(ClementineError::InvalidInput("document archive has no readable text stream".into()));
    }
    let mut paragraphs = Vec::new();
    for p in xml.split("<w:p") {
        if !p.starts_with('>') && !p.starts_with(' ') {
            continue;
        }
        let t = xml_texts_tag(p, "w:t");
        paragraphs.push(t);
    }
    if paragraphs.is_empty() {
        Ok(html_to_text(&xml, false))
    } else {
        Ok(paragraphs.join("\n").trim().to_string() + "\n")
    }
}

fn epub(text: &str) -> Result<Vec<u8>> {
    let body: String = text
        .lines()
        .map(|l| format!("<p>{}</p>\n", xml_escape(l)))
        .collect();
    let container = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\"><rootfiles><rootfile full-path=\"OEBPS/content.opf\" media-type=\"application/oebps-package+xml\"/></rootfiles></container>";
    let opf = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><package xmlns=\"http://www.idpf.org/2007/opf\" unique-identifier=\"BookId\" version=\"2.0\"><metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:title>Document</dc:title><dc:language>en</dc:language></metadata><manifest><item id=\"ch1\" href=\"chapter.xhtml\" media-type=\"application/xhtml+xml\"/></manifest><spine><itemref idref=\"ch1\"/></spine></package>";
    let xhtml = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE html><html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>Document</title></head><body>\n{body}</body></html>");
    let ze = |e: zip::result::ZipError| ClementineError::ArchiveError(e.to_string());
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    z.start_file("mimetype", stored).map_err(ze)?;
    z.write_all(b"application/epub+zip")?;
    for (name, data) in [
        ("META-INF/container.xml", container),
        ("OEBPS/content.opf", opf),
        ("OEBPS/chapter.xhtml", xhtml.as_str()),
    ] {
        z.start_file(name, deflated).map_err(ze)?;
        z.write_all(data.as_bytes())?;
    }
    Ok(z.finish().map_err(ze)?.into_inner())
}

fn epub_to_text(bytes: &[u8]) -> Result<String> {
    let ze = |e: zip::result::ZipError| ClementineError::ArchiveError(e.to_string());
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).map_err(ze)?;
    let mut out = String::new();
    for i in 0..z.len() {
        let Ok(mut f) = z.by_index(i) else { continue };
        let name = f.name().to_ascii_lowercase();
        if name.ends_with(".xhtml") || name.ends_with(".html") || name.ends_with(".htm") {
            let mut s = String::new();
            if f.read_to_string(&mut s).is_ok() {
                out.push_str(&html_to_text(&s, false));
                out.push('\n');
            }
        }
    }
    Ok(out.trim().to_string() + "\n")
}

fn pptx(text: &str) -> Result<Vec<u8>> {
    let runs: String = text
        .lines()
        .take(25)
        .map(|l| format!("<a:p><a:r><a:t>{}</a:t></a:r></a:p>", xml_escape(l)))
        .collect();
    let types = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/ppt/presentation.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml\"/><Override PartName=\"/ppt/slides/slide1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slide+xml\"/></Types>";
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"ppt/presentation.xml\"/></Relationships>";
    let pres = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><p:presentation xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"><p:sldIdLst><p:sldId id=\"256\" r:id=\"rId1\"/></p:sldIdLst></p:presentation>";
    let prels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide\" Target=\"slides/slide1.xml\"/></Relationships>";
    let slide = format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><p:sld xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"><p:cSld><p:spTree><p:sp><p:txBody>{runs}</p:txBody></p:sp></p:spTree></p:cSld></p:sld>");
    let ze = |e: zip::result::ZipError| ClementineError::ArchiveError(e.to_string());
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in [
        ("[Content_Types].xml", types),
        ("_rels/.rels", rels),
        ("ppt/presentation.xml", pres),
        ("ppt/_rels/presentation.xml.rels", prels),
        ("ppt/slides/slide1.xml", slide.as_str()),
    ] {
        z.start_file(name, o).map_err(ze)?;
        z.write_all(data.as_bytes())?;
    }
    Ok(z.finish().map_err(ze)?.into_inner())
}

fn pptx_to_text(bytes: &[u8]) -> Result<String> {
    let Ok(mut z) = zip::ZipArchive::new(Cursor::new(bytes)) else {
        return Ok(String::from_utf8_lossy(bytes).into_owned());
    };
    let mut out = Vec::new();
    for i in 0..z.len() {
        let Ok(mut f) = z.by_index(i) else { continue };
        let name = f.name().to_string();
        if name.starts_with("ppt/slides/slide") && name.ends_with(".xml") {
            let mut s = String::new();
            if f.read_to_string(&mut s).is_ok() {
                let mut slide_lines = Vec::new();
                for p in s.split("<a:p") {
                    let t = xml_texts_tag(p, "a:t");
                    if !t.trim().is_empty() {
                        slide_lines.push(t);
                    }
                }
                if !slide_lines.is_empty() {
                    out.push(slide_lines.join("\n"));
                }
            }
        }
    }
    Ok(out.join("\n\n").trim().to_string() + "\n")
}

fn rtf_to_text(rtf: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = rtf.chars().collect();
    let mut i = 0;
    let mut depth = 0i32;
    let mut skip_depth = None;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '{' => {
                depth += 1;
                let rest: String = chars[i + 1..chars.len().min(i + 16)].iter().collect();
                if skip_depth.is_none()
                    && (rest.starts_with("\\fonttbl")
                        || rest.starts_with("\\colortbl")
                        || rest.starts_with("\\stylesheet")
                        || rest.starts_with("\\info")
                        || rest.starts_with("\\*"))
                {
                    skip_depth = Some(depth);
                }
                i += 1;
            }
            '}' => {
                if skip_depth == Some(depth) {
                    skip_depth = None;
                }
                depth -= 1;
                i += 1;
            }
            '\\' => {
                i += 1;
                if i >= chars.len() {
                    break;
                }
                if chars[i] == '\\' || chars[i] == '{' || chars[i] == '}' {
                    if skip_depth.is_none() {
                        out.push(chars[i]);
                    }
                    i += 1;
                    continue;
                }
                let start = i;
                while i < chars.len() && chars[i].is_ascii_alphabetic() {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '-') {
                    i += 1;
                }
                if i < chars.len() && chars[i] == ' ' {
                    i += 1;
                }
                if skip_depth.is_none() {
                    match word.as_str() {
                        "par" | "line" => out.push('\n'),
                        "tab" => out.push('\t'),
                        _ => {}
                    }
                }
            }
            '\r' | '\n' => i += 1,
            _ => {
                if skip_depth.is_none() {
                    out.push(c);
                }
                i += 1;
            }
        }
    }
    out.trim().to_string() + "\n"
}

fn render_text_page_image(text: &str) -> DynamicImage {
    let (w, h) = (600u32, 840u32);
    let mut img = RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255]));
    // Subtle page header & ruled text representation
    for (idx, line) in text.lines().take(36).enumerate() {
        let y0 = 42 + idx as u32 * 20;
        let len_px = ((line.trim().len() as u32) * 7).clamp(0, w - 88);
        for dy in 0..8 {
            for dx in 0..len_px {
                let shade = if idx == 0 { 28 } else { 68 };
                img.put_pixel(44 + dx, y0 + dy, Rgba([shade, shade, shade + 6, 255]));
            }
        }
    }
    DynamicImage::ImageRgba8(img)
}

fn write_text_target(text: &str, src: &str, target: &str, out: &Path) -> Result<()> {
    let unsupported = || ClementineError::UnsupportedConversion { from: src.into(), to: target.into() };
    match target {
        "txt" => fs::write(out, text.as_bytes())?,
        "md" => fs::write(out, text.as_bytes())?,
        "html" => fs::write(out, txt_to_html(text).as_bytes())?,
        "pdf" => fs::write(out, text_pdf(text))?,
        "docx" | "pages" => fs::write(out, docx(text)?)?,
        "epub" => fs::write(out, epub(text)?)?,
        "pptx" => fs::write(out, pptx(text)?)?,
        "xlsx" => {
            let rows: Vec<Vec<String>> = text.lines().map(|l| vec![l.to_string()]).collect();
            fs::write(out, xlsx(&rows)?)?;
        }
        "jpg" | "png" => {
            let img = render_text_page_image(text);
            super::image_ops::write_image_pub(&img, target, out, 90)?;
        }
        _ => return Err(unsupported()),
    }
    Ok(())
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

fn ods(rows: &[Vec<String>]) -> Result<Vec<u8>> {
    let mut table = String::new();
    for r in rows {
        table.push_str("<table:table-row>");
        for c in r {
            table.push_str(&format!(
                "<table:table-cell office:value-type=\"string\"><text:p>{}</text:p></table:table-cell>",
                xml_escape(c)
            ));
        }
        table.push_str("</table:table-row>");
    }
    let content = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><office:document-content xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" xmlns:table=\"urn:oasis:names:tc:opendocument:xmlns:table:1.0\" xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\"><office:body><office:spreadsheet><table:table table:name=\"Sheet1\">{table}</table:table></office:spreadsheet></office:body></office:document-content>"
    );
    let manifest = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><manifest:manifest xmlns:manifest=\"urn:oasis:names:tc:opendocument:xmlns:manifest:1.0\"><manifest:file-entry manifest:full-path=\"/\" manifest:media-type=\"application/vnd.oasis.opendocument.spreadsheet\"/><manifest:file-entry manifest:full-path=\"content.xml\" manifest:media-type=\"text/xml\"/></manifest:manifest>";
    let ze = |e: zip::result::ZipError| ClementineError::ArchiveError(e.to_string());
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    z.start_file("mimetype", stored).map_err(ze)?;
    z.write_all(b"application/vnd.oasis.opendocument.spreadsheet")?;
    z.start_file("META-INF/manifest.xml", deflated).map_err(ze)?;
    z.write_all(manifest.as_bytes())?;
    z.start_file("content.xml", deflated).map_err(ze)?;
    z.write_all(content.as_bytes())?;
    Ok(z.finish().map_err(ze)?.into_inner())
}

fn ods_rows(bytes: &[u8]) -> Result<Vec<Vec<String>>> {
    let ze = |e: zip::result::ZipError| ClementineError::ArchiveError(e.to_string());
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).map_err(ze)?;
    let mut f = z.by_name("content.xml").map_err(ze)?;
    let mut xml = String::new();
    f.read_to_string(&mut xml)?;
    let mut rows = Vec::new();
    for r in xml.split("<table:table-row").skip(1) {
        let mut cells = Vec::new();
        for c in r.split("<table:table-cell").skip(1) {
            let body = c.split("</table:table-cell>").next().unwrap_or("");
            cells.push(xml_texts_tag(body, "text:p"));
        }
        rows.push(cells);
    }
    Ok(rows)
}

fn convert_rows(rows: &[Vec<String>], src: &str, target: &str) -> Result<Vec<u8>> {
    Ok(match target {
        "csv" => write_rows(rows, b',')?.into_bytes(),
        "tsv" => write_rows(rows, b'\t')?.into_bytes(),
        "html" => rows_html(rows).into_bytes(),
        "md" => rows_md(rows).into_bytes(),
        "xlsx" => xlsx(rows)?,
        "ods" => ods(rows)?,
        "pdf" => {
            let t: String = rows.iter().map(|r| r.join(" | ") + "\n").collect();
            text_pdf(&t)
        }
        _ => {
            return Err(ClementineError::UnsupportedConversion { from: src.into(), to: target.into() })
        }
    })
}

// ------------------------------------------------- PDF split / merge / FlateDecode

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|i| i + from)
}

fn ref_after(s: &str, key: &str) -> Option<u32> {
    let rest = s[s.find(key)? + key.len()..].trim_start();
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn split_obj(body: &[u8]) -> (String, Option<&[u8]>) {
    let stream_marker = find(body, b"stream\r\n", 0)
        .map(|i| (i, 8))
        .or_else(|| find(body, b"stream\n", 0).map(|i| (i, 7)));
    match stream_marker {
        Some((i, skip)) => {
            let data = &body[i + skip..];
            let end = data
                .windows(9)
                .rposition(|w| w == b"endstream")
                .map(|p| {
                    if p > 0 && (data[p - 1] == b'\n' || data[p - 1] == b'\r') {
                        if p > 1 && data[p - 2] == b'\r' { p - 2 } else { p - 1 }
                    } else {
                        p
                    }
                })
                .unwrap_or(data.len());
            (String::from_utf8_lossy(&body[..i]).into_owned(), Some(&data[..end]))
        }
        None => (String::from_utf8_lossy(body).into_owned(), None),
    }
}

fn decode_pdf_stream(cdict: &str, raw: &[u8]) -> Result<Vec<u8>> {
    if !cdict.contains("/Filter") {
        return Ok(raw.to_vec());
    }
    if cdict.contains("FlateDecode") {
        let mut out = Vec::new();
        if ZlibDecoder::new(raw).read_to_end(&mut out).is_ok() && !out.is_empty() {
            return Ok(out);
        }
        out.clear();
        if DeflateDecoder::new(raw).read_to_end(&mut out).is_ok() && !out.is_empty() {
            return Ok(out);
        }
        return Err(ClementineError::InvalidInput("pdf: failed to inflate FlateDecode stream".into()));
    }
    Err(ClementineError::InvalidInput("pdf: unsupported stream filter".into()))
}

/// Reads pages out of uncompressed or `FlateDecode`-compressed PDFs.
pub fn pdf_pages(bytes: &[u8]) -> Result<Vec<PdfPage>> {
    let bad = |m: &str| ClementineError::InvalidInput(format!("pdf: {m}"));
    let mut objs: HashMap<u32, &[u8]> = HashMap::new();
    let mut order = Vec::new();
    let mut start = 0;
    while let Some(e) = find(bytes, b"endobj", start) {
        let chunk = &bytes[start..e];
        start = e + 6;
        let obj_pos = find(chunk, b" 0 obj", 0);
        let Some(p) = obj_pos else { continue };
        let head = &chunk[..p];
        let digits = head.iter().rev().take_while(|b| b.is_ascii_digit()).count();
        let Ok(id) = String::from_utf8_lossy(&head[head.len() - digits..]).parse::<u32>() else {
            continue;
        };
        let mut body_start = p + 6;
        while body_start < chunk.len() && (chunk[body_start] == b'\r' || chunk[body_start] == b'\n') {
            body_start += 1;
        }
        objs.insert(id, &chunk[body_start..]);
        order.push(id);
    }
    let mut pages = Vec::new();
    for id in order {
        let (dict, _) = split_obj(objs[&id]);
        let norm = dict.replace(' ', "");
        if !norm.contains("/Type/Page") || norm.contains("/Type/Pages") {
            continue;
        }
        let cid = ref_after(&dict, "/Contents").ok_or_else(|| bad("page without /Contents"))?;
        let (cdict, data) = split_obj(objs.get(&cid).ok_or_else(|| bad("missing content"))?);
        let raw = data.ok_or_else(|| bad("content is not a stream"))?;
        let content = decode_pdf_stream(&cdict, raw)?;
        let (mut w, mut h) = (595.0, 842.0);
        if let Some(i) = dict.find("/MediaBox") {
            let inner = dict[i..].split(['[', ']']).nth(1).unwrap_or("");
            let n: Vec<f32> = inner.split_whitespace().filter_map(|t| t.parse().ok()).collect();
            if n.len() == 4 {
                (w, h) = (n[2] - n[0], n[3] - n[1]);
            }
        }
        let mut jpeg = None;
        if let Some(body) = ref_after(&dict, "/Im0").and_then(|iid| objs.get(&iid)) {
            let (idict, idata) = split_obj(body);
            if let (true, Some(d), Some(iw), Some(ih)) = (
                idict.contains("DCTDecode"),
                idata,
                ref_after(&idict, "/Width"),
                ref_after(&idict, "/Height"),
            ) {
                jpeg = Some((d.to_vec(), iw, ih));
            }
        }
        pages.push(PdfPage { w, h, content, jpeg });
    }
    if pages.is_empty() {
        return Err(bad("no pages found"));
    }
    Ok(pages)
}

pub fn pdf_to_text(bytes: &[u8]) -> Result<String> {
    let pages = pdf_pages(bytes)?;
    let mut out = String::new();
    for p in pages {
        let s = String::from_utf8_lossy(&p.content);
        let chars: Vec<char> = s.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '(' {
                i += 1;
                let mut line = String::new();
                while i < chars.len() && chars[i] != ')' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1;
                    }
                    line.push(chars[i]);
                    i += 1;
                }
                out.push_str(&line);
                out.push('\n');
            }
            i += 1;
        }
    }
    Ok(out.trim().to_string() + "\n")
}

/// Writes every page of `src` as `Page N.pdf` inside `out_dir`.
pub fn split_pdf(src: &Path, out_dir: &Path) -> Result<()> {
    fs::create_dir_all(out_dir)?;
    for (i, p) in pdf_pages(&fs::read(src)?)?.into_iter().enumerate() {
        fs::write(out_dir.join(format!("Page {}.pdf", i + 1)), build_pdf(&[p]))?;
    }
    Ok(())
}

/// Concatenates the pages of every source PDF into `out`.
pub fn merge_pdf(srcs: &[std::path::PathBuf], out: &Path) -> Result<()> {
    let mut pages = Vec::new();
    for s in srcs {
        pages.extend(pdf_pages(&fs::read(s)?)?);
    }
    fs::write(out, build_pdf(&pages))?;
    Ok(())
}

/// Reorders/filters pages by 0-based `order`; empty reverses the page order.
pub fn organize_pdf(src: &Path, order: &[usize], out: &Path) -> Result<()> {
    let pages = pdf_pages(&fs::read(src)?)?;
    let order: Vec<usize> =
        if order.is_empty() { (0..pages.len()).rev().collect() } else { order.to_vec() };
    let mut picked = Vec::new();
    for i in order {
        let p = pages.get(i).ok_or_else(|| {
            ClementineError::InvalidInput(format!("page {i} out of range ({} pages)", pages.len()))
        })?;
        picked.push(PdfPage { w: p.w, h: p.h, content: p.content.clone(), jpeg: p.jpeg.clone() });
    }
    fs::write(out, build_pdf(&picked))?;
    Ok(())
}

/// Re-serializes pages into a clean PDF (build_pdf writes no /Info dictionary).
pub fn clean_pdf(src: &Path, out: &Path) -> Result<()> {
    fs::write(out, build_pdf(&pdf_pages(&fs::read(src)?)?))?;
    Ok(())
}

// ------------------------------------------------------------------ XLSX

fn xlsx(rows: &[Vec<String>]) -> Result<Vec<u8>> {
    let mut data = String::new();
    for r in rows {
        data.push_str("<row>");
        for c in r {
            data.push_str(&format!(
                "<c t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
                xml_escape(c)
            ));
        }
        data.push_str("</row>");
    }
    let h = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>";
    let sheet = format!("{h}<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>{data}</sheetData></worksheet>");
    let types = format!("{h}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/><Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/></Types>");
    let rels = format!("{h}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>");
    let wb = format!("{h}<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets><sheet name=\"Sheet1\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>");
    let wbrels = format!("{h}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/></Relationships>");
    let ze = |e: zip::result::ZipError| ClementineError::ArchiveError(e.to_string());
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, d) in [
        ("[Content_Types].xml", &types),
        ("_rels/.rels", &rels),
        ("xl/workbook.xml", &wb),
        ("xl/_rels/workbook.xml.rels", &wbrels),
        ("xl/worksheets/sheet1.xml", &sheet),
    ] {
        z.start_file(name, o).map_err(ze)?;
        z.write_all(d.as_bytes())?;
    }
    Ok(z.finish().map_err(ze)?.into_inner())
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

fn xml_texts_tag(s: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find(&open) {
        rest = &rest[i + open.len()..];
        if !rest.starts_with('>') && !rest.starts_with(' ') {
            continue;
        }
        let Some(gt) = rest.find('>') else { break };
        rest = &rest[gt + 1..];
        let end = rest.find(&close).unwrap_or(rest.len());
        out.push_str(&unescape(&rest[..end]));
        rest = &rest[end..];
    }
    out
}

fn xml_texts(s: &str) -> String {
    xml_texts_tag(s, "t")
}

fn xlsx_rows(bytes: &[u8]) -> Result<Vec<Vec<String>>> {
    let ze = |e: zip::result::ZipError| ClementineError::ArchiveError(e.to_string());
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).map_err(ze)?;
    let mut read = |name: &str| -> Result<Option<String>> {
        match z.by_name(name) {
            Ok(mut f) => {
                let mut s = String::new();
                f.read_to_string(&mut s)?;
                Ok(Some(s))
            }
            Err(_) => Ok(None),
        }
    };
    let sheet = read("xl/worksheets/sheet1.xml")?
        .ok_or_else(|| ClementineError::InvalidInput("xlsx has no sheet1".into()))?;
    let shared: Vec<String> = read("xl/sharedStrings.xml")?
        .map(|s| s.split("<si").skip(1).map(xml_texts).collect())
        .unwrap_or_default();
    let mut rows = Vec::new();
    for row in sheet.split("<row").skip(1) {
        let mut cells = Vec::new();
        for c in row.split("<c ").skip(1) {
            let head = &c[..c.find('>').unwrap_or(c.len())];
            if head.ends_with('/') {
                cells.push(String::new());
                continue;
            }
            let body = c.split("</c>").next().unwrap_or("");
            let v = if head.contains("t=\"s\"") {
                let idx = body.split("<v>").nth(1).and_then(|v| v.split('<').next());
                idx.and_then(|i| i.trim().parse::<usize>().ok())
                    .and_then(|i| shared.get(i).cloned())
                    .unwrap_or_default()
            } else if head.contains("inlineStr") {
                xml_texts(body)
            } else {
                body.split("<v>")
                    .nth(1)
                    .and_then(|v| v.split("</v>").next())
                    .map(unescape)
                    .unwrap_or_default()
            };
            cells.push(v);
        }
        rows.push(cells);
    }
    Ok(rows)
}

// ------------------------------------------------------------------ HTML

fn html_to_text(html: &str, md: bool) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i].replace(['\n', '\r', '\t'], " "));
        let Some(gt) = rest[i..].find('>') else {
            rest = "";
            break;
        };
        let tag = rest[i + 1..i + gt].trim();
        rest = &rest[i + gt + 1..];
        let closing = tag.starts_with('/');
        let name = tag
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match name.as_str() {
            "script" | "style" if !closing => {
                let close = format!("</{name}");
                rest = rest.to_ascii_lowercase().find(&close).map(|j| &rest[j..]).unwrap_or("");
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                out.push_str("\n\n");
                if md && !closing {
                    out.push_str(&"#".repeat((name.as_bytes()[1] - b'0') as usize));
                    out.push(' ');
                }
            }
            "p" | "ul" | "ol" | "table" => out.push_str("\n\n"),
            "li" if !closing => out.push_str("\n- "),
            "br" | "div" | "tr" => out.push('\n'),
            _ => {}
        }
    }
    out.push_str(&rest.replace(['\n', '\r', '\t'], " "));
    let mut lines: Vec<String> = Vec::new();
    for l in unescape(&out).lines() {
        let l = l.split_whitespace().collect::<Vec<_>>().join(" ");
        if !(l.is_empty() && lines.last().is_none_or(|p| p.is_empty())) {
            lines.push(l);
        }
    }
    lines.join("\n").trim().to_string() + "\n"
}

// ----------------------------------------------------------------- entry

pub fn is_doc_ext(ext: &str) -> bool {
    matches!(
        ext,
        "srt" | "vtt" | "csv" | "tsv" | "md" | "txt" | "xlsx" | "xls" | "ods" | "html"
            | "pdf" | "docx" | "pages" | "epub" | "rtf" | "rtfd" | "textclipping" | "pptx" | "ppt"
    )
}

pub fn convert(path: &Path, src: &str, target: &str, out: &Path) -> Result<()> {
    let raw = fs::read(path)?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let unsupported = || ClementineError::UnsupportedConversion { from: src.into(), to: target.into() };
    match src {
        "srt" | "vtt" => {
            fs::write(out, subtitles(&text, src, target)?.into_bytes())?;
            Ok(())
        }
        "csv" | "tsv" => {
            let rows = read_rows(&text, if src == "tsv" { b'\t' } else { b',' })?;
            fs::write(out, convert_rows(&rows, src, target)?)?;
            Ok(())
        }
        "xlsx" | "xls" => {
            let rows = xlsx_rows(&raw).or_else(|_| read_rows(&text, b'\t'))?;
            fs::write(out, convert_rows(&rows, src, target)?)?;
            Ok(())
        }
        "ods" => {
            let rows = ods_rows(&raw)?;
            fs::write(out, convert_rows(&rows, src, target)?)?;
            Ok(())
        }
        "html" => match target {
            "txt" | "md" => {
                fs::write(out, html_to_text(&text, target == "md").into_bytes())?;
                Ok(())
            }
            _ => write_text_target(&html_to_text(&text, false), src, target, out),
        },
        "md" | "txt" | "textclipping" => {
            if src == "md" && target == "html" {
                fs::write(out, md_to_html(&text).into_bytes())?;
                return Ok(());
            }
            let plain = if src == "md" { md_to_text(&text) } else { text };
            write_text_target(&plain, src, target, out)
        }
        "docx" | "pages" => {
            let plain = docx_to_text(&raw)?;
            write_text_target(&plain, src, target, out)
        }
        "epub" => {
            let plain = epub_to_text(&raw)?;
            write_text_target(&plain, src, target, out)
        }
        "rtf" | "rtfd" => {
            let plain = rtf_to_text(&text);
            write_text_target(&plain, src, target, out)
        }
        "pptx" | "ppt" => {
            let plain = pptx_to_text(&raw)?;
            write_text_target(&plain, src, target, out)
        }
        "pdf" => {
            if matches!(target, "jpg" | "png") {
                if let Ok(pages) = pdf_pages(&raw) {
                    if let Some((jpeg_bytes, _, _)) = pages.first().and_then(|p| p.jpeg.as_ref()) {
                        if let Ok(img) = image::load_from_memory(jpeg_bytes) {
                            return super::image_ops::write_image_pub(&img, target, out, 90);
                        }
                    }
                }
            }
            let plain = pdf_to_text(&raw)?;
            write_text_target(&plain, src, target, out)
        }
        _ => Err(unsupported()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::execute_conversion;
    use super::*;
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
        assert_eq!(short, "1\n00:00:01,000 --> 00:00:02,000\nHi\n\n");
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

        let docx_path = execute_conversion(&p, "docx").unwrap();
        let mut z = zip::ZipArchive::new(fs::File::open(&docx_path).unwrap()).unwrap();
        for name in ["[Content_Types].xml", "_rels/.rels", "word/document.xml"] {
            assert!(z.by_name(name).is_ok(), "{name}");
        }
        let mut doc = String::new();
        z.by_name("word/document.xml").unwrap().read_to_string(&mut doc).unwrap();
        assert!(doc.contains(">Title<"));
    }

    #[test]
    fn docx_epub_pptx_ods_pdf_roundtrips_and_flatedecode() {
        let d = tempfile::tempdir().unwrap();
        let txt_path = d.path().join("sample.txt");
        fs::write(&txt_path, "Hello Clementine\nLine Two\n").unwrap();

        // TXT -> DOCX -> TXT & PDF
        let docx_p = execute_conversion(&txt_path, "docx").unwrap();
        let from_docx = fs::read_to_string(execute_conversion(&docx_p, "txt").unwrap()).unwrap();
        assert!(from_docx.contains("Hello Clementine"));

        // TXT -> EPUB -> MD
        let epub_p = execute_conversion(&txt_path, "epub").unwrap();
        let from_epub = fs::read_to_string(execute_conversion(&epub_p, "md").unwrap()).unwrap();
        assert!(from_epub.contains("Hello Clementine"));

        // TXT -> PDF -> PPTX -> PDF & JPG
        let pdf_p = execute_conversion(&txt_path, "pdf").unwrap();
        let pptx_p = execute_conversion(&pdf_p, "pptx").unwrap();
        let pptx_jpg = execute_conversion(&pptx_p, "jpg").unwrap();
        assert!(pptx_jpg.exists());

        // CSV -> ODS -> XLSX -> PDF
        let csv_p = d.path().join("data.csv");
        fs::write(&csv_p, "col1,col2\nval1,val2\n").unwrap();
        let ods_p = execute_conversion(&csv_p, "ods").unwrap();
        let xlsx_p = execute_conversion(&ods_p, "xlsx").unwrap();
        let sheet_pdf = execute_conversion(&xlsx_p, "pdf").unwrap();
        assert!(sheet_pdf.exists());

        // RTF -> TXT
        let rtf_p = d.path().join("note.rtf");
        fs::write(&rtf_p, "{\\rtf1\\ansi{\\fonttbl\\f0 Arial;}\\f0 Hello RTF\\par Second line}").unwrap();
        let rtf_txt = fs::read_to_string(execute_conversion(&rtf_p, "txt").unwrap()).unwrap();
        assert!(rtf_txt.contains("Hello RTF") && rtf_txt.contains("Second line"));

        // FlateDecode compressed PDF stream parsing
        let raw_stream = b"BT /F1 11 Tf 50 792 Td (Compressed PDF Line) Tj ET";
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(raw_stream).unwrap();
        let compressed = enc.finish().unwrap();
        let mut flate_pdf = Vec::new();
        flate_pdf.extend_from_slice(b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page /MediaBox [0 0 595 842] /Contents 4 0 R >>\nendobj\n");
        flate_pdf.extend_from_slice(format!("4 0 obj\n<< /Length {} /Filter /FlateDecode >>\nstream\n", compressed.len()).as_bytes());
        flate_pdf.extend_from_slice(&compressed);
        flate_pdf.extend_from_slice(b"\nendstream\nendobj\n%%EOF\n");
        let parsed = pdf_to_text(&flate_pdf).unwrap();
        assert!(parsed.contains("Compressed PDF Line"));
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
