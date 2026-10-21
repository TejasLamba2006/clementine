use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelMode {
    Conversions,
    Tools,
    Actions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileCategory {
    Image,
    Audio,
    Video,
    Document,
    Spreadsheet,
    Archive,
    Subtitle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolMeta {
    pub id: &'static str,
    pub title: &'static str,
    pub name: &'static str,
    pub symbol: &'static str,
    pub is_action: bool,
    pub description: &'static str,
}

const IMAGE_OUTPUTS: [&str; 8] = ["jpg", "png", "webp", "heic", "tiff", "avif", "bmp", "pdf"];
const IMAGE_INPUTS: [&str; 8] = ["jpg", "png", "webp", "heic", "tiff", "svg", "avif", "bmp"];
const AUDIO: [&str; 8] = ["mp3", "m4a", "wav", "flac", "ogg", "opus", "aiff", "wma"];
const VIDEO: [&str; 6] = ["mp4", "mov", "mkv", "webm", "avi", "wmv"];
const SUBTITLES: [&str; 2] = ["srt", "vtt"];
const DOCS_IN: [&str; 6] = ["docx", "pages", "md", "rtf", "rtfd", "textclipping"];
const SHEETS: [&str; 5] = ["xlsx", "xls", "ods", "csv", "tsv"];
const DOCS_OUT: [&str; 9] = ["docx", "pages", "pdf", "txt", "md", "html", "epub", "png", "jpg"];
const INPUT_ONLY: [&str; 6] = ["rar", "ppt", "xls", "rtf", "rtfd", "textclipping"];
const ARCHIVES: [&str; 4] = ["zip", "tar", "gz", "rar"];
const DOC_CATEGORY: [&str; 12] = [
    "pdf", "ppt", "pptx", "docx", "pages", "md", "rtf", "rtfd", "textclipping", "txt", "html",
    "epub",
];
const MULTI_TOOLS: [&str; 13] = [
    "compress",
    "removeMetadata",
    "extractText",
    "upscaleImage",
    "cropImage",
    "normalizeAudio",
    "muteVideo",
    "splitPDF",
    "extractArchive",
    "createPDF",
    "mergePDF",
    "joinVideos",
    "createCollage",
];

fn norm(ext: &str) -> String {
    ext.strip_prefix('.').unwrap_or(ext).to_ascii_lowercase()
}

pub fn conversion_targets(ext: &str) -> Vec<&'static str> {
    let ext = norm(ext);
    let e = ext.as_str();
    let raw: Vec<&'static str> = if IMAGE_INPUTS.contains(&e) {
        IMAGE_OUTPUTS.to_vec()
    } else if AUDIO.contains(&e) {
        AUDIO.to_vec()
    } else if VIDEO.contains(&e) {
        [VIDEO.as_slice(), &["gif", "mp3"]].concat()
    } else if e == "gif" {
        VIDEO.to_vec()
    } else if e == "pdf" {
        vec!["docx", "pages", "pptx", "xlsx", "jpg", "png", "epub", "txt", "md"]
    } else if e == "ppt" || e == "pptx" {
        vec!["pdf", "jpg", "png"]
    } else if SHEETS.contains(&e) {
        [SHEETS.as_slice(), &["pdf"]].concat()
    } else if DOCS_IN.contains(&e) {
        DOCS_OUT.to_vec()
    } else if e == "epub" {
        vec!["pdf", "docx", "pages", "txt", "md", "html"]
    } else if e == "txt" {
        vec!["docx", "pages", "pdf", "md", "html", "epub", "jpg", "png"]
    } else if SUBTITLES.contains(&e) {
        [SUBTITLES.as_slice(), &["txt"]].concat()
    } else if ARCHIVES.contains(&e) {
        vec!["zip", "tar", "gz"]
    } else {
        vec![]
    };
    raw.into_iter()
        .filter(|a| *a != e && !INPUT_ONLY.contains(a))
        .collect()
}

pub fn format_label(ext: &str) -> String {
    if ext.eq_ignore_ascii_case("gz") {
        "GZIP".to_string()
    } else {
        ext.to_ascii_uppercase()
    }
}

pub fn file_category(ext: &str) -> Option<FileCategory> {
    let ext = norm(ext);
    let e = ext.as_str();
    if AUDIO.contains(&e) {
        Some(FileCategory::Audio)
    } else if VIDEO.contains(&e) || e == "gif" {
        Some(FileCategory::Video)
    } else if DOC_CATEGORY.contains(&e) {
        Some(FileCategory::Document)
    } else if SHEETS.contains(&e) {
        Some(FileCategory::Spreadsheet)
    } else if ARCHIVES.contains(&e) {
        Some(FileCategory::Archive)
    } else if SUBTITLES.contains(&e) {
        Some(FileCategory::Subtitle)
    } else if IMAGE_INPUTS.contains(&e) {
        Some(FileCategory::Image)
    } else {
        None
    }
}

