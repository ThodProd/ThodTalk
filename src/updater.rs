use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

/// Updates are accepted only from the official repository over HTTPS.
const REPO_SLUG: &str = "ThodProd/ThodTalk";
const DOWNLOAD_PREFIX: &str = "https://github.com/ThodProd/ThodTalk/releases/download/";
const RELEASES_API: &str = "https://api.github.com/repos/ThodProd/ThodTalk/releases?per_page=30";
const RELEASE_FILE: &str = "thodtalk-release.json";
const LEGACY_RELEASE_FILE: &str = "thodhub-release.json";
const MAX_ZIP: u64 = 200 * 1024 * 1024;
const MAX_EXTRACTED: u64 = 400 * 1024 * 1024;
const MAX_FILES: usize = 6000;

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    digest: Option<String>,
}

/// Descriptor published next to the ZIP. Both the current ThodTalk format and
/// the older ThodHUB format are accepted so existing releases stay installable.
#[derive(Deserialize)]
struct Descriptor {
    version: String,
    #[serde(rename = "packageUrl", alias = "url")]
    url: String,
    sha256: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    notes: String,
    #[serde(default = "windows_platform")]
    platform: String,
    #[serde(default = "default_hub_api", rename = "hubApi")]
    hub_api: u32,
}

fn windows_platform() -> String {
    "win-x64".into()
}
fn default_hub_api() -> u32 {
    1
}

#[derive(Serialize, Deserialize)]
pub struct Prepared {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub published_at: String,
    stage: PathBuf,
    target: PathBuf,
    archive: PathBuf,
    hash: String,
}

/// Runs the download in a child process so the UI stays responsive and a
/// network stall cannot block the application.
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
    // Keep stdin open: if the UI process disappears, the worker sees EOF and exits.
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
        loop {
            match std::io::stdin().read(&mut byte) {
                Ok(0) | Err(_) => std::process::exit(0),
                Ok(_) => {}
            }
        }
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
        .trim()
        .split('.')
        .map(str::parse::<u32>)
        .collect::<std::result::Result<_, _>>()?;
    ensure!(p.len() == 3, "Некорректная версия");
    Ok([p[0], p[1], p[2]])
}

/// Accepts `hub-v1.2.3` (ThodHUB compatibility) and `v1.2.3`.
fn tag_version(tag: &str) -> Option<[u32; 3]> {
    let value = tag
        .strip_prefix("hub-v")
        .or_else(|| tag.strip_prefix('v'))?;
    version(value).ok()
}

