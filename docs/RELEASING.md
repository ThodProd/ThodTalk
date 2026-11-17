# Выпуск обновлений ThodTalk

Документ описывает полный цикл: версия → сборка → проверка → GitHub Release →
автоматическое обновление у пользователей. Краткие правила для агентов:
`AGENTS.md`.

## 1. Как приложение понимает, что есть обновление

Приложение (`src/updater.rs`):

1. Запускает дочерний процесс `thodtalk.exe --update-worker`
   (при старте через 3 секунды и далее каждые 6 часов).
2. Запрашивает `https://api.github.com/repos/ThodProd/ThodTalk/releases?per_page=30`.
3. Оставляет только стабильные выпуски (`draft=false`, `prerelease=false`),
   тег которого имеет вид `hub-vX.Y.Z` или `vX.Y.Z`, и версия больше текущей.
4. Для самого нового выпуска загружает описание обновления:
   - сначала `thodtalk-release.json`;
   - если его нет — старый `thodhub-release.json`;
   - если нет ни одного — ищет ZIP `thodtalk-X.Y.Z-win-x64.zip` или
     `ThodTalk_vX.Y.Z.zip` и берёт SHA-256 из поля `digest` GitHub API либо из
     asset-файла `*.zip.sha256`.
5. Скачивает ZIP, проверяет размер и SHA-256, безопасно распаковывает и проверяет
   `manifest.json` (`id`, `version`, `platform=win-x64`, `mode=standalone`,
   `launch.executable=app/thodtalk.exe`).
6. Показывает баннер «Доступно обновление X» в боковой панели и в
   «Настройки → Обновления».
7. Если включено «Устанавливать автоматически» и пользователь не в разговоре,
   через 8 секунд запускает установку. Иначе ждёт нажатия кнопки.
8. Helper ждёт завершения приложения, сохраняет старые файлы, заменяет `app/*`
   и `manifest.json`, перезапускает приложение. При ошибке откатывает файлы и
   запускает прежнюю версию.

Профиль, история и ключи находятся в `%LOCALAPPDATA%/ThodHUB/Data/thodtalk`
и обновлением не затрагиваются.

## 2. Обязательные условия выпуска

- Версия `X.Y.Z` одинаковая в:
  - `Cargo.toml` (`[package] version`);
  - `Cargo.lock` (пакет `thodtalk`);
  - `hub/manifest.json`.
- Тег: `hub-vX.Y.Z` (совместимость с ThodHUB и старыми версиями приложения).
- Тег указывает на тот же коммит, который запушен в `origin/main`.
- Релиз не черновик и не prerelease.
- В релизе есть файлы:
  - `thodtalk-X.Y.Z-win-x64.zip`
  - `thodtalk-release.json`
  - `thodhub-release.json`
  - `thodtalk-X.Y.Z-win-x64.zip.sha256`
  - `verification.txt`
- SHA-256 и размер в описаниях совпадают с ZIP.
- ZIP содержит `manifest.json` в корне и `app/thodtalk.exe`.
- Все пути внутри ZIP используют прямые слэши (`app/thodtalk.exe`). Обратные
  слэши старые клиенты отклоняют как «Некорректный путь обновления».
  `scripts/package-release.ps1` проверяет это и manifest до публикации.

Рабочая версия повышается **всегда**. Опубликованный тег и файлы релиза менять
нельзя: GitHub этого не гарантирует, а пользователи уже могли скачать пакет.
Ошибка исправляется новой версией.

## 3. Форматы описания обновления

### 3.1. Новый `thodtalk-release.json`

```json
{
  "schemaVersion": 1,
  "id": "thodtalk",
  "version": "0.3.11",
  "platform": "win-x64",
  "tag": "hub-v0.3.11",
  "packageUrl": "https://github.com/ThodProd/ThodTalk/releases/download/hub-v0.3.11/thodtalk-0.3.11-win-x64.zip",
  "sha256": "…64 hex…",
  "size": 12345678,
  "notes": "Что изменилось",
  "publishedAt": "2026-11-17T12:00:00Z"
}
```

### 3.2. Совместимый `thodhub-release.json`

Нужен старым версиям ThodTalk (в частности 0.3.9 и ниже) и ThodHUB.

```json
{
  "version": "0.3.11",
  "packageUrl": "https://github.com/ThodProd/ThodTalk/releases/download/hub-v0.3.11/thodtalk-0.3.11-win-x64.zip",
  "sha256": "…64 hex…",
  "size": 12345678,
  "notes": "Что изменилось",
  "platform": "win-x64",
  "hubApi": 1
}
```

Оба файла создаёт `scripts/package-release.ps1`. Не редактируйте их вручную:
`packageUrl` обязан указывать на тег этого же выпуска.

## 4. Стандартный выпуск

Из корня репозитория, в PowerShell:

```powershell
.\scripts\bump-version.ps1 -Version 0.3.11
```

Проверьте diff и допишите `README.md` / release notes.

