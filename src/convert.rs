use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use pdfium_render::prelude::*;
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    os::windows::{ffi::OsStrExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    thread,
    time::Duration,
};

pub type Result<T> = std::result::Result<T, String>;
pub const IMAGES: &[&str] = &["jpg", "png", "webp", "bmp", "tiff", "gif", "ico"];
pub const AUDIO: &[&str] = &["mp3", "wav", "flac", "aac", "ogg", "m4a", "opus", "wma", "aiff"];
pub const VIDEO: &[&str] = &["mp4", "mkv", "mov", "avi", "webm", "m4v", "wmv", "mpeg", "mpg", "ts"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group { Images, Documents, Media }

#[derive(Clone, Debug)]
pub struct Detected {
    pub group: Group,
    pub format: String,
    pub video: bool,
}

impl Detected {
    pub fn targets(&self) -> &'static [&'static str] {
        match self.group {
            Group::Images => &["jpg", "png", "webp", "bmp", "tiff", "gif", "pdf"],
            Group::Documents => &["jpg", "png", "txt"],
            Group::Media if self.video => &["mp3", "wav", "flac", "aac", "ogg", "mp4", "mkv", "webm", "gif"],
            Group::Media => &["mp3", "wav", "flac", "aac", "ogg"],
        }
    }

    pub fn default_target(&self) -> &'static str {
        match self.group {
            Group::Documents => "jpg",
            Group::Images if self.format == "png" => "jpg",
            Group::Images => "png",
            Group::Media if self.format == "mp3" => "wav",
            Group::Media => "mp3",
        }
    }
}

pub fn detect(path: &Path) -> Result<Detected> {
    if !path.is_file() { return Err("Перетащите файл, а не папку.".into()); }
    let mut header = [0; 1024];
    let len = File::open(path).map_err(|e| e.to_string())?.read(&mut header).map_err(|e| e.to_string())?;
    let header = &header[..len];
    if header.windows(5).any(|part| part == b"%PDF-") {
        return Ok(Detected { group: Group::Documents, format: "pdf".into(), video: false });
    }
    if let Ok(format) = image::guess_format(header) {
        let format = match format {
            ImageFormat::Jpeg => "jpg", ImageFormat::Png => "png", ImageFormat::WebP => "webp",
            ImageFormat::Bmp => "bmp", ImageFormat::Tiff => "tiff", ImageFormat::Gif => "gif",
            ImageFormat::Ico => "ico", _ => return Err("Этот формат изображения пока не поддерживается.".into()),
        };
        return Ok(Detected { group: Group::Images, format: format.into(), video: false });
    }
    let ext = path.extension().unwrap_or_default().to_string_lossy().to_lowercase();
    if AUDIO.contains(&ext.as_str()) || VIDEO.contains(&ext.as_str()) {
        return Ok(Detected { group: Group::Media, video: VIDEO.contains(&ext.as_str()), format: ext });
    }
    Err("Формат не поддерживается. Добавьте изображение, PDF, аудио или видео.".into())
}

pub fn tool_path(name: &str) -> PathBuf {
    // ponytail: tools ship beside the EXE; a portable folder needs no installer.
    #[cfg(test)]
    return Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join(name);
    #[cfg(not(test))]
    std::env::current_exe().unwrap_or_default().parent().unwrap_or(Path::new(".")).join(name)
}

fn cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) { Err("Отменено".into()) } else { Ok(()) }
}