pub fn tool_targets(ext: &str, count: usize) -> Vec<&'static str> {
    if count < 1 {
        return vec![];
    }
    let ext = norm(ext);
    let multi = count > 1;
    let raw: Vec<&'static str> = match ext.as_str() {
        "jpg" | "png" | "webp" | "heic" | "tiff" | "bmp" | "avif" => {
            if multi {
                vec![
                    "compress",
                    "removeMetadata",
                    "extractText",
                    "upscaleImage",
                    "cropImage",
                    "createPDF",
                    "createCollage",
                ]
            } else {
                vec![
                    "compress",
                    "removeMetadata",
                    "removeBg",
                    "upscaleImage",
                    "extractText",
                    "editImage",
                    "annotateImage",
                    "frameImage",
                    "cropImage",
                    "redactImage",
                ]
            }
        }
        "svg" => {
            if multi {
                vec!["createPDF", "createCollage"]
            } else {
                vec![]
            }
        }
        "mp3" | "m4a" | "wav" | "flac" => vec![
            "compress",
            "removeMetadata",
            "normalizeAudio",
            "audioToVideo",
            "trimAudio",
            "audioChannels",
            "redactAudio",
        ],
        "mp4" | "mov" | "mkv" => {
            if multi {
                vec![
                    "compress",
                    "removeMetadata",
                    "muteVideo",
                    "trimVideo",
                    "cropVideo",
                    "changeVideoSpeed",
                    "joinVideos",
                ]
            } else {
                vec![
                    "compress",
                    "removeMetadata",
                    "muteVideo",
                    "trimVideo",
                    "cropVideo",
                    "changeVideoSpeed",
                    "videoSnapshots",
                    "splitVideo",
                    "redactVideo",
                ]
            }
        }
        "gif" => vec!["removeMetadata"],
        "pdf" => {
            if multi {
                vec!["compress", "removeMetadata", "extractText", "splitPDF", "mergePDF"]
            } else {
                vec!["compress", "removeMetadata", "extractText", "splitPDF", "organizePDF"]
            }
        }
        "zip" | "tar" | "gz" | "rar" => vec!["extractArchive"],
        _ => vec![],
    };
    raw.into_iter()
        .filter(|id| count == 1 || MULTI_TOOLS.contains(id))
        .collect()
}

pub fn action_targets() -> Vec<&'static str> {
    vec!["airDrop", "float", "copy", "move", "copyPath", "openTerminal"]
}

const fn m(
    id: &'static str,
    title: &'static str,
    name: &'static str,
    is_action: bool,
    description: &'static str,
) -> ToolMeta {
    // symbol mirrors id; the UI layer maps it to an icon asset
    ToolMeta { id, title, name, symbol: id, is_action, description }
}