```powershell
.\scripts\check.ps1
git add -A
git commit -m "chore: release 0.3.11"
git push origin main
```

Затем:

```powershell
.\scripts\release.ps1 -Notes "Кратко: что изменилось в 0.3.11"
```

`release.ps1`:

1. проверяет версию и чистоту `main`;
2. проверяет, что локальная ветка не отстаёт от `origin/main`;
3. запускает `cargo test --locked` (можно пропустить `-SkipTests`, но не
   рекомендуется);
4. вызывает `scripts/package-release.ps1`: release-сборка, лицензии, ZIP,
   описания, `.sha256`, `verification.txt`;
5. создаёт тег `hub-vX.Y.Z`, если его нет, и пушит его;
6. публикует GitHub Release со всеми файлами;
7. сверяет список asset-файлов и SHA-256 загруженного ZIP.

Проверка:

```powershell
gh release view hub-v0.3.11 -R ThodProd/ThodTalk
```

### Полезные параметры

```powershell
.\scripts\release.ps1 -DryRun                       # собрать пакет без тега и публикации
.\scripts\release.ps1 -SkipTests                    # только сборка и публикация
.\scripts\release.ps1 -SkipBuild                    # использовать готовый target/release/thodtalk.exe
.\scripts\release.ps1 -NotesFile .\release-notes.md # заметки из файла UTF-8
```

## 5. Ручная публикация (если скрипт недоступен)

Соберите пакет:

```powershell
.\scripts\package-release.ps1 -Notes "Что изменилось"
```

Файлы появятся в `dist/<version>/`. Затем:

```powershell
git tag -a hub-v0.3.11 -m "ThodTalk 0.3.11"
git push origin hub-v0.3.11
gh release create hub-v0.3.11 `
  "dist/0.3.11/thodtalk-0.3.11-win-x64.zip" `
  "dist/0.3.11/thodtalk-release.json" `
  "dist/0.3.11/thodhub-release.json" `
  "dist/0.3.11/thodtalk-0.3.11-win-x64.zip.sha256" `
  "dist/0.3.11/verification.txt" `
  --repo ThodProd/ThodTalk --title "ThodTalk 0.3.11" `
  --notes "Что изменилось" --verify-tag --latest
```

Никогда не загружайте файлы в **существующий** релиз и не пересоздавайте тег.

## 6. Совместимость со старыми версиями

- ThodTalk 0.3.0–0.3.9 ищут в релизе `thodhub-release.json` и тег `hub-v*`.
  Поэтому этот файл обязателен, пока такие версии поддерживаются.
- Новые версии сначала читают `thodtalk-release.json`, поэтому формат можно
  развивать, сохраняя обратную совместимость.
- Если выпуск опубликован без описания, новые версии смогут обновиться по
  `digest` GitHub или `.sha256`, но старые — нет.

## 7. Проверка обновления без публикации

1. Соберите пакет: `.\scripts\package-release.ps1`.
2. Поднимите локальную версию тестовой сборки (например, `0.3.10`).
3. Опубликуйте тестовый выпуск с версией больше текущей.
4. Запустите старую сборку, откройте «Настройки → Обновления» и нажмите
   «Проверить обновления».

Не оставляйте тестовые релизы и теги в официальном репозитории: они попадут
пользователям.

## 8. Диагностика

| Симптом | Причина | Решение |
|---|---|---|
| «Установлена последняя версия» | нет релиза с версией больше текущей | проверьте тег и `Cargo.toml` |
| «Репозиторий или выпуски ещё не опубликованы» | GitHub API недоступен/404 | проверьте `ThodProd/ThodTalk` |
| «GitHub временно ограничил проверку» | rate limit 60 запросов/час на IP | повторите позже |
| «В выпуске нет описания обновления или контрольной суммы» | нет JSON, `digest` и `.sha256` | опубликуйте `thodtalk-release.json` |
| «SHA-256 обновления не совпал» | ZIP изменили после публикации | новый релиз с новой версией |
| «Некорректный manifest обновления» | неверная версия/платформа/путь exe | пересоберите пакет |
| «Приложение запущено хабом» | задан `THODHUB_API` | обновляйте через ThodHUB |
| Обновление скачалось, но не ставится | приложение в разговоре | завершите звонок |

Логи обновления не содержат секретов. Временные файлы лежат в
`%LOCALAPPDATA%/ThodHUB/Data/thodtalk/cache/updates/<uuid>/`:
`package.zip`, `stage/`, `verified/`, `backup/`, `plan.json`,
`update-helper.exe`. Старые каталоги очищаются автоматически (остаются два
последних).

## 9. CI

- `.github/workflows/ci.yml` — тесты и debug-сборка на push/PR.
- `.github/workflows/release.yml` — ручной выпуск по уже существующему тегу
  (`workflow_dispatch`). Используйте, если локальная сборка недоступна.

Локальный `release.ps1` — основной путь. CI и локальный выпуск не должны
публиковать одну и ту же версию.
