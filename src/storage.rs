use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{io::{Cursor, Write}, path::{Path, PathBuf}};
use zeroize::Zeroizing;

#[derive(Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: uuid::Uuid,
    pub name: String,
    pub avatar: Vec<u8>,
    pub microphone_allowed: bool,
    pub network_allowed: bool,
}

impl Default for Profile {
    fn default() -> Self {
        Self { id: uuid::Uuid::new_v4(), name: String::new(), avatar: Vec::new(), microphone_allowed: false, network_allowed: false }
    }
}

impl Profile {
    pub fn validate(&mut self) -> Result<()> {
        self.name = self.name.trim().to_owned();
        ensure!(!self.name.is_empty() && self.name.chars().count() <= 40, "Имя должно содержать от 1 до 40 символов");
        ensure!(!self.name.chars().any(char::is_control), "В имени недопустимы управляющие символы");
        ensure!(self.avatar.len() <= 1024 * 1024, "Слишком большая аватарка");
        Ok(())
    }
}

pub fn avatar_from_file(path: &Path) -> Result<Vec<u8>> {
    ensure!(std::fs::metadata(path)?.len() <= 20 * 1024 * 1024, "Выберите изображение меньше 20 МБ");
    let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let img = reader.decode().context("Не удалось прочитать изображение")?;
    let mut output = Cursor::new(Vec::new());
    img.resize_to_fill(256, 256, image::imageops::FilterType::Lanczos3).write_to(&mut output, image::ImageFormat::Png)?;
    Ok(output.into_inner())
}

pub struct Store { pub root: PathBuf }
impl Store {
    pub fn open(root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }
    pub fn default_path() -> Result<PathBuf> {
        if let Some(path) = std::env::var_os("THODHUB_DATA_DIR") { return Ok(PathBuf::from(path)); }
        Ok(PathBuf::from(std::env::var_os("LOCALAPPDATA").context("Не задан LOCALAPPDATA")?).join("ThodHUB/Data/thodtalk"))
    }
    pub fn save<T: Serialize>(&self, name: &str, value: &T) -> Result<()> {
        ensure!(!name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'), "Некорректное имя хранилища");
        let plaintext = Zeroizing::new(serde_json::to_vec(value)?);
        let encrypted = protect(&plaintext, false)?;
        let mut file = tempfile::NamedTempFile::new_in(&self.root)?;
        file.write_all(&encrypted)?;
        file.as_file().sync_all()?;
        file.persist(self.root.join(format!("{name}.bin"))).map_err(|e| e.error)?;
        Ok(())
    }
    pub fn load<T: serde::de::DeserializeOwned>(&self, name: &str) -> Result<Option<T>> {
        ensure!(!name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'), "Некорректное имя хранилища");
        let path = self.root.join(format!("{name}.bin"));
        if !path.exists() { return Ok(None); }
        ensure!(std::fs::metadata(&path)?.len() <= 64 * 1024 * 1024, "Файл хранилища слишком большой");
        let bytes = std::fs::read(path)?;
        let plaintext = Zeroizing::new(protect(&bytes, true).context("Не удалось расшифровать данные. Профиль Windows должен совпадать")?);
        Ok(Some(serde_json::from_slice(&plaintext)?))
    }
}

#[cfg(windows)]
fn protect(data: &[u8], decrypt: bool) -> Result<Vec<u8>> {
    use windows_sys::Win32::{Foundation::LocalFree, Security::Cryptography::*};
    ensure!(data.len() <= u32::MAX as usize, "Слишком большой блок данных");
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    // DPAPI allocates output with LocalAlloc; free it on both operations.
    unsafe {
        let success = if decrypt {
            CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        } else {
            CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        };
        ensure!(success != 0, "Ошибка Windows DPAPI: {}", std::io::Error::last_os_error());
        let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        if decrypt { std::ptr::write_bytes(output.pbData, 0, output.cbData as usize); }
        LocalFree(output.pbData as _);
        Ok(result)
    }
}

#[cfg(not(windows))]
fn protect(_: &[u8], _: bool) -> Result<Vec<u8>> { anyhow::bail!("Хранилище поддерживает только Windows") }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_validation() {
        let mut p = Profile::default();
        assert!(p.validate().is_err());
        p.name = "  Друг  ".into(); p.validate().unwrap(); assert_eq!(p.name, "Друг");
        p.name = "Друг\nadmin".into(); assert!(p.validate().is_err());
    }
    #[test]
    fn encrypted_roundtrip_and_corruption() {
        let dir = tempfile::tempdir().unwrap(); let store = Store::open(dir.path().to_owned()).unwrap();
        let value = "Секретная переписка друзей".to_owned();
        store.save("history", &value).unwrap();
        let path = dir.path().join("history.bin");
        let mut bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.windows(value.len()).any(|w| w == value.as_bytes()));
        assert_eq!(store.load::<String>("history").unwrap(), Some(value));
        let last = bytes.len() - 1; bytes[last] ^= 1; std::fs::write(path, bytes).unwrap();
        assert!(store.load::<String>("history").is_err());
        assert!(store.save("../escape", &0).is_err());
    }
}