struct Scratch(PathBuf);
impl Scratch {
    fn new(parent: &Path) -> Result<Self> {
        static ID: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = parent.join(format!(".simpleconvert-{}-{}", std::process::id(), ID.fetch_add(1, Ordering::Relaxed)));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("Не удалось сохранить рядом с оригиналом: {e}")),
            }
        }
    }
}
impl Drop for Scratch {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

#[link(name = "kernel32")]
unsafe extern "system" { fn MoveFileW(existing: *const u16, new: *const u16) -> i32; }

fn publish(staged: &Path, parent: &Path, stem: &str, extension: &str) -> Result<PathBuf> {
    let from: Vec<u16> = staged.as_os_str().encode_wide().chain(Some(0)).collect();
    for suffix in 0..100_000 {
        let name = if suffix == 0 { format!("{stem}.{extension}") } else { format!("{stem} ({suffix}).{extension}") };
        let output = parent.join(name);
        let to: Vec<u16> = output.as_os_str().encode_wide().chain(Some(0)).collect();
        // MoveFileW is atomic on this volume and never replaces an existing file.
        if unsafe { MoveFileW(from.as_ptr(), to.as_ptr()) } != 0 { return Ok(output); }
        let err = std::io::Error::last_os_error();
        if !matches!(err.raw_os_error(), Some(80 | 183)) { return Err(format!("Не удалось записать результат: {err}")); }
    }
    Err("Слишком много файлов с одинаковым именем.".into())
}

fn read_image(path: &Path) -> Result<DynamicImage> {
    let mut decoder = ImageReader::open(path).map_err(|e| e.to_string())?
        .with_guessed_format().map_err(|e| e.to_string())?.into_decoder().map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().map_err(|e| e.to_string())?;
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    img.apply_orientation(orientation);
    Ok(img)
}

fn save_image(img: &DynamicImage, output: &Path, target: &str) -> Result<()> {
    if target == "jpg" {
        let mut rgb = image::RgbImage::new(img.width(), img.height());
        for (dst, src) in rgb.pixels_mut().zip(img.to_rgba8().pixels()) {
            let a = u32::from(src[3]);
            for c in 0..3 { dst[c] = ((u32::from(src[c]) * a + 255 * (255 - a) + 127) / 255) as u8; }
        }
        image::codecs::jpeg::JpegEncoder::new_with_quality(File::create(output).map_err(|e| e.to_string())?, 92)
            .encode_image(&DynamicImage::ImageRgb8(rgb)).map_err(|e| e.to_string())
    } else {
        img.save_with_format(output, ImageFormat::from_extension(target).ok_or("Неизвестный формат изображения")?)
            .map_err(|e| e.to_string())
    }
}

fn pdfium() -> Result<Pdfium> {
    Pdfium::bind_to_library(tool_path("pdfium.dll")).map(Pdfium::new)
        .map_err(|e| format!("Не удалось загрузить pdfium.dll. Храните её рядом с программой. {e}"))
}

fn image_to_pdf(img: &DynamicImage, output: &Path) -> Result<()> {
    let pdfium = pdfium()?;
    let mut doc = pdfium.create_new_pdf().map_err(|e| e.to_string())?;
    let width = PdfPoints::new(img.width() as f32 * 0.75);
    let height = PdfPoints::new(img.height() as f32 * 0.75);
    let object = PdfPageImageObject::new_with_size(&doc, img, width, height).map_err(|e| e.to_string())?;
    let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::Custom(width, height)).map_err(|e| e.to_string())?;
    page.objects_mut().add_image_object(object).map_err(|e| e.to_string())?;
    drop(page);
    doc.save_to_file(output).map_err(|e| e.to_string())
}

fn pdf_to_files(path: &Path, target: &str, scratch: &Path, cancel: &AtomicBool, progress: &impl Fn(String)) -> Result<Vec<PathBuf>> {
    let pdfium = pdfium()?;
    let doc = pdfium.load_pdf_from_file(path, None).map_err(|e| format!("Не удалось открыть PDF: он повреждён или защищён паролем. {e}"))?;
    if doc.pages().is_empty() { return Err("В PDF нет страниц.".into()); }
    let mut staged = Vec::new();
    let mut text = String::new();
    for (i, page) in doc.pages().iter().enumerate() {
        cancelled(cancel)?;
        progress(format!("Страница {} из {}", i + 1, doc.pages().len()));
        if target == "txt" {
            if i > 0 { text.push_str("\r\n\r\n"); }
            text.push_str(&page.text().map_err(|e| e.to_string())?.all());
        } else {
            let w = page.width().value * 200.0 / 72.0;
            let h = page.height().value * 200.0 / 72.0;
            if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 || w * h > 80_000_000.0 {
                return Err("Страница PDF слишком велика для экспорта в 200 dpi.".into());
            }
            let img = page.render_with_config(&PdfRenderConfig::new().set_target_width(w.round() as i32)
                .set_target_height(h.round() as i32)).map_err(|e| e.to_string())?.as_image();
            let out = scratch.join(format!("page-{:03}.{target}", i + 1));
            save_image(&img, &out, target)?;
            staged.push(out);
        }
    }
    if target == "txt" {
        if text.trim().is_empty() { return Err("В PDF нет текстового слоя. Для сканов требуется OCR.".into()); }
        let out = scratch.join("text.txt");
        fs::write(&out, text).map_err(|e| e.to_string())?;
        staged.push(out);
    }
    Ok(staged)
}