fn normalize_sha256(value: &str) -> Result<String> {
    let value = value.trim();
    let value = value.strip_prefix("sha256:").unwrap_or(value);
    ensure!(
        value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()),
        "Некорректная контрольная сумма SHA-256"
    );
    Ok(value.to_ascii_lowercase())
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn download(client: &reqwest::blocking::Client, url: &str, limit: u64) -> Result<Vec<u8>> {
    ensure!(
        url.starts_with(DOWNLOAD_PREFIX),
        "Обновления разрешены только из {REPO_SLUG}"
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

fn api_get(client: &reqwest::blocking::Client, url: &str) -> Result<Vec<u8>> {
    let response = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()?;
    ensure!(
        response.status() != reqwest::StatusCode::NOT_FOUND,
        "Репозиторий или выпуски ещё не опубликованы"
    );
    if response.status() == reqwest::StatusCode::FORBIDDEN
        || response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        bail!("GitHub временно ограничил проверку обновлений. Повторите позже");
    }
    let mut bytes = Vec::new();
    response
        .error_for_status()?
        .take(4 * 1024 * 1024)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn choose_release(releases: Vec<Release>, current: [u32; 3]) -> Option<(Release, [u32; 3])> {
    releases
        .into_iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter_map(|r| tag_version(&r.tag_name).map(|v| (r, v)))
        .filter(|(_, v)| *v > current)
        .max_by_key(|(_, v)| *v)
}

fn descriptor_for(
    client: &reqwest::blocking::Client,
    release: &Release,
    tag: [u32; 3],
) -> Result<Descriptor> {
    for name in [RELEASE_FILE, LEGACY_RELEASE_FILE] {
        if let Some(asset) = release
            .assets
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(name))
        {
            let bytes = download(client, &asset.browser_download_url, 256 * 1024)
                .with_context(|| format!("Не удалось загрузить {name}"))?;
            let mut descriptor: Descriptor = serde_json::from_slice(&bytes)
                .with_context(|| format!("Некорректный {name} в выпуске"))?;
            ensure!(
                version(&descriptor.version)? == tag,
                "Версия в {name} не совпала с выпуском"
            );
            ensure!(
                descriptor.platform == "win-x64",
                "Несовместимая платформа обновления"
            );
            ensure!(descriptor.hub_api == 1, "Несовместимый формат обновления");
            descriptor.sha256 = normalize_sha256(&descriptor.sha256)?;
            return Ok(descriptor);
        }
    }
    descriptor_from_assets(client, release, tag)
        .context("В выпуске нет описания обновления или контрольной суммы")
}

/// Fallback for releases published without a descriptor (for example by hand):
/// locate the Windows ZIP, take GitHub's SHA-256 digest or a `.sha256` sidecar.
fn descriptor_from_assets(
    client: &reqwest::blocking::Client,
    release: &Release,
    tag: [u32; 3],
) -> Result<Descriptor> {
    let value = format!("{}.{}.{}", tag[0], tag[1], tag[2]);
    let names = [
        format!("thodtalk-{value}-win-x64.zip"),
        format!("ThodTalk_v{value}.zip"),
    ];
    let asset = release
        .assets
        .iter()
        .find(|a| names.iter().any(|n| a.name.eq_ignore_ascii_case(n)))
        .context("Не найден ZIP для Windows x64")?;
    let hash = if let Some(digest) = &asset.digest {
        normalize_sha256(digest)?
    } else {
        let sidecar = format!("{}.sha256", asset.name);
        let asset = release
            .assets
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(&sidecar))
            .with_context(|| format!("Добавьте {sidecar} или описание обновления"))?;
        let bytes = download(client, &asset.browser_download_url, 4096)?;
        let text = String::from_utf8(bytes)?;
        normalize_sha256(
            text.split_whitespace()
                .next()
                .context("Пустой файл контрольной суммы")?,
        )?
    };
    Ok(Descriptor {
        version: value,
        url: asset.browser_download_url.clone(),
        sha256: hash,
        size: asset.size,
        notes: release.body.clone().unwrap_or_default(),
        platform: "win-x64".into(),
        hub_api: 1,
    })
}

pub fn prepare(root: &Path) -> Result<Option<Prepared>> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!(
            "ThodTalk/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/ThodProd/ThodTalk)"
        ))
        .https_only(true)
        .timeout(Duration::from_secs(180))
        .connect_timeout(Duration::from_secs(15))
        .build()?;
    let current = version(env!("CARGO_PKG_VERSION"))?;
    let releases: Vec<Release> = serde_json::from_slice(&api_get(&client, RELEASES_API)?)?;
    let Some((release, tag)) = choose_release(releases, current) else {
        return Ok(None);
    };
    let mut descriptor = descriptor_for(&client, &release, tag)?;
    ensure!(
        version(&descriptor.version)? == tag,
        "Версия выпуска не совпала"
    );
    ensure!(
        descriptor
            .url
            .starts_with(&format!("{DOWNLOAD_PREFIX}{}/", release.tag_name)),
        "Пакет из другого выпуска"
    );
    ensure!(
        descriptor.size == 0 || descriptor.size <= MAX_ZIP,
        "Недопустимый размер обновления"
    );
    let limit = if descriptor.size > 0 {
        descriptor.size
    } else {
        MAX_ZIP
    };
    let bytes = download(&client, &descriptor.url, limit)?;
    if descriptor.size > 0 {
        ensure!(
            bytes.len() as u64 == descriptor.size,
            "Размер загруженного файла не совпал"
        );
    }
    descriptor.size = bytes.len() as u64;
    ensure!(
        sha256(&bytes) == descriptor.sha256,
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
    cleanup_cache(&parent, &dir);

    let notes = if descriptor.notes.trim().is_empty() {
        release.body.unwrap_or_default()
    } else {
        descriptor.notes
    };
    Ok(Some(Prepared {
        version: descriptor.version,
        notes,
        published_at: release.published_at.unwrap_or_default(),
        stage,
        target: std::env::current_exe()?.canonicalize()?,
        archive,
        hash: descriptor.sha256,
    }))
}

