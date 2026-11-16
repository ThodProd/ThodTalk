use crate::storage::Store;
use anyhow::{Context, Result, ensure};
pub const MAX_IMAGE: usize = 180_000;
/// Limit for arbitrary files. The transport caps one packet at 1 MiB, so files
/// travel as chunks of `FILE_CHUNK` bytes.
pub const MAX_FILE: usize = 8 * 1024 * 1024;
pub fn prepare(path: &std::path::Path) -> Result<Vec<u8>> {
    ensure!(
        std::fs::metadata(path)?.len() <= 20 * 1024 * 1024,
        "Изображение больше 20 МБ"
    );
    let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
    reader.limits(limits());
    let img = reader.decode()?.thumbnail(1280, 1280).to_rgb8();
    for quality in [82, 65, 45, 25] {
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality)
            .encode_image(&img)?;
        if bytes.len() <= MAX_IMAGE {
            return Ok(bytes);
        }
    }
    anyhow::bail!("Слишком детальное изображение: уменьшите размер до 1280 пикселей")
}
fn limits() -> image::Limits {
    let mut l = image::Limits::default();
    l.max_image_width = Some(8192);
    l.max_image_height = Some(8192);
    l.max_alloc = Some(128 * 1024 * 1024);
    l
}
pub fn remove(store: &Store, id: uuid::Uuid) -> Result<()> {
    let path = store.root.join(format!("image-{id}.bin"));
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}
pub fn save(store: &Store, id: uuid::Uuid, bytes: &[u8]) -> Result<()> {
    ensure!(bytes.len() <= MAX_IMAGE, "Слишком большое изображение");
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Jpeg);
    let mut l = limits();
    l.max_image_width = Some(1280);
    l.max_image_height = Some(1280);
    l.max_alloc = Some(8 * 1024 * 1024);
    reader.limits(l);
    reader.decode()?;
    let key = format!("image-{id}");
    // Never overwrite an existing attachment referenced by a saved message.
    if store.load::<Vec<u8>>(&key)?.is_none() {
        store.save(&key, &bytes)?;
    }
    Ok(())
}

/// Read a file of any extension for sending.
pub fn read_file(path: &std::path::Path) -> Result<(Vec<u8>, String, u64)> {
    let meta = std::fs::metadata(path).with_context(|| "Файл недоступен")?;
    ensure!(meta.is_file(), "Это не файл");
    let size = meta.len();
    ensure!(
        size > 0 && size <= MAX_FILE as u64,
        "Файл должен быть от 1 байта до 8 МБ"
    );
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("Некорректное имя файла")?
        .to_owned();
    ensure!(valid_file_name(&name), "Некорректное имя файла");
    let bytes = std::fs::read(path)?;
    ensure!(bytes.len() <= MAX_FILE, "Файл больше 8 МБ");
    Ok((bytes, name, size))
}

/// A bare file name: no directories, control characters or excessive length.
pub fn valid_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 120
        && !name.chars().any(char::is_control)
        && !name.contains(['/', '\\'])
        && name != "."
        && name != ".."
}

/// Store arbitrary bytes under the same key as images.
pub fn save_bytes(store: &Store, id: uuid::Uuid, bytes: &[u8]) -> Result<()> {
    ensure!(bytes.len() <= MAX_FILE, "Файл больше 8 МБ");
    let key = format!("image-{id}");
    if store.load::<Vec<u8>>(&key)?.is_none() {
        store.save(&key, &bytes)?;
    }
    Ok(())
}

pub fn load_bytes(store: &Store, id: uuid::Uuid) -> Result<Option<Vec<u8>>> {
    store.load(&format!("image-{id}"))
}
