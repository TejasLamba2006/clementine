use std::path::{Path, PathBuf};
use std::process::Command;

use super::{ClementineError, Result, ToolParams};

const AUDIO: [&str; 8] = ["mp3", "m4a", "wav", "flac", "ogg", "opus", "aiff", "wma"];
const VIDEO: [&str; 6] = ["mp4", "mov", "mkv", "webm", "avi", "wmv"];

pub fn is_audio_ext(e: &str) -> bool {
    AUDIO.contains(&e)
}

pub fn is_video_ext(e: &str) -> bool {
    VIDEO.contains(&e)
}

pub fn is_media_ext(e: &str) -> bool {
    is_audio_ext(e) || is_video_ext(e)
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn p(x: &Path) -> String {
    x.to_string_lossy().into_owned()
}

fn convert_flags(target: &str) -> Vec<String> {
    s(match target {
        "mp3" => &["-vn", "-c:a", "libmp3lame", "-q:a", "2"][..],
        "m4a" => &["-vn", "-c:a", "aac", "-b:a", "192k"],
        "wav" => &["-vn", "-c:a", "pcm_s16le"],
        "flac" => &["-vn", "-c:a", "flac"],
        "ogg" => &["-vn", "-c:a", "libvorbis", "-q:a", "5"],
        "opus" => &["-vn", "-c:a", "libopus", "-b:a", "128k"],
        "aiff" => &["-vn", "-c:a", "pcm_s16be"],
        "wma" => &["-vn", "-c:a", "wmav2"],
        "mp4" => &["-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv420p", "-movflags", "+faststart"],
        "mov" => &["-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv420p"],
        "mkv" => &["-c:v", "libx264", "-c:a", "aac"],
        "webm" => &["-c:v", "libvpx-vp9", "-c:a", "libopus"],
        "avi" => &["-c:v", "mpeg4", "-c:a", "libmp3lame"],
        "wmv" => &["-c:v", "wmv2", "-c:a", "wmav2"],
        "gif" => &["-vf", "fps=12,scale=480:-1:flags=lanczos", "-an"],
        _ => &[],
    })
}

/// Chains `atempo` filters since each one only accepts 0.5..=2.0.
fn atempo_chain(mut f: f32) -> String {
    let mut parts = Vec::new();
    while f > 2.0 {
        parts.push("atempo=2.0".to_string());
        f /= 2.0;
    }
    while f < 0.5 {
        parts.push("atempo=0.5".to_string());
        f /= 0.5;
    }
    parts.push(format!("atempo={f}"));
    parts.join(",")
}

/// Builds the ffmpeg argument list. `op` is `"convert"` (target format is the
/// extension of `out`) or a tool id. The caller prepends nothing and appends
/// nothing: the returned list is complete, ending with the output path.
pub fn build_ffmpeg_args(inputs: &[PathBuf], op: &str, params: &ToolParams, out: &Path) -> Vec<String> {
    let mut a = s(&["-y", "-hide_banner", "-loglevel", "error"]);
    let first = &inputs[0];
    let in_ext = first.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let out_ext = out.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let is_video = is_video_ext(&in_ext);
    let times = |a: &mut Vec<String>| {
        a.push("-ss".into());
        a.push(params.start_sec.to_string());
        if let Some(e) = params.end_sec {
            a.push("-to".into());
            a.push(e.to_string());
        }
    };
    let ordered: Vec<&PathBuf> = if op == "joinVideos" && !params.page_order.is_empty() {
        params.page_order.iter().filter_map(|&idx| inputs.get(idx)).collect()
    } else {
        inputs.iter().collect()
    };
    for i in &ordered {
        a.push("-i".into());
        a.push(p(i));
    }
    match op {
        "convert" => a.extend(convert_flags(&out_ext)),
        "compress" => a.extend(if is_video {
            s(&["-c:v", "libx264", "-crf", "28", "-preset", "medium", "-c:a", "aac", "-b:a", "128k"])
        } else {
            s(&["-b:a", "96k"])
        }),
        "removeMetadata" => a.extend(s(&["-map_metadata", "-1", "-c", "copy"])),
        "muteVideo" => a.extend(s(&["-an", "-c:v", "copy"])),
        "trimVideo" => times(&mut a),
        "trimAudio" => {
            times(&mut a);
            if params.trim_silence {
                // Trim leading and trailing silence while preserving internal pauses
                a.extend(s(&[
                    "-af",
                    "silenceremove=start_periods=1:start_duration=0.05:start_threshold=-42dB,areverse,silenceremove=start_periods=1:start_duration=0.05:start_threshold=-42dB,areverse",
                ]));
            }
        }
        "changeVideoSpeed" => {
            let f = params.speed_factor;
            a.extend(s(&[
                "-filter_complex",
                &format!("[0:v]setpts=PTS/{f}[v];[0:a]{}[a]", atempo_chain(f)),
                "-map",
                "[v]",
                "-map",
                "[a]",
            ]));
        }
        "normalizeAudio" => {
            let af = format!(
                "loudnorm=I={}:TP={}:LRA={}",
                params.target_lufs, params.true_peak, params.loudness_range
            );
            a.extend(s(&["-af", &af]));
        }
        "audioChannels" => {
            let lg = params.left_gain;
            let rg = params.right_gain;
            if (lg - 1.0).abs() > 0.001 || (rg - 1.0).abs() > 0.001 {
                let pan = if params.mono {
                    format!("pan=mono|c0={:.2}*c0+{:.2}*c1", lg * 0.5, rg * 0.5)
                } else {
                    format!("pan=stereo|c0={lg:.2}*c0|c1={rg:.2}*c1")
                };
                a.extend(s(&["-af", &pan, "-ac", if params.mono { "1" } else { "2" }]));
            } else {
                a.extend(s(&["-ac", if params.mono { "1" } else { "2" }]));
            }
        }
        "cropVideo" | "redactVideo" => {
            let vf = match (op, params.crop_rect) {
                ("cropVideo", Some((x, y, w, h))) => format!("crop={w}:{h}:{x}:{y}"),
                ("cropVideo", None) => "crop=trunc(iw*0.8/2)*2:trunc(ih*0.8/2)*2".into(),
                (_, Some((x, y, w, h))) => {
                    if let Some(end) = params.end_sec {
                        let color = if params.redact_solid { "black@1" } else { "black@0.88" };
                        format!("drawbox=x={x}:y={y}:w={w}:h={h}:color={color}:t=fill:enable='between(t,{},{end})'", params.start_sec)
                    } else {
                        format!("drawbox=x={x}:y={y}:w={w}:h={h}:color=black@1:t=fill")
                    }
                }
                (_, None) => "drawbox=x=iw/4:y=ih/4:w=iw/2:h=ih/2:color=black@1:t=fill".into(),
            };
            a.extend(s(&["-vf", &vf, "-c:a", "copy"]));
        }
        "audioToVideo" => {
            let sz = match params.visualizer_shape {
                1 => "720x1280",
                2 => "720x720",
                _ => "1280x720",
            };
            let mode = if params.visualizer_bars { "p2p" } else { "cline" };
            a.extend(s(&[
                "-filter_complex",
                &format!("[0:a]showwaves=s={sz}:mode={mode}:colors=0xff5200,format=yuv420p[v]"),
                "-map",
                "[v]",
                "-map",
                "0:a",
                "-c:v",
                "libx264",
                "-c:a",
                "aac",
            ]));
        }
        "redactAudio" => {
            let end = params.end_sec.map(|e| e.to_string()).unwrap_or_else(|| "9999999".into());
            a.extend(s(&["-af", &format!("volume=enable='between(t,{},{end})':volume=0", params.start_sec)]));
        }
        "joinVideos" => {
            let n = ordered.len();
            let streams: String = (0..n).map(|i| format!("[{i}:v][{i}:a]")).collect();
            a.extend(s(&[
                "-filter_complex",
                &format!("{streams}concat=n={n}:v=1:a=1[v][a]"),
                "-map",
                "[v]",
                "-map",
                "[a]",
            ]));
        }
        "splitVideo" => {
            if !params.split_points.is_empty() {
                let pts: String = params
                    .split_points
                    .iter()
                    .map(|t| format!("{t:.2}"))
                    .collect::<Vec<_>>()
                    .join(",");
                a.extend(s(&["-c", "copy", "-map", "0", "-f", "segment", "-segment_times", &pts, "-reset_timestamps", "1"]));
            } else {
                let end = params.end_sec.unwrap_or(10.0);
                a.extend(s(&["-c", "copy", "-map", "0", "-f", "segment", "-segment_time", &end.to_string(), "-reset_timestamps", "1"]));
            }
            a.push(p(&out.join(format!("clip_%03d.{in_ext}"))));
            return a;
        }
        "videoSnapshots" => {
            let t = params.snapshot_times.first().copied().unwrap_or(params.start_sec);
            let at = a.iter().position(|x| x == "-i").unwrap_or(a.len());
            a.splice(at..at, s(&["-ss", &t.to_string()]));
            a.extend(s(&["-frames:v", "1"]));
        }
        _ => {}
    }
    a.push(p(out));
    a
}

pub fn find_ffmpeg() -> PathBuf {
    let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let c = dir.join(exe);
            if c.is_file() {
                return c;
            }
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let c = PathBuf::from(local).join("Clementine").join("bin").join("ffmpeg.exe");
        if c.is_file() {
            return c;
        }
    }
    PathBuf::from(exe)
}