fn cleanup_cache(parent: &Path, keep: &Path) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    let mut dirs: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let path = e.path();
            let modified = fs::metadata(&path).and_then(|m| m.modified()).ok()?;
            Some((modified, path))
        })
        .collect();
    dirs.sort_by_key(|(modified, _)| *modified);
    while dirs.len() > 2 {
        let (_, path) = dirs.remove(0);
        if path != keep {
            let _ = fs::remove_dir_all(path);
        }
    }
}

fn extract(bytes: &[u8], stage: &Path, expected: &str) -> Result<()> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    ensure!(zip.len() <= MAX_FILES, "Слишком много файлов обновления");
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
        ensure!(total <= MAX_EXTRACTED, "Распакованное обновление слишком большое");
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

fn retry<T>(mut operation: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut last = None;
    for attempt in 0..40u64 {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) => {
                last = Some(error);
                std::thread::sleep(Duration::from_millis(100 + attempt * 50));
            }
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("Не удалось заменить файл")))
}

fn replace_file(source: &Path, target: &Path, backup: &Path, relative: &Path) -> Result<Option<PathBuf>> {
    fs::create_dir_all(target.parent().unwrap())?;
    let saved = if target.exists() {
        ensure!(
            !fs::symlink_metadata(target)?.file_type().is_symlink(),
            "Файл приложения является ссылкой"
        );
        let saved = backup.join(relative);
        fs::create_dir_all(saved.parent().unwrap())?;
        // Copy, not rename: the backup cache may be on another drive (portable install).
        retry(|| fs::copy(target, &saved).map(|_| ()))?;
        Some(saved)
    } else {
        None
    };
    let name = target
        .file_name()
        .context("Некорректное имя файла приложения")?
        .to_string_lossy();
    let temporary = target.with_file_name(format!(".{name}.{}.new", std::process::id()));
    let _ = fs::remove_file(&temporary);
    if let Err(error) = retry(|| fs::copy(source, &temporary).map(|_| ())) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    // Rust's rename replaces an existing file on Windows and stays on the same volume.
    if let Err(error) = retry(|| fs::rename(&temporary, target)) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(saved)
}

pub fn apply(plan: &Path, parent_pid: u32) -> Result<()> {
    let prepared: Prepared = serde_json::from_slice(&fs::read(plan)?)?;
    let bytes = fs::read(&prepared.archive)?;
    ensure!(
        sha256(&bytes) == prepared.hash,
        "Пакет изменился после загрузки"
    );
    let work = prepared.stage.parent().context("Нет каталога обновления")?;
    let clean = work.join("verified");
    let backup = work.join("backup");
    let _ = fs::remove_dir_all(&clean);
    let _ = fs::remove_dir_all(&backup);
    fs::create_dir(&clean)?;
    extract(&bytes, &clean, &prepared.version)?;
    #[cfg(windows)]
    unsafe {
        let handle = OpenProcess(0x00100000, 0, parent_pid);
        if !handle.is_null() {
            let result = WaitForSingleObject(handle, 120_000);
            CloseHandle(handle);
            ensure!(result == 0, "Приложение не завершилось за две минуты");
        }
    }
    let target_dir = prepared
        .target
        .parent()
        .context("Нет каталога приложения")?;
    fs::create_dir(&backup)?;
    let mut changed: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    // Installed layout: <root>/manifest.json + <root>/app/thodtalk.exe.
    // Otherwise the executable is treated as self-contained and only it is replaced.
    let installed = target_dir.file_name().is_some_and(|n| n == "app")
        && target_dir.parent().is_some_and(|root| root.join("manifest.json").is_file());
    let result = (|| -> Result<()> {
        if installed {
            let mut files = Vec::new();
            collect(&clean.join("app"), Path::new(""), &mut files)?;
            for relative in files {
                let target = target_dir.join(&relative);
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
                let saved = replace_file(
                    &clean.join("app").join(&relative),
                    &target,
                    &backup,
                    &relative,
                )?;
                changed.push((target, saved));
            }
            let root = target_dir.parent().unwrap();
            let manifest = root.join("manifest.json");
            let source = clean.join("manifest.json");
            if manifest.is_file() && source.is_file() {
                let saved = replace_file(&source, &manifest, &backup, Path::new("root-manifest.json"))?;
                changed.push((manifest, saved));
            }
        } else {
            let source = clean.join("app/thodtalk.exe");
            let saved = replace_file(&source, &prepared.target, &backup, Path::new("thodtalk.exe"))?;
            changed.push((prepared.target.clone(), saved));
        }
        Ok(())
    })();
    if let Err(e) = result {
        rollback(&changed);
        let _ = std::process::Command::new(&prepared.target).spawn();
        return Err(e);
    }
    if let Err(e) = std::process::Command::new(&prepared.target).spawn() {
        rollback(&changed);
        let _ = std::process::Command::new(&prepared.target).spawn();
        return Err(e.into());
    }
    Ok(())
}

