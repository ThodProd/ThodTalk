use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};
const REPO: &str = "https://github.com/ThodProd/ThodTalk/releases/download/";
const MAX_ZIP: u64 = 100 * 1024 * 1024;
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}
#[derive(Deserialize)]
struct Descriptor {
    version: String,
    #[serde(rename = "packageUrl")]
    url: String,
    sha256: String,
    size: u64,
    platform: String,
    #[serde(rename = "hubApi")]
    hub_api: u32,
}
#[derive(Serialize, Deserialize)]
pub struct Prepared {
    pub version: String,
    stage: PathBuf,
    target: PathBuf,
    archive: PathBuf,
    hash: String,
}
pub fn background_prepare(root: &Path) -> Result<Option<Prepared>> {
    use std::process::{Command, Stdio};
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("--update-worker")
        .env("THODHUB_DATA_DIR", root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn()?;
    let _lifetime = child.stdin.take();
    let output = child.wait_with_output()?;
    ensure!(
        output.status.success(),
        "Процесс обновления завершился с ошибкой"
    );
    let result: std::result::Result<Option<Prepared>, String> =
        serde_json::from_slice(&output.stdout)?;
    result.map_err(anyhow::Error::msg)
}
pub fn run_worker() -> Result<()> {
    std::thread::spawn(|| {
        let mut byte = [0];
        let _ = std::io::stdin().read(&mut byte);
        std::process::exit(0);
    });
    let result = crate::storage::Store::default_path()
        .and_then(|root| prepare(&root))
        .map_err(|e| format!("{e:#}"));
    serde_json::to_writer(std::io::stdout(), &result)?;
    std::io::stdout().flush()?;
    Ok(())
}
fn version(v: &str) -> Result<[u32; 3]> {
    let p: Vec<_> = v
        .split('.')
        .map(str::parse::<u32>)
        .collect::<std::result::Result<_, _>>()?;
    ensure!(p.len() == 3, "Некорректная версия");
    Ok([p[0], p[1], p[2]])
}
fn download(client: &reqwest::blocking::Client, url: &str, limit: u64) -> Result<Vec<u8>> {
    ensure!(
        url.starts_with(REPO),
        "Обновления разрешены только из ThodProd/ThodTalk"
    );
    let mut bytes = Vec::new();
    client
        .get(url)
        .send()?
        .error_for_status()?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "Файл обновления слишком большой"
    );
    Ok(bytes)
}
pub fn prepare(root: &Path) -> Result<Option<Prepared>> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("ThodTalk/", env!("CARGO_PKG_VERSION")))
        .https_only(true)
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(15))
        .build()?;
    let response = client
        .get("https://api.github.com/repos/ThodProd/ThodTalk/releases?per_page=10")
        .send()?;
    ensure!(
        response.status() != reqwest::StatusCode::NOT_FOUND,
        "Репозиторий или выпуски ещё не опубликованы"
    );
    let mut bytes = Vec::new();
    response
        .error_for_status()?
        .take(1024 * 1024)
        .read_to_end(&mut bytes)?;
    let releases: Vec<Release> = serde_json::from_slice(&bytes)?;
    let Some(release) = releases
        .into_iter()
        .filter(|r| {
            !r.draft
                && !r.prerelease
                && r.tag_name.starts_with("hub-v")
                && r.assets.iter().any(|a| a.name == "thodhub-release.json")
        })
        .max_by_key(|r| version(r.tag_name.trim_start_matches("hub-v")).unwrap_or([0, 0, 0]))
    else {
        return Ok(None);
    };
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == "thodhub-release.json")
        .unwrap();
    let descriptor: Descriptor =
        serde_json::from_slice(&download(&client, &asset.browser_download_url, 65536)?)?;
    ensure!(
        release.tag_name == format!("hub-v{}", descriptor.version),
        "Версия выпуска не совпала"
    );
    ensure!(
        descriptor.platform == "win-x64" && descriptor.hub_api == 1,
        "Несовместимая платформа обновления"
    );
    if version(&descriptor.version)? <= version(env!("CARGO_PKG_VERSION"))? {
        return Ok(None);
    }
    ensure!(
        descriptor.size > 0 && descriptor.size <= MAX_ZIP,
        "Недопустимый размер обновления"
    );
    ensure!(
        descriptor
            .url
            .starts_with(&format!("{REPO}{}/", release.tag_name)),
        "Пакет из другого выпуска"
    );
    let bytes = download(&client, &descriptor.url, descriptor.size)?;
    ensure!(
        bytes.len() as u64 == descriptor.size,
        "Размер загруженного файла не совпал"
    );
    ensure!(
        format!("{:x}", Sha256::digest(&bytes)) == descriptor.sha256.to_lowercase(),
        "SHA-256 обновления не совпал"
    );
    let parent = root.join("cache/updates");
    fs::create_dir_all(&parent)?;
    let dir = parent.join(uuid::Uuid::new_v4().to_string());
    fs::create_dir(&dir)?;
    let archive = dir.join("package.zip");
    fs::write(&archive, &bytes)?;
    let stage = dir.join("stage");
    fs::create_dir(&stage)?;
    extract(&bytes, &stage, &descriptor.version)?;
    Ok(Some(Prepared {
        version: descriptor.version,
        stage,
        target: std::env::current_exe()?.canonicalize()?,
        archive,
        hash: descriptor.sha256.to_lowercase(),
    }))
}
fn extract(bytes: &[u8], stage: &Path, expected: &str) -> Result<()> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    ensure!(zip.len() <= 5000, "Слишком много файлов обновления");
    let mut names = std::collections::BTreeSet::new();
    let mut total = 0u64;
    for n in 0..zip.len() {
        let f = zip.by_index(n)?;
        let name = f.name().to_string();
        ensure!(
            !name.contains('\\') && !name.contains(':') && !name.starts_with('/'),
            "Некорректный путь обновления"
        );
        let path = f
            .enclosed_name()
            .context("Путь обновления выходит за каталог")?
            .to_owned();
        ensure!(
            name.split('/').filter(|s| !s.is_empty()).all(|s| s != "."
                && s != ".."
                && !s.ends_with([' ', '.'])
                && ![
                    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6",
                    "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7",
                    "LPT8", "LPT9"
                ]
                .contains(&s.split('.').next().unwrap().to_uppercase().as_str())),
            "Недопустимое имя файла"
        );
        ensure!(names.insert(name.to_lowercase()), "Повтор пути обновления");
        ensure!(
            f.unix_mode().is_none_or(|m| m & 0o170000 != 0o120000),
            "Ссылки в обновлении запрещены"
        );
        total = total
            .checked_add(f.size())
            .context("Размер обновления переполнен")?;
        ensure!(
            total <= 300 * 1024 * 1024,
            "Распакованное обновление слишком большое"
        );
        let out = stage.join(path);
        if f.is_dir() {
            fs::create_dir_all(&out)?;
        } else {
            fs::create_dir_all(out.parent().unwrap())?;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(out)?;
            let declared = f.size();
            let copied = std::io::copy(&mut f.take(declared + 1), &mut output)?;
            ensure!(copied == declared, "Размер распакованного файла не совпал");
            output.flush()?;
        }
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(stage.join("manifest.json"))?)?;
    ensure!(
        manifest["id"] == "thodtalk"
            && manifest["schemaVersion"] == 1
            && manifest["hubApi"] == 1
            && manifest["version"] == expected
            && manifest["mode"] == "standalone"
            && manifest["platform"] == "win-x64"
            && manifest["launch"]["executable"] == "app/thodtalk.exe",
        "Некорректный manifest обновления"
    );
    ensure!(
        stage.join("app/thodtalk.exe").is_file(),
        "В обновлении нет приложения"
    );
    Ok(())
}
pub fn install(prepared: &Prepared) -> Result<()> {
    ensure!(
        std::env::var_os("THODHUB_API").is_none(),
        "Приложение запущено хабом: установите эту версию через раздел обновлений ThodHUB"
    );
    let parent = prepared.stage.parent().context("Нет каталога обновления")?;
    let helper = parent.join("update-helper.exe");
    fs::copy(std::env::current_exe()?, &helper)?;
    let plan = parent.join("plan.json");
    fs::write(&plan, serde_json::to_vec(prepared)?)?;
    let mut command = std::process::Command::new(helper);
    command
        .arg("--apply-update")
        .arg(plan)
        .arg(std::process::id().to_string());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command.spawn()?;
    Ok(())
}
pub fn apply(plan: &Path, parent_pid: u32) -> Result<()> {
    let prepared: Prepared = serde_json::from_slice(&fs::read(plan)?)?;
    let bytes = fs::read(&prepared.archive)?;
    ensure!(
        format!("{:x}", Sha256::digest(&bytes)) == prepared.hash,
        "Пакет изменился после загрузки"
    );
    let clean = prepared.stage.parent().unwrap().join("verified");
    fs::create_dir(&clean)?;
    extract(&bytes, &clean, &prepared.version)?;
    #[cfg(windows)]
    unsafe {
        let handle = OpenProcess(0x00100000, 0, parent_pid);
        if !handle.is_null() {
            let result = WaitForSingleObject(handle, 60_000);
            CloseHandle(handle);
            ensure!(result == 0, "Приложение не завершилось за минуту");
        }
    }
    let target_dir = prepared
        .target
        .parent()
        .context("Нет каталога приложения")?;
    let backup = prepared.stage.parent().unwrap().join("backup");
    fs::create_dir(&backup)?;
    let mut changed: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let result = (|| -> Result<()> {
        let mut files = Vec::new();
        collect(&clean.join("app"), Path::new(""), &mut files)?;
        for relative in files {
            let target = target_dir.join(&relative);
            let saved = backup.join(&relative);
            // Refuse symlink/junction parents rather than writing outside the application.
            let mut ancestor = target.parent();
            while let Some(p) = ancestor {
                if p.exists() {
                    ensure!(
                        !fs::symlink_metadata(p)?.file_type().is_symlink(),
                        "Каталог приложения содержит ссылку"
                    );
                }
                if p == target_dir {
                    break;
                }
                ancestor = p.parent();
            }
            fs::create_dir_all(target.parent().unwrap())?;
            let old = if target.exists() {
                ensure!(
                    !fs::symlink_metadata(&target)?.file_type().is_symlink(),
                    "Файл приложения является ссылкой"
                );
                fs::create_dir_all(saved.parent().unwrap())?;
                fs::rename(&target, &saved)?;
                Some(saved)
            } else {
                None
            };
            changed.push((target.clone(), old));
            fs::copy(clean.join("app").join(relative), &target)?;
        }
        // Portable package manifest is a sibling of app/. Do not create it beside a lone EXE.
        if target_dir.file_name().is_some_and(|n| n == "app") {
            let target = target_dir.parent().unwrap().join("manifest.json");
            if target.is_file() {
                let saved = backup.join("root-manifest.json");
                fs::rename(&target, &saved)?;
                changed.push((target.clone(), Some(saved)));
                fs::copy(clean.join("manifest.json"), target)?;
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        for (target, old) in changed.into_iter().rev() {
            if target.exists() {
                let _ = fs::remove_file(&target);
            }
            if let Some(old) = old {
                let _ = fs::rename(old, target);
            }
        }
        let _ = std::process::Command::new(&prepared.target).spawn();
        return Err(e);
    }
    std::process::Command::new(&prepared.target).spawn()?;
    Ok(())
}
fn collect(root: &Path, relative: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(root.join(relative))? {
        let entry = entry?;
        let path = relative.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            collect(root, &path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}
#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
    fn WaitForSingleObject(handle: *mut std::ffi::c_void, milliseconds: u32) -> u32;
    fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn numeric_versions() {
        assert!(version("0.10.0").unwrap() > version("0.9.9").unwrap());
        assert!(version("0.3").is_err());
    }
    #[test]
    fn rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file("../outside", zip::write::FileOptions::default())
            .unwrap();
        zip.write_all(b"bad").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        assert!(extract(&bytes, dir.path(), "0.3.0").is_err());
    }
    #[test]
    fn verifies_manifest_and_rejects_case_collisions() {
        fn package(duplicate: bool) -> Vec<u8> {
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            z.start_file("manifest.json", zip::write::FileOptions::default())
                .unwrap();
            z.write_all(br#"{"schemaVersion":1,"hubApi":1,"id":"thodtalk","version":"0.3.0","platform":"win-x64","mode":"standalone","launch":{"executable":"app/thodtalk.exe"}}"#).unwrap();
            z.start_file("app/thodtalk.exe", zip::write::FileOptions::default())
                .unwrap();
            z.write_all(b"fixture").unwrap();
            if duplicate {
                z.start_file("app/ThodTalk.exe", zip::write::FileOptions::default())
                    .unwrap();
                z.write_all(b"other").unwrap();
            }
            z.finish().unwrap().into_inner()
        }
        let valid = tempfile::tempdir().unwrap();
        assert!(extract(&package(false), valid.path(), "0.3.0").is_ok());
        let mismatch = tempfile::tempdir().unwrap();
        assert!(extract(&package(false), mismatch.path(), "0.3.1").is_err());
        let duplicate = tempfile::tempdir().unwrap();
        assert!(extract(&package(true), duplicate.path(), "0.3.0").is_err());
    }
}
