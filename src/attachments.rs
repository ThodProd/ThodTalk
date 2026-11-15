use crate::storage::Store;
use anyhow::{Result, ensure};
pub const MAX_IMAGE: usize = 180_000;
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
