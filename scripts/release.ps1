param(
    [string]$Version = '',
    [string]$Notes = '',
    [string]$NotesFile = '',
    [switch]$SkipBuild,
    [switch]$SkipTests,
    [switch]$DryRun
)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$env:GIT_PAGER = 'cat'

function Invoke-Native([scriptblock]$Action) {
    # Windows PowerShell 5.1 turns native stderr into terminating errors when
    # ErrorActionPreference is Stop. Keep it non-terminating and check exit codes.
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { & $Action } finally { $ErrorActionPreference = $previous }
}

$root = Split-Path $PSScriptRoot -Parent
$repo = 'ThodProd/ThodTalk'
$env:Path = "$env:USERPROFILE\.cargo\bin;" + [Environment]::GetEnvironmentVariable('Path','User') + ';' + [Environment]::GetEnvironmentVariable('Path','Machine')

Push-Location $root
try {
    $manifest = Get-Content -LiteralPath 'hub/manifest.json' -Raw | ConvertFrom-Json
    if (!$Version) { $Version = $manifest.version }
    if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "Некорректная версия: $Version" }
    if ($manifest.version -ne $Version) {
        throw "hub/manifest.json содержит $($manifest.version), а запрошена $Version. Сначала обновите версию."
    }
    $cargo = [IO.File]::ReadAllText((Join-Path $root 'Cargo.toml'))
    if ($cargo -notmatch '(?m)^version\s*=\s*"([^"]+)"' -or $Matches[1] -ne $Version) {
        throw "Cargo.toml должен содержать version = `"$Version`""
    }
    $tag = "hub-v$Version"

    $branch = (Invoke-Native { git rev-parse --abbrev-ref HEAD } | Out-String).Trim()
    if (!$DryRun -and $branch -ne 'main') { throw "Выпуск делается только из ветки main, сейчас: $branch" }
    $dirty = (Invoke-Native { git status --porcelain } | Out-String).Trim()
    if (!$DryRun -and $dirty) {
        throw "Есть незакоммиченные изменения. Закоммитьте и запушьте их перед выпуском:`n$dirty"
    }
    if (!$DryRun) {
        Invoke-Native { git fetch origin main --quiet }
        if ($LASTEXITCODE) { throw 'Не удалось выполнить git fetch' }
        $behind = (Invoke-Native { git rev-list --count 'HEAD..origin/main' } | Out-String).Trim()
        if ($behind -ne '0') { throw 'Локальная ветка отстаёт от origin/main. Сначала выполните git pull.' }
        Invoke-Native { gh auth status --hostname github.com } | Out-Null
        if ($LASTEXITCODE) { throw 'GitHub CLI не авторизован: выполните gh auth login' }
        $existing = Invoke-Native { gh release list --repo $repo --limit 100 --json tagName --jq '.[].tagName' }
        if ($LASTEXITCODE) { throw 'Не удалось получить список выпусков GitHub' }
        if ($existing -contains $tag) { throw "Выпуск $tag уже опубликован. Опубликованные файлы менять нельзя: увеличьте версию." }
    }

    if (!$Notes -and $NotesFile) {
        $Notes = [IO.File]::ReadAllText((Resolve-Path -LiteralPath $NotesFile).Path)
    }
    if (!$Notes) {
        $Notes = (Invoke-Native { git log -1 --pretty=%B } | Out-String).Trim()
    }
    if (!$Notes) { $Notes = "ThodTalk $Version" }

    if (!$SkipTests) {
        Invoke-Native { cargo test --locked }
        if ($LASTEXITCODE) { throw 'Тесты не прошли' }
    }
    & (Join-Path $PSScriptRoot 'package-release.ps1') -Notes $Notes -SkipBuild:$SkipBuild
    if ($LASTEXITCODE) { throw 'Сборка пакета не прошла' }

    $package = Get-Content -LiteralPath (Join-Path $root "dist/$Version/package.json") -Raw | ConvertFrom-Json
    $assets = @(
        $package.zip,
        $package.descriptor,
        $package.legacyDescriptor,
        $package.checksum,
        $package.verification
    )
    foreach ($asset in $assets) {
        if (!(Test-Path -LiteralPath $asset)) { throw "Нет файла выпуска: $asset" }
    }

    if ($DryRun) {
        Write-Output "DRY RUN: тег $tag, файлы:"
        $assets | ForEach-Object { Write-Output "  $_" }
        return
    }

    $head = (Invoke-Native { git rev-parse HEAD } | Out-String).Trim()
    $localTag = (Invoke-Native { git tag --list $tag } | Out-String).Trim()
    if ($localTag) {
        $target = (Invoke-Native { git rev-list -n 1 $tag } | Out-String).Trim()
        if ($target -ne $head) { throw "Тег $tag уже указывает на другой коммит" }
    } else {
        Invoke-Native { git tag -a $tag -m "ThodTalk $Version" }
        if ($LASTEXITCODE) { throw "Не удалось создать тег $tag" }
    }
    $remoteTag = (Invoke-Native { git ls-remote --tags origin "refs/tags/$tag" } | Out-String).Trim()
    if (!$remoteTag) {
        Invoke-Native { git push origin $tag }
        if ($LASTEXITCODE) { throw "Не удалось отправить тег $tag" }
    }

    $notesPath = Join-Path ([IO.Path]::GetTempPath()) ("thodtalk-release-" + [Guid]::NewGuid().ToString('N') + '.md')
    [IO.File]::WriteAllText($notesPath, $Notes, [Text.UTF8Encoding]::new($false))
    try {
        $arguments = @(
            'release', 'create', $tag,
            '--repo', $repo,
            '--title', "ThodTalk $Version",
            '--notes-file', $notesPath,
            '--verify-tag',
            '--latest'
        ) + $assets
        Invoke-Native { & gh @arguments }
        if ($LASTEXITCODE) { throw 'gh release create завершился ошибкой' }
    } finally {
        Remove-Item -LiteralPath $notesPath -Force -ErrorAction SilentlyContinue
    }

    $names = Invoke-Native { gh release view $tag --repo $repo --json assets --jq '.assets[].name' }
    if ($LASTEXITCODE) { throw 'Не удалось проверить файлы выпуска' }
    foreach ($asset in $assets) {
        $name = Split-Path $asset -Leaf
        if ($names -notcontains $name) { throw "В выпуске нет файла $name" }
    }
    # Do not put double quotes inside the jq expression: Windows PowerShell 5.1
    # strips them when passing the argument to a native command.
    $zipName = Split-Path $package.zip -Leaf
    $expectedDigest = "sha256:$($package.sha256)"
    $digest = ''
    for ($attempt = 0; $attempt -lt 15; $attempt++) {
        $digest = (Invoke-Native { gh api "repos/$repo/releases/tags/$tag" --jq ".assets[] | select(.name == '$zipName') | .digest" } | Out-String).Trim()
        if ($LASTEXITCODE) { throw 'Не удалось проверить SHA-256 файла выпуска' }
        if ($digest -eq $expectedDigest) { break }
        Start-Sleep -Seconds 2
    }
    if ($digest -ne $expectedDigest) {
        throw "SHA-256 загруженного ZIP не совпал: $digest (ожидался $expectedDigest)"
    }

    $exe = (Invoke-Native { git rev-parse --short HEAD } | Out-String).Trim()
    Write-Output "Выпуск опубликован: https://github.com/$repo/releases/tag/$tag"
    Write-Output "Коммит: $exe"
    Write-Output "ZIP SHA-256: $($package.sha256)"
    Write-Output "Проверка обновления из приложения: раздел «Обновления»."
} finally {
    Pop-Location
}