const TOOL_META: [ToolMeta; 35] = [
    m("compress", "COMPRESS", "Compress", false, "Shrink file size"),
    m("removeMetadata", "METADATA", "Metadata", false, "Strip embedded metadata"),
    m("removeBg", "REMOVE BG", "Remove BG", false, "Remove background locally"),
    m("upscaleImage", "UPSCALE", "Upscale & Sharpen", false, "Upscale 2x or 4x locally"),
    m("extractText", "OCR TEXT", "Extract Text (OCR)", false, "Extract text offline"),
    m("editImage", "EDIT", "Edit Photo", false, "Adjust photo settings"),
    m("annotateImage", "ANNOTATE", "Annotate Photo", false, "Draw on the image"),
    m("frameImage", "ADD BG", "Add BG", false, "Add a background frame"),
    m("cropImage", "CROP", "Crop Image", false, "Crop to a region"),
    m("redactImage", "REDACT", "Redact Photo", false, "Hide sensitive areas"),
    m("createPDF", "CREATE PDF", "Create PDF", false, "Combine images into a PDF"),
    m("createCollage", "COLLAGE", "Create Collage", false, "Arrange images in a grid"),
    m("normalizeAudio", "NORMALIZE", "Normalize Volume", false, "Even out loudness"),
    m("audioToVideo", "VISUALIZE", "Audio Visualizer", false, "Turn audio into a video"),
    m("trimAudio", "TRIM", "Trim Audio", false, "Cut audio to a range"),
    m("audioChannels", "CHANNELS", "Convert Audio Channels", false, "Change channel layout"),
    m("redactAudio", "BLEEP", "Bleep Audio", false, "Silence sections of audio"),
    m("muteVideo", "MUTE", "Remove Audio", false, "Remove the audio track"),
    m("trimVideo", "TRIM", "Trim Video", false, "Cut video to a range"),
    m("cropVideo", "CROP", "Crop Video", false, "Crop video frame"),
    m("changeVideoSpeed", "SPEED", "Change Video Speed", false, "Speed up or slow down"),
    m("videoSnapshots", "SNAPSHOT", "Video Snapshots", false, "Save frames as images"),
    m("splitVideo", "SPLIT", "Split Video", false, "Split into clips"),
    m("redactVideo", "REDACT", "Redact Video", false, "Hide areas of the video"),
    m("joinVideos", "JOIN", "Join Videos", false, "Merge videos into one"),
    m("splitPDF", "SPLIT", "Split PDF", false, "Split into separate pages"),
    m("organizePDF", "ORGANIZE", "Organize PDF", false, "Reorder or remove pages"),
    m("mergePDF", "MERGE PDF", "Merge PDFs", false, "Merge PDFs into one"),
    m("extractArchive", "EXTRACT", "Extract Archive", false, "Unpack the archive"),
    m("airDrop", "SHARE", "Nearby Share", true, "Share with nearby devices"),
    m("float", "FLOAT", "Float", true, "Pin to the floating shelf"),
    m("copy", "COPY", "Copy", true, "Copy files to the clipboard"),
    m("move", "MOVE", "Move", true, "Move files to a folder"),
    m("copyPath", "COPY PATH", "Copy Path", true, "Copy full file paths"),
    m("openTerminal", "TERMINAL", "Terminal", true, "Open a terminal here"),
];

pub fn tool_meta(id: &str) -> Option<ToolMeta> {
    TOOL_META.iter().find(|t| t.id == id).copied()
}

fn strip_ext(name: &str) -> &str {
    for suf in [".tar.gz", ".tgz"] {
        if let Some(s) = name.strip_suffix(suf) {
            return s;
        }
    }
    match name.rfind('.') {
        Some(i) if i > 0 => &name[..i],
        _ => name,
    }
}

pub fn next_conversion_filename(
    source_name: &str,
    target_ext: &str,
    existing: &HashSet<String>,
) -> String {
    let stem = strip_ext(source_name);
    let ext = if target_ext == "gz" { "tar.gz" } else { target_ext };
    let mut n = 1;
    loop {
        let name = if n == 1 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem} {n}.{ext}")
        };
        if !existing.contains(&name) {
            return name;
        }
        n += 1;
    }
}

fn tool_output(tool_id: &str, source_ext: &str) -> (&'static str, String) {
    let src = source_ext.to_string();
    match tool_id {
        "compress" => ("Compressed", if source_ext == "wav" { "flac".into() } else { src }),
        "removeMetadata" => ("No Metadata", src),
        "removeBg" => ("No BG", "png".into()),
        "upscaleImage" => ("Upscaled", src),
        "extractText" => ("OCR", "txt".into()),
        "editImage" => ("Edited", src),
        "annotateImage" => ("Annotated", "png".into()),
        "frameImage" => ("Background", src),
        "cropImage" => ("Cropped", src),
        "redactImage" => ("Redacted", src),
        "createPDF" => ("Combined", "pdf".into()),
        "createCollage" => ("Collage", "png".into()),
        "normalizeAudio" => ("Normalized", src),
        "audioToVideo" => ("Visualizer", "mp4".into()),
        "trimAudio" => ("Trimmed", src),
        "audioChannels" => ("Mono", src),
        "redactAudio" => ("Bleeped", src),
        "muteVideo" => ("Muted", src),
        "trimVideo" => ("Trimmed", src),
        "cropVideo" => ("Cropped", src),
        "changeVideoSpeed" => ("Speed 2x", src),
        "videoSnapshots" => ("Snapshot", "png".into()),
        "splitVideo" => ("Clips", "folder".into()),
        "redactVideo" => ("Redacted", src),
        "joinVideos" => ("Joined", "mp4".into()),
        "splitPDF" => ("Pages", "folder".into()),
        "organizePDF" => ("Organized", "pdf".into()),
        "mergePDF" => ("Merged", "pdf".into()),
        "extractArchive" => ("Extracted", "folder".into()),
        _ => ("Output", src),
    }
}