fn rollback(changed: &[(PathBuf, Option<PathBuf>)]) {
    for (target, old) in changed.iter().rev() {
        match old {
            Some(old) => {
                if let Some(parent) = target.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let _ = retry(|| fs::copy(old, target).map(|_| ()));
            }
            None => {
                let _ = fs::remove_file(target);
            }
        }
    }
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

    fn release(tag: &str, prerelease: bool, assets: Vec<Asset>) -> Release {
        Release {
            tag_name: tag.into(),
            draft: false,
            prerelease,
            body: Some("Заметки".into()),
            published_at: Some("2026-11-17T00:00:00Z".into()),
            assets,
        }
    }

    fn asset(name: &str, size: u64, digest: Option<&str>) -> Asset {
        Asset {
            name: name.into(),
            browser_download_url: format!("{DOWNLOAD_PREFIX}hub-v0.3.11/{name}"),
            size,
            digest: digest.map(str::to_owned),
        }
    }

    #[test]
    fn numeric_versions() {
        assert!(version("0.10.0").unwrap() > version("0.9.9").unwrap());
        assert!(version("0.3").is_err());
        assert_eq!(tag_version("hub-v0.3.10"), Some([0, 3, 10]));
        assert_eq!(tag_version("v0.3.10"), Some([0, 3, 10]));
        assert_eq!(tag_version("0.3.10"), None);
    }

    #[test]
    fn chooses_highest_stable_newer_release() {
        let releases = vec![
            release("hub-v0.3.9", false, vec![]),
            release("hub-v0.3.11", false, vec![]),
            release("hub-v0.3.12", true, vec![]),
            release("hub-v0.4.0", false, vec![]),
            release("nightly", false, vec![]),
        ];
        let (chosen, tag) = choose_release(releases, [0, 3, 10]).unwrap();
        assert_eq!(chosen.tag_name, "hub-v0.4.0");
        assert_eq!(tag, [0, 4, 0]);
        assert!(choose_release(vec![release("hub-v0.3.10", false, vec![])], [0, 3, 10]).is_none());
    }

    #[test]
    fn normalizes_sha256() {
        assert!(normalize_sha256("sha256:AB").is_err());
        let value = "a".repeat(64);
        assert_eq!(normalize_sha256(&format!("sha256:{}", value.to_uppercase())).unwrap(), value);
    }

    #[test]
    fn fallback_descriptor_uses_asset_digest() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let digest = "b".repeat(64);
        let release = release(
            "hub-v0.3.11",
            false,
            vec![
                asset("thodtalk-0.3.11-win-x64.zip", 12, Some(&format!("sha256:{digest}"))),
                asset("thodtalk-0.3.11-win-x64.zip.sha256", 65, None),
            ],
        );
        let client = reqwest::blocking::Client::new();
        let descriptor = descriptor_from_assets(&client, &release, [0, 3, 11]).unwrap();
        assert_eq!(descriptor.version, "0.3.11");
        assert_eq!(descriptor.sha256, digest);
        assert_eq!(descriptor.size, 12);
        assert_eq!(descriptor.platform, "win-x64");
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