fn media(path: &Path, target: &str, output: &Path, scratch: &Path, cancel: &AtomicBool) -> Result<()> {
    let log = scratch.join("ffmpeg.log");
    let mut cmd = Command::new(tool_path("ffmpeg.exe"));
    cmd.creation_flags(0x08000000).args(["-hide_banner", "-nostdin", "-v", "error", "-n", "-protocol_whitelist", "file,pipe", "-i"]).arg(path);
    let options: &[&str] = match target {
        "mp3" => &["-map", "0:a:0", "-vn", "-c:a", "libmp3lame", "-q:a", "2"],
        "wav" => &["-map", "0:a:0", "-vn", "-c:a", "pcm_s16le"],
        "flac" => &["-map", "0:a:0", "-vn", "-c:a", "flac"],
        "aac" => &["-map", "0:a:0", "-vn", "-c:a", "aac", "-b:a", "192k"],
        "ogg" => &["-map", "0:a:0", "-vn", "-c:a", "libvorbis", "-q:a", "5"],
        "mp4" | "mkv" => &["-map", "0:v:0", "-map", "0:a:0?", "-c:v", "libx264", "-preset", "fast", "-crf", "22", "-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2", "-pix_fmt", "yuv420p", "-c:a", "aac"],
        "webm" => &["-map", "0:v:0", "-map", "0:a:0?", "-c:v", "libvpx-vp9", "-crf", "32", "-b:v", "0", "-c:a", "libopus"],
        "gif" => &["-map", "0:v:0", "-an", "-vf", "fps=12,scale=640:-1:flags=lanczos,split[a][b];[a]palettegen[p];[b][p]paletteuse"],
        _ => return Err("Неподдерживаемый формат медиа.".into()),
    };
    let mut child = cmd.args(options).arg(output).stdin(Stdio::null()).stdout(Stdio::null())
        .stderr(File::create(&log).map_err(|e| e.to_string())?).spawn()
        .map_err(|e| format!("Не удалось запустить ffmpeg.exe. Храните его рядом с программой. {e}"))?;
    loop {
        if cancel.load(Ordering::Relaxed) { let _ = child.kill(); let _ = child.wait(); return Err("Отменено".into()); }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => {
                let mut file = File::open(&log).map_err(|e| e.to_string())?;
                let _ = file.seek(SeekFrom::End(-8192));
                let mut tail = Vec::new();
                file.read_to_end(&mut tail).map_err(|e| e.to_string())?;
                return Err(format!("Конвертация не удалась. Проверьте, есть ли в видео звуковая дорожка.\n{}", String::from_utf8_lossy(&tail)));
            }
            Ok(None) => thread::sleep(Duration::from_millis(100)),
            Err(e) => { let _ = child.kill(); let _ = child.wait(); return Err(e.to_string()); }
        }
    }
}

pub fn convert(path: &Path, target: &str, cancel: &AtomicBool, progress: impl Fn(String)) -> Result<Vec<PathBuf>> {
    cancelled(cancel)?;
    let detected = detect(path)?;
    if !detected.targets().contains(&target) { return Err("Эта пара форматов не поддерживается.".into()); }
    if detected.format == target { return Err("Выберите формат, отличный от исходного.".into()); }
    let parent = path.parent().ok_or("Не удалось определить папку оригинала")?;
    let scratch = Scratch::new(parent)?;
    let staged = match detected.group {
        Group::Documents => pdf_to_files(path, target, &scratch.0, cancel, &progress)?,
        Group::Images => {
            let img = read_image(path)?;
            let output = scratch.0.join(format!("image.{target}"));
            cancelled(cancel)?;
            if target == "pdf" { image_to_pdf(&img, &output)?; } else { save_image(&img, &output, target)?; }
            vec![output]
        }
        Group::Media => {
            let output = scratch.0.join(format!("media.{target}"));
            media(path, target, &output, &scratch.0, cancel)?;
            vec![output]
        }
    };
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let multiple = staged.len() > 1;
    let mut outputs = Vec::new();
    let result = (|| {
        for (i, file) in staged.iter().enumerate() {
            cancelled(cancel)?;
            let name = if multiple { format!("{stem}_page_{:03}", i + 1) } else { stem.to_string() };
            outputs.push(publish(file, parent, &name, target)?);
        }
        Ok(())
    })();
    if let Err(e) = result {
        for output in outputs { let _ = fs::remove_file(output); }
        return Err(e);
    }
    Ok(outputs)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