/// Probes the duration in seconds of a media file using `ffmpeg -i`.
pub fn probe_duration_secs(path: &Path) -> f32 {
    let mut cmd = Command::new(find_ffmpeg());
    cmd.args(["-hide_banner", "-i"]).arg(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let Ok(out) = cmd.output() else { return 4.0 };
    let stderr = String::from_utf8_lossy(&out.stderr);
    for line in stderr.lines() {
        if let Some(idx) = line.find("Duration:") {
            let rest = line[idx + 9..].trim();
            let tc = rest.split(',').next().unwrap_or("").trim();
            let parts: Vec<&str> = tc.split(':').collect();
            if parts.len() == 3 {
                let h: f32 = parts[0].parse().unwrap_or(0.0);
                let m: f32 = parts[1].parse().unwrap_or(0.0);
                let s: f32 = parts[2].parse().unwrap_or(0.0);
                let total = h * 3600.0 + m * 60.0 + s;
                if total > 0.05 {
                    return total;
                }
            }
        }
    }
    4.0
}

/// Extracts a single video frame at `time_sec` scaled within `max_w x max_h` via `ffmpeg` image2pipe.
pub fn extract_video_frame_rgba(path: &Path, time_sec: f32, max_w: u32, _max_h: u32) -> Option<image::RgbaImage> {
    let mut cmd = Command::new(find_ffmpeg());
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-ss",
        &format!("{:.2}", time_sec.max(0.0)),
        "-i",
    ])
    .arg(path)
    .args([
        "-frames:v",
        "1",
        "-vf",
        &format!("scale={max_w}:-1"),
        "-f",
        "image2pipe",
        "-vcodec",
        "png",
        "-",
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let out = cmd.output().ok()?;
    if !out.status.success() || out.stdout.is_empty() {
        return None;
    }
    image::load_from_memory_with_format(&out.stdout, image::ImageFormat::Png)
        .ok()
        .map(|im| im.to_rgba8())
}

/// Extracts `count` evenly-spaced thumbnail frames across the video's duration for the timeline filmstrip.
pub fn extract_video_filmstrip(path: &Path, duration: f32, count: usize, tile_w: u32, tile_h: u32) -> Vec<image::RgbaImage> {
    let mut tiles = Vec::with_capacity(count);
    let dur = duration.max(0.5);
    for i in 0..count {
        let t = ((i as f32 + 0.35) / count.max(1) as f32) * (dur - 0.1).max(0.0);
        if let Some(fr) = extract_video_frame_rgba(path, t, tile_w * 2, tile_h * 2) {
            tiles.push(image::imageops::resize(&fr, tile_w, tile_h, image::imageops::FilterType::Triangle));
        }
    }
    tiles
}

fn run(args: &[String]) -> Result<()> {
    let mut cmd = Command::new(find_ffmpeg());
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let o = cmd
        .output()
        .map_err(|e| ClementineError::MediaError(format!("cannot launch ffmpeg: {e}")))?;
    if o.status.success() {
        Ok(())
    } else {
        Err(ClementineError::MediaError(String::from_utf8_lossy(&o.stderr).trim().to_string()))
    }
}

pub fn convert(path: &Path, target: &str, out: &Path) -> Result<()> {
    if !is_media_ext(target) && target != "gif" {
        return Err(ClementineError::UnsupportedConversion {
            from: path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default(),
            to: target.into(),
        });
    }
    run(&build_ffmpeg_args(&[path.to_path_buf()], "convert", &ToolParams::default(), out))
}

pub fn run_tool(job: &[PathBuf], tool_id: &str, params: &ToolParams, out: &Path) -> Result<()> {
    if tool_id == "videoSnapshots" && params.snapshot_times.len() > 1 {
        let stem = out.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Snapshot".into());
        let parent = out.parent().unwrap_or(Path::new("."));
        for (idx, &t) in params.snapshot_times.iter().enumerate() {
            let mut single = params.clone();
            single.snapshot_times = vec![t];
            let target = if idx == 0 {
                out.to_path_buf()
            } else {
                parent.join(format!("{stem} {}.png", idx + 1))
            };
            run(&build_ffmpeg_args(job, tool_id, &single, &target))?;
        }
        return Ok(());
    }
    run(&build_ffmpeg_args(job, tool_id, params, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(input: &str, op: &str, params: &ToolParams, out: &str) -> Vec<String> {
        build_ffmpeg_args(&[PathBuf::from(input)], op, params, Path::new(out))
    }

    fn head() -> Vec<String> {
        s(&["-y", "-hide_banner", "-loglevel", "error"])
    }

    #[test]
    fn audio_and_video_conversions() {
        let d = ToolParams::default();
        let mut want = head();
        want.extend(s(&["-i", "a.wav", "-vn", "-c:a", "libmp3lame", "-q:a", "2", "a.mp3"]));
        assert_eq!(args("a.wav", "convert", &d, "a.mp3"), want);

        let mut want = head();
        want.extend(s(&[
            "-i", "v.mov", "-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv420p", "-movflags",
            "+faststart", "v.mp4",
        ]));
        assert_eq!(args("v.mov", "convert", &d, "v.mp4"), want);

        let gif = args("v.mp4", "convert", &d, "v.gif");
        assert!(gif.contains(&"fps=12,scale=480:-1:flags=lanczos".to_string()));
        assert!(gif.contains(&"-an".to_string()));
        for t in ["m4a", "flac", "ogg", "opus", "aiff", "wma", "mkv", "webm", "avi", "wmv"] {
            assert!(args("a.mp3", "convert", &d, &format!("a.{t}")).len() > head().len() + 3, "{t}");
        }
    }

    #[test]
    fn simple_tools() {
        let d = ToolParams::default();
        assert!(args("v.mp4", "muteVideo", &d, "o.mp4").join(" ").ends_with("-an -c:v copy o.mp4"));
        assert!(args("a.mp3", "normalizeAudio", &d, "o.mp3").join(" ").contains("-af loudnorm=I=-16"));
        assert!(args("a.mp3", "audioChannels", &d, "o.mp3").join(" ").ends_with("-ac 1 o.mp3"));
        assert!(args("v.mp4", "removeMetadata", &d, "o.mp4").join(" ").ends_with("-map_metadata -1 -c copy o.mp4"));
        assert!(args("v.mp4", "compress", &d, "o.mp4").join(" ").contains("-crf 28"));
        assert!(args("a.mp3", "compress", &d, "o.mp3").join(" ").ends_with("-b:a 96k o.mp3"));
    }

    #[test]
    fn trim_and_speed() {
        let p = ToolParams { start_sec: 1.5, end_sec: Some(4.0), ..Default::default() };
        let mut want = head();
        want.extend(s(&["-i", "v.mp4", "-ss", "1.5", "-to", "4", "o.mp4"]));
        assert_eq!(args("v.mp4", "trimVideo", &p, "o.mp4"), want);
        let mut want = head();
        want.extend(s(&["-i", "a.mp3", "-ss", "1.5", "-to", "4", "o.mp3"]));
        assert_eq!(args("a.mp3", "trimAudio", &p, "o.mp3"), want);

        let trim_sil = ToolParams { start_sec: 0.0, end_sec: Some(5.0), trim_silence: true, ..Default::default() };
        assert!(args("a.mp3", "trimAudio", &trim_sil, "o.mp3").join(" ").contains("silenceremove"));

        let open = ToolParams { start_sec: 2.0, ..Default::default() };
        assert!(!args("a.mp3", "trimAudio", &open, "o.mp3").contains(&"-to".to_string()));

        let speed = args("v.mp4", "changeVideoSpeed", &ToolParams::default(), "o.mp4").join(" ");
        assert!(speed.contains("[0:v]setpts=PTS/2[v];[0:a]atempo=2[a]"));
        let fast = ToolParams { speed_factor: 4.0, ..Default::default() };
        assert!(args("v.mp4", "changeVideoSpeed", &fast, "o.mp4").join(" ").contains("atempo=2.0,atempo=2"));
        assert_eq!(atempo_chain(0.25), "atempo=0.5,atempo=0.5");
    }

    #[test]
    fn join_split_snapshot() {
        let d = ToolParams::default();
        let join = build_ffmpeg_args(
            &[PathBuf::from("a.mp4"), PathBuf::from("b.mp4")],
            "joinVideos",
            &d,
            Path::new("o.mp4"),
        )
        .join(" ");
        assert!(join.contains("-i a.mp4 -i b.mp4"));
        assert!(join.contains("[0:v][0:a][1:v][1:a]concat=n=2:v=1:a=1[v][a]"));

        let split = args("v.mp4", "splitVideo", &d, "clips");
        assert!(split.join(" ").contains("-f segment -segment_time 10"));
        assert!(split.last().unwrap().ends_with("clip_%03d.mp4"));

        let split_custom = ToolParams { split_points: vec![1.25, 2.50], ..Default::default() };
        assert!(args("v.mp4", "splitVideo", &split_custom, "clips").join(" ").contains("-segment_times 1.25,2.50"));

        let snap = args("v.mp4", "videoSnapshots", &d, "o.png").join(" ");
        assert!(snap.contains("-ss 0 -i v.mp4 -frames:v 1 o.png"));

        let red = ToolParams { start_sec: 1.0, end_sec: Some(2.0), ..Default::default() };
        assert!(args("a.mp3", "redactAudio", &red, "o.mp3").join(" ").contains("between(t,1,2)"));
        assert!(args("a.mp3", "audioToVideo", &d, "o.mp4").join(" ").contains("showwaves"));
    }

    #[test]
    fn ext_classes() {
        assert!(is_audio_ext("flac") && is_video_ext("mkv") && is_media_ext("wma"));
        assert!(!is_media_ext("png") && !is_video_ext("gif"));
    }
}