pub fn next_tool_filename(
    source_name: &str,
    source_ext: &str,
    tool_id: &str,
    existing: &HashSet<String>,
) -> (String, String) {
    let stem = strip_ext(source_name);
    let (label, out_format) = tool_output(tool_id, source_ext);
    let ext_part = if out_format == "folder" { String::new() } else { format!(".{out_format}") };
    let mut n = 1;
    loop {
        let name = if n == 1 {
            format!("{stem} {label}{ext_part}")
        } else {
            format!("{stem} {label} {n}{ext_part}")
        };
        if !existing.contains(&name) {
            return (name, out_format);
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_IN: &[&str] = &[
        "jpg", "png", "webp", "heic", "tiff", "svg", "avif", "bmp", "mp3", "m4a", "wav", "flac",
        "ogg", "opus", "aiff", "wma", "mp4", "mov", "mkv", "webm", "avi", "wmv", "gif", "pdf",
        "ppt", "pptx", "xlsx", "xls", "ods", "csv", "tsv", "docx", "pages", "md", "rtf", "rtfd",
        "textclipping", "epub", "txt", "srt", "vtt", "zip", "tar", "gz", "rar",
    ];

    #[test]
    fn conversion_path_count() {
        let n: usize = ALL_IN.iter().map(|e| conversion_targets(e).len()).sum();
        assert_eq!(n, 275);
    }

    #[test]
    fn conversion_rules() {
        assert_eq!(conversion_targets(".JPG").len(), 7);
        assert!(!conversion_targets("xls").contains(&"xls"));
        assert!(conversion_targets("rar").iter().all(|t| *t != "rar"));
        assert_eq!(conversion_targets("gz"), vec!["zip", "tar"]);
        assert!(conversion_targets("mp4").contains(&"gif"));
        assert!(conversion_targets("unknown").is_empty());
        assert_eq!(format_label("gz"), "GZIP");
        assert_eq!(format_label("png"), "PNG");
    }

    #[test]
    fn categories() {
        assert_eq!(file_category("svg"), Some(FileCategory::Image));
        assert_eq!(file_category("gif"), Some(FileCategory::Video));
        assert_eq!(file_category("vtt"), Some(FileCategory::Subtitle));
        assert_eq!(file_category("xyz"), None);
    }

    #[test]
    fn tool_targets_rules() {
        assert!(tool_targets("jpg", 0).is_empty());
        assert_eq!(tool_targets("jpg", 1).len(), 10);
        assert!(tool_targets("jpg", 1).contains(&"removeBg"));
        assert!(tool_targets("jpg", 1).contains(&"upscaleImage"));
        assert!(tool_targets("jpg", 1).contains(&"extractText"));
        assert_eq!(
            tool_targets("jpg", 3),
            vec![
                "compress",
                "removeMetadata",
                "extractText",
                "upscaleImage",
                "cropImage",
                "createPDF",
                "createCollage"
            ]
        );
        assert_eq!(
            tool_targets("mp4", 2),
            vec!["compress", "removeMetadata", "muteVideo", "joinVideos"]
        );
        assert_eq!(
            tool_targets("pdf", 2),
            vec!["compress", "removeMetadata", "extractText", "splitPDF", "mergePDF"]
        );
        assert!(tool_targets("svg", 1).is_empty());
        assert_eq!(tool_targets("rar", 1), vec!["extractArchive"]);
    }

    #[test]
    fn meta_covers_26_tools_and_6_actions() {
        assert_eq!(TOOL_META.iter().filter(|t| !t.is_action).count(), 29);
        let actions = action_targets();
        assert_eq!(actions.len(), 6);
        for a in &actions {
            assert!(tool_meta(a).unwrap().is_action);
        }
        assert_eq!(tool_meta("airDrop").unwrap().title, "SHARE");
        assert_eq!(tool_meta("copyPath").unwrap().name, "Copy Path");
        for e in ALL_IN {
            for c in [1, 2] {
                for id in tool_targets(e, c) {
                    assert!(tool_meta(id).is_some(), "{id}");
                }
            }
        }
    }

    fn set(v: &[&str]) -> HashSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn conversion_filenames() {
        let none = HashSet::new();
        assert_eq!(next_conversion_filename("a.png", "jpg", &none), "a.jpg");
        assert_eq!(next_conversion_filename("a.png", "jpg", &set(&["a.jpg"])), "a 2.jpg");
        assert_eq!(
            next_conversion_filename("a.png", "jpg", &set(&["a.jpg", "a 2.jpg"])),
            "a 3.jpg"
        );
        assert_eq!(next_conversion_filename("b.tar.gz", "zip", &none), "b.zip");
        assert_eq!(next_conversion_filename("b.tgz", "zip", &none), "b.zip");
        assert_eq!(next_conversion_filename("b.zip", "gz", &none), "b.tar.gz");
        assert_eq!(next_conversion_filename("my.file.mp3", "wav", &none), "my.file.wav");
    }

    #[test]
    fn tool_filenames() {
        let none = HashSet::new();
        assert_eq!(
            next_tool_filename("a.jpg", "jpg", "compress", &none),
            ("a Compressed.jpg".to_string(), "jpg".to_string())
        );
        assert_eq!(
            next_tool_filename("a.wav", "wav", "compress", &none),
            ("a Compressed.flac".to_string(), "flac".to_string())
        );
        assert_eq!(
            next_tool_filename("a.jpg", "jpg", "compress", &set(&["a Compressed.jpg"])).0,
            "a Compressed 2.jpg"
        );
        assert_eq!(
            next_tool_filename("v.mp4", "mp4", "splitVideo", &none),
            ("v Clips".to_string(), "folder".to_string())
        );
        assert_eq!(
            next_tool_filename("v.mp4", "mp4", "splitVideo", &set(&["v Clips"])).0,
            "v Clips 2"
        );
    }

    #[test]
    fn all_tools_meta_and_output() {
        let none = HashSet::new();
        // (id, title, name, source ext, expected output filename)
        let rows: [(&str, &str, &str, &str, &str); 26] = [
            ("compress", "COMPRESS", "Compress", "jpg", "a Compressed.jpg"),
            ("removeMetadata", "METADATA", "Metadata", "jpg", "a No Metadata.jpg"),
            ("editImage", "EDIT", "Edit Photo", "jpg", "a Edited.jpg"),
            ("annotateImage", "ANNOTATE", "Annotate Photo", "jpg", "a Annotated.png"),
            ("frameImage", "ADD BG", "Add BG", "jpg", "a Background.jpg"),
            ("cropImage", "CROP", "Crop Image", "jpg", "a Cropped.jpg"),
            ("redactImage", "REDACT", "Redact Photo", "jpg", "a Redacted.jpg"),
            ("createPDF", "CREATE PDF", "Create PDF", "jpg", "a Combined.pdf"),
            ("createCollage", "COLLAGE", "Create Collage", "jpg", "a Collage.png"),
            ("normalizeAudio", "NORMALIZE", "Normalize Volume", "mp3", "a Normalized.mp3"),
            ("audioToVideo", "VISUALIZE", "Audio Visualizer", "mp3", "a Visualizer.mp4"),
            ("trimAudio", "TRIM", "Trim Audio", "mp3", "a Trimmed.mp3"),
            ("audioChannels", "CHANNELS", "Convert Audio Channels", "mp3", "a Mono.mp3"),
            ("redactAudio", "BLEEP", "Bleep Audio", "mp3", "a Bleeped.mp3"),
            ("muteVideo", "MUTE", "Remove Audio", "mp4", "a Muted.mp4"),
            ("trimVideo", "TRIM", "Trim Video", "mp4", "a Trimmed.mp4"),
            ("cropVideo", "CROP", "Crop Video", "mp4", "a Cropped.mp4"),
            ("changeVideoSpeed", "SPEED", "Change Video Speed", "mp4", "a Speed 2x.mp4"),
            ("videoSnapshots", "SNAPSHOT", "Video Snapshots", "mp4", "a Snapshot.png"),
            ("splitVideo", "SPLIT", "Split Video", "mp4", "a Clips"),
            ("redactVideo", "REDACT", "Redact Video", "mp4", "a Redacted.mp4"),
            ("joinVideos", "JOIN", "Join Videos", "mp4", "a Joined.mp4"),
            ("splitPDF", "SPLIT", "Split PDF", "pdf", "a Pages"),
            ("organizePDF", "ORGANIZE", "Organize PDF", "pdf", "a Organized.pdf"),
            ("mergePDF", "MERGE PDF", "Merge PDFs", "pdf", "a Merged.pdf"),
            ("extractArchive", "EXTRACT", "Extract Archive", "zip", "a Extracted"),
        ];
        for (id, title, name, ext, out) in rows {
            let t = tool_meta(id).unwrap();
            assert_eq!((t.title, t.name), (title, name), "{id}");
            let src = format!("a.{ext}");
            assert_eq!(next_tool_filename(&src, ext, id, &none).0, out, "{id}");
        }
    }
}
