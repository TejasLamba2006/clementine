use std::fs;
use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
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
        "xlsx" => xlsx(&rows)?,
        "pdf" => {
            let t: String = rows.iter().map(|r| r.join(" | ") + "\n").collect();
            text_pdf(&t)
        }
        _ => {
            return Err(ClementineError::UnsupportedConversion { from: src.into(), to: target.into() })
        }
    })
}

// ------------------------------------------------- PDF split / merge

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|i| i + from)
}

fn ref_after(s: &str, key: &str) -> Option<u32> {
    let rest = s[s.find(key)? + key.len()..].trim_start();
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Splits an object body into its dictionary text and optional stream bytes.
fn split_obj(body: &[u8]) -> (String, Option<&[u8]>) {
    match find(body, b"stream\n", 0) {
        Some(i) => {
            let data = &body[i + 7..];
            let end = data.windows(10).rposition(|w| w == b"\nendstream").unwrap_or(data.len());
            (String::from_utf8_lossy(&body[..i]).into_owned(), Some(&data[..end]))
        }
        None => (String::from_utf8_lossy(body).into_owned(), None),
    }
}

/// Reads pages back out of an uncompressed PDF (the shape `build_pdf` writes).
/// ponytail: no filters, xref streams or object streams; swap in a real PDF
/// parser (lopdf) when foreign PDFs must be split.
pub fn pdf_pages(bytes: &[u8]) -> Result<Vec<PdfPage>> {
    let bad = |m: &str| ClementineError::InvalidInput(format!("pdf: {m}"));
    let mut objs: HashMap<u32, &[u8]> = HashMap::new();
    let mut order = Vec::new();
    let mut start = 0;
    while let Some(e) = find(bytes, b"\nendobj", start) {
        let chunk = &bytes[start..e];
        start = e + 7;
        let Some(p) = find(chunk, b" 0 obj\n", 0) else { continue };
        let head = &chunk[..p];
        let digits = head.iter().rev().take_while(|b| b.is_ascii_digit()).count();
        let Ok(id) = String::from_utf8_lossy(&head[head.len() - digits..]).parse::<u32>() else {
            continue;
        };
        objs.insert(id, &chunk[p + 7..]);
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
        if cdict.contains("/Filter") {
            return Err(bad("compressed content streams are not supported"));
        }
        let content = data.ok_or_else(|| bad("content is not a stream"))?.to_vec();
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

/// Concatenated text of every `<t>` element in `s`.
fn xml_texts(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("<t") {
        rest = &rest[i + 2..];
        if !rest.starts_with('>') && !rest.starts_with(' ') {
            continue;
        }
        let Some(gt) = rest.find('>') else { break };
        rest = &rest[gt + 1..];
        let end = rest.find("</t>").unwrap_or(rest.len());
        out.push_str(&unescape(&rest[..end]));
        rest = &rest[end..];
    }
    out
}

/// ponytail: first sheet only; empty cells are not padded by column reference.
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

pub fn convert(path: &Path, src: &str, target: &str, out: &Path) -> Result<()> {
    let raw = fs::read(path)?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let unsupported = || ClementineError::UnsupportedConversion { from: src.into(), to: target.into() };
    let bytes: Vec<u8> = match src {
        "srt" | "vtt" => subtitles(&text, src, target)?.into_bytes(),
        "csv" | "tsv" => sheet(&text, src, target)?,
        "xlsx" => match target {
            "csv" => write_rows(&xlsx_rows(&raw)?, b',')?.into_bytes(),
            "tsv" => write_rows(&xlsx_rows(&raw)?, b'\t')?.into_bytes(),
            _ => return Err(unsupported()),
        },
        "html" => match target {
            "txt" | "md" => html_to_text(&text, target == "md").into_bytes(),
            _ => return Err(unsupported()),
        },
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
