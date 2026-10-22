use super::*;

// One real end-to-end check: codecs, PDFs, media, cancellation and no overwrites.
#[test]
fn conversion_smoke_check() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(".test-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let scratch = Scratch(root);
    let root = &scratch.0;
    let cancel = AtomicBool::new(false);
    let png = root.join("прозрачное фото.png");
    DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(32, 24, image::Rgba([255, 0, 0, 0]))).save(&png).unwrap();
    let original = fs::read(&png).unwrap();
    let disguised = root.join("картинка без расширения");
    fs::copy(&png, &disguised).unwrap();
    assert_eq!(detect(&disguised).unwrap().format, "png");
    assert_eq!(detect(&png).unwrap().default_target(), "jpg");
    let jobs = crate::plan(vec![png.clone(), png.clone()], "", "").unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].1, "jpg");
    assert!(crate::plan(vec![png.clone()], "pdf", "jpg").is_err());
    assert!(crate::plan(vec![png.clone()], "png", "mp3").is_err());

    let first = convert(&png, "jpg", &cancel, |_| {}).unwrap();
    let jpg = image::open(&first[0]).unwrap().to_rgb8();
    assert_eq!(jpg.dimensions(), (32, 24));
    assert!(jpg.get_pixel(0, 0).0.iter().all(|c| *c > 248), "JPEG transparency must become white");
    let first_bytes = fs::read(&first[0]).unwrap();
    let second = convert(&png, "jpg", &cancel, |_| {}).unwrap();
    assert_ne!(first, second);
    assert_eq!(fs::read(&first[0]).unwrap(), first_bytes);
    assert_eq!(fs::read(&png).unwrap(), original);
    assert!(convert(&png, "png", &cancel, |_| {}).is_err());
    for target in ["webp", "bmp", "tiff", "gif"] {
        let output = convert(&png, target, &cancel, |_| {}).unwrap();
        assert_eq!(image::open(&output[0]).unwrap().width(), 32);
        assert_eq!(output[0].parent(), Some(root.as_path()));
    }
    let pdf = convert(&png, "pdf", &cancel, |_| {}).unwrap();
    let rendered = convert(&pdf[0], "png", &cancel, |_| {}).unwrap();
    assert_eq!(rendered.len(), 1);
    assert!(image::open(&rendered[0]).unwrap().width() > 32);
    assert!(convert(&pdf[0], "txt", &cancel, |_| {}).unwrap_err().contains("OCR"));

    let pdfium = pdfium().unwrap();
    let mut doc = pdfium.create_new_pdf().unwrap();
    let font = doc.fonts_mut().helvetica();
    for _ in 0..2 {
        let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::a4()).unwrap();
        page.objects_mut().create_text_object(PdfPoints::new(40.0), PdfPoints::new(700.0), "Hello from SimpleConvert", font, PdfPoints::new(18.0)).unwrap();
    }
    let multi = root.join("много страниц.pdf");
    doc.save_to_file(&multi).unwrap();
    drop(doc);
    drop(pdfium);
    let pages = convert(&multi, "jpg", &cancel, |_| {}).unwrap();
    assert_eq!(pages.len(), 2);
    assert!(pages[0].file_name().unwrap().to_string_lossy().contains("page_001"));
    let text = convert(&multi, "txt", &cancel, |_| {}).unwrap();
    assert_eq!(fs::read_to_string(&text[0]).unwrap().matches("Hello from SimpleConvert").count(), 2);

    let video = root.join("ролик с музыкой.mp4");
    let status = Command::new(tool_path("ffmpeg.exe")).creation_flags(0x08000000)
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=blue:s=64x64:r=10:d=1", "-f", "lavfi", "-i", "sine=frequency=440:duration=1", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
        .arg(&video).status().unwrap();
    assert!(status.success());
    assert_eq!(detect(&video).unwrap().default_target(), "mp3");
    let jobs = crate::plan(vec![png.clone(), video.clone()], "", "").unwrap();
    assert_eq!(jobs.iter().map(|j| j.1.as_str()).collect::<Vec<_>>(), ["jpg", "mp3"]);
    let mp3 = convert(&jobs[1].0, &jobs[1].1, &cancel, |_| {}).unwrap();
    assert!(fs::metadata(&mp3[0]).unwrap().len() > 1000);
    let status = Command::new(tool_path("ffmpeg.exe")).creation_flags(0x08000000)
        .args(["-v", "error", "-i"]).arg(&mp3[0]).args(["-f", "null", "-"]).status().unwrap();
    assert!(status.success());
    for target in ["wav", "flac", "aac", "ogg", "mkv", "webm", "gif"] {
        let out = convert(&video, target, &cancel, |_| {}).unwrap();
        assert!(fs::metadata(&out[0]).unwrap().len() > 0, "{target}");
    }
    let silent = root.join("без звука.mp4");
    assert!(Command::new(tool_path("ffmpeg.exe")).creation_flags(0x08000000).args(["-v", "error", "-i"]).arg(&video)
        .args(["-an", "-c:v", "copy"]).arg(&silent).status().unwrap().success());
    assert!(convert(&silent, "mp3", &cancel, |_| {}).is_err());
    assert!(!root.join("без звука.mp3").exists());

    cancel.store(true, Ordering::Relaxed);
    assert_eq!(convert(&png, "jpg", &cancel, |_| {}).unwrap_err(), "Отменено");
    assert!(!fs::read_dir(root).unwrap().any(|p| p.unwrap().file_name().to_string_lossy().starts_with(".simpleconvert-")));
    assert_eq!(fs::read(&png).unwrap(), original);
}
