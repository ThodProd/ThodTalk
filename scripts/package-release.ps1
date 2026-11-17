param(
    [string]$OutputDirectory = '',
    [string]$Notes = '',
    [string]$NotesFile = '',
    [switch]$SkipBuild
)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$OutputEncoding = [System.Text.UTF8Encoding]::new($false)

function Invoke-Native([scriptblock]$Action) {
    # Windows PowerShell 5.1 turns native stderr into terminating errors when
    # ErrorActionPreference is Stop. Keep it non-terminating and check exit codes.
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { & $Action } finally { $ErrorActionPreference = $previous }
}

function Get-RelativePath([string]$Base, [string]$Path) {
    $baseFull = [IO.Path]::GetFullPath($Base)
    if (!$baseFull.EndsWith([IO.Path]::DirectorySeparatorChar)) { $baseFull += [IO.Path]::DirectorySeparatorChar }
    $pathFull = [IO.Path]::GetFullPath($Path)
    if ($pathFull.StartsWith($baseFull, [StringComparison]::OrdinalIgnoreCase)) {
        return $pathFull.Substring($baseFull.Length)
    }
    return [IO.Path]::GetFileName($Path)
}

function New-UpdateZip([string]$Stage, [string]$Destination) {
    # ZipFile::CreateFromDirectory writes backslashes on .NET Framework. Old
    # ThodTalk updaters reject such names, so create entries with forward slashes.
    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $stageFull = [IO.Path]::GetFullPath($Stage).TrimEnd([IO.Path]::DirectorySeparatorChar)
    $stream = [IO.File]::Open($Destination, [IO.FileMode]::CreateNew, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    try {
        $archive = New-Object System.IO.Compression.ZipArchive($stream, [System.IO.Compression.ZipArchiveMode]::Create, $false)
        try {
            $files = Get-ChildItem -LiteralPath $stageFull -Recurse -File -Force | Sort-Object FullName
            foreach ($file in $files) {
                $relative = $file.FullName.Substring($stageFull.Length + 1).Replace('\', '/')
                [void][System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
                    $archive,
                    $file.FullName,
                    $relative,
                    [System.IO.Compression.CompressionLevel]::Optimal
                )
            }
        } finally {
            $archive.Dispose()
        }
    } finally {
        $stream.Dispose()
    }
}

function Assert-UpdateZip([string]$Path, [string]$Version) {
    # Mirrors the checks in src/updater.rs (and in 0.3.0-0.3.9) so a package
    # that an older client cannot install never gets published.
    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $reserved = @('CON', 'PRN', 'AUX', 'NUL', 'COM1', 'COM2', 'COM3', 'COM4', 'COM5', 'COM6', 'COM7', 'COM8', 'COM9', 'LPT1', 'LPT2', 'LPT3', 'LPT4', 'LPT5', 'LPT6', 'LPT7', 'LPT8', 'LPT9')
    $zip = [System.IO.Compression.ZipFile]::OpenRead($Path)
    try {
        if ($zip.Entries.Count -gt 5000) { throw "Слишком много файлов в ZIP: $($zip.Entries.Count)" }
        $names = @{}
        $total = [uint64]0
        foreach ($entry in $zip.Entries) {
            $name = $entry.FullName
            if ($name.Contains('\') -or $name.Contains(':') -or $name.StartsWith('/')) {
                throw "Некорректный путь в ZIP: $name"
            }
            foreach ($part in $name.Split('/')) {
                if ([string]::IsNullOrEmpty($part)) { continue }
                if ($part -eq '.' -or $part -eq '..' -or $part.EndsWith(' ') -or $part.EndsWith('.')) {
                    throw "Недопустимое имя в ZIP: $name"
                }
                $base = $part.Split('.')[0].ToUpperInvariant()
                if ($reserved -contains $base) { throw "Зарезервированное имя в ZIP: $name" }
            }
            if ($names.ContainsKey($name)) { throw "Повтор пути в ZIP: $name" }
            $names[$name] = $true
            $total += [uint64]$entry.Length
            if ($total -gt 300MB) { throw 'Распакованный ZIP слишком большой' }
        }
        if (!$names.ContainsKey('manifest.json')) { throw 'В ZIP нет manifest.json' }
        if (!$names.ContainsKey('app/thodtalk.exe')) { throw 'В ZIP нет app/thodtalk.exe' }

        $manifestEntry = $zip.GetEntry('manifest.json')
        $reader = New-Object IO.StreamReader($manifestEntry.Open(), [Text.UTF8Encoding]::new($false))
        try {
            $manifest = $reader.ReadToEnd() | ConvertFrom-Json
        } finally {
            $reader.Dispose()
        }
        if ($manifest.id -ne 'thodtalk' -or
            $manifest.schemaVersion -ne 1 -or
            $manifest.hubApi -ne 1 -or
            $manifest.version -ne $Version -or
            $manifest.mode -ne 'standalone' -or
            $manifest.platform -ne 'win-x64' -or
            $manifest.launch.executable -ne 'app/thodtalk.exe') {
            throw 'Некорректный manifest.json в ZIP'
        }
    } finally {
        $zip.Dispose()
    }
}

$root = Split-Path $PSScriptRoot -Parent
$env:Path = "$env:USERPROFILE\.cargo\bin;" + [Environment]::GetEnvironmentVariable('Path','User') + ';' + [Environment]::GetEnvironmentVariable('Path','Machine')
$env:CMAKE_GENERATOR = 'MinGW Makefiles'
$env:CMAKE_POLICY_VERSION_MINIMUM = '3.5'
$env:CFLAGS = '-std=c11'

Push-Location $root
try {
    $manifest = Get-Content -LiteralPath 'hub/manifest.json' -Raw | ConvertFrom-Json
    if ($manifest.id -ne 'thodtalk' -or $manifest.version -notmatch '^\d+\.\d+\.\d+$') {
        throw 'Некорректный hub/manifest.json'
    }
    $version = $manifest.version
    $cargo = [IO.File]::ReadAllText((Join-Path $root 'Cargo.toml'))
    if ($cargo -notmatch '(?m)^version\s*=\s*"([^"]+)"') { throw 'В Cargo.toml нет version' }
    if ($Matches[1] -ne $version) { throw "Версия Cargo.toml ($($Matches[1])) не совпадает с manifest ($version)" }
    $lock = [IO.File]::ReadAllText((Join-Path $root 'Cargo.lock'))
    if ($lock -notmatch '(?ms)^\[\[package\]\]\s*name = "thodtalk"\s*version = "([^"]+)"') {
        throw 'В Cargo.lock нет пакета thodtalk'
    }
    if ($Matches[1] -ne $version) { throw "Версия Cargo.lock ($($Matches[1])) не совпадает с manifest ($version)" }
    if (!$Notes -and $NotesFile) {
        $Notes = [IO.File]::ReadAllText((Resolve-Path -LiteralPath $NotesFile).Path)
    }
    if (!$Notes) { $Notes = "ThodTalk $version" }

    if (!$SkipBuild) {
        Invoke-Native { cargo build --release --locked }
        if ($LASTEXITCODE) { throw 'Сборка завершилась ошибкой' }
    }
    $exe = Join-Path $root 'target/release/thodtalk.exe'
    if (!(Test-Path -LiteralPath $exe)) { throw 'Нет target/release/thodtalk.exe: соберите приложение' }

    $stage = Join-Path $root ('dist/stage-' + $version + '-' + [Guid]::NewGuid().ToString('N'))
    $app = Join-Path $stage 'app'
    New-Item -ItemType Directory -Path $app -Force | Out-Null
    Copy-Item 'hub/manifest.json' $stage
    Copy-Item 'hub/ПРОЧИТАЙТЕ.txt' $stage
    Copy-Item $exe $app
    Copy-Item README.md,LICENSE,NOTICE.md $app
    Copy-Item docs/SECURITY.md $app
    Copy-Item docs/TESTING.md $app
    Copy-Item assets/icon.svg,assets/artwork.svg $app
    $ico = Get-ChildItem target/release/build -Filter thodtalk.ico -Recurse | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (!$ico) { throw 'Иконка не собрана' }
    Copy-Item $ico.FullName (Join-Path $app 'thodtalk.ico')

    # Runtime imports must be Windows system libraries only.
    $objdump = Get-Command objdump -ErrorAction SilentlyContinue
    if ($objdump) {
        $imports = Invoke-Native { & $objdump.Source -p (Join-Path $app 'thodtalk.exe') }
        if ($LASTEXITCODE) { throw 'Не удалось проверить PE' }
        foreach ($line in $imports) {
            if ($line -match 'DLL Name:\s*(\S+)') {
                $dll = $Matches[1]
                if ($dll -notlike 'api-ms-win-*' -and !(Test-Path -LiteralPath (Join-Path $env:WINDIR "System32/$dll"))) {
                    throw "Не включена зависимость: $dll"
                }
            }
        }
    } else {
        Write-Warning 'objdump не найден: проверка DLL-зависимостей пропущена'
    }

    $metadata = Invoke-Native { cargo metadata --locked --offline --format-version 1 } | ConvertFrom-Json
    if ($LASTEXITCODE) { throw 'Не удалось получить список зависимостей' }
    $licenses = Join-Path $app 'licenses'
    New-Item -ItemType Directory -Path $licenses -Force | Out-Null
    $index = @(
        'Лицензии компонентов ThodTalk. Включён полный список закреплённых зависимостей сборки, в том числе инструментов и тестов.',
        ''
    )
    foreach ($package in $metadata.packages | Sort-Object name,version) {
        if ($package.name -eq 'thodtalk') { continue }
        $prefix = "$($package.name)-$($package.version)"
        $index += "$prefix | $($package.license) | $($package.repository)"
        $source = Split-Path $package.manifest_path -Parent
        foreach ($file in Get-ChildItem -LiteralPath $source -File -Recurse | Where-Object {
            $_.Name -match '(?i)(^|[-_.])(license|licence|copying|notice|ofl|ufl)([-_.]|$)' -or
            ($_.Directory.Name -eq 'fonts' -and $_.Extension -eq '.txt')
        }) {
            $relative = Get-RelativePath $source $file.FullName
            $dest = Join-Path (Join-Path $licenses $prefix) $relative
            New-Item -ItemType Directory -Path (Split-Path $dest -Parent) -Force | Out-Null
            Copy-Item -LiteralPath $file.FullName -Destination $dest
        }
    }
    [IO.File]::WriteAllLines((Join-Path $app 'THIRD-PARTY.txt'), $index, [Text.UTF8Encoding]::new($false))

    $sysroot = Invoke-Native { rustc --print sysroot }
    if ($LASTEXITCODE) { throw 'Не удалось определить runtime Rust' }
    $runtimeNotices = Join-Path $licenses 'Rust-and-GNU-runtime'
    New-Item -ItemType Directory -Path $runtimeNotices -Force | Out-Null
    Copy-Item (Join-Path $sysroot 'share/doc/rust/COPYRIGHT-library.html') $runtimeNotices
    Copy-Item (Join-Path $sysroot 'share/doc/rust/licenses') $runtimeNotices -Recurse
    Copy-Item (Join-Path $sysroot 'share/doc/COPYING.RUNTIME'),(Join-Path $sysroot 'share/doc/COPYING3') $runtimeNotices

    $output = if ($OutputDirectory) { [IO.Path]::GetFullPath($OutputDirectory) } else { Join-Path $root "dist/$version" }
    New-Item -ItemType Directory -Path $output -Force | Out-Null
    $name = "thodtalk-$version-win-x64.zip"
    $zip = Join-Path $output $name
    if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip -Force }
    New-UpdateZip $stage $zip
    if (!(Test-Path -LiteralPath $zip)) { throw 'ZIP не создан' }
    Assert-UpdateZip $zip $version

    $hash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
    $size = (Get-Item -LiteralPath $zip).Length
    $tag = "hub-v$version"
    $url = "https://github.com/ThodProd/ThodTalk/releases/download/$tag/$name"
    $publishedAt = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')

    $descriptor = [ordered]@{
        schemaVersion = 1
        id = 'thodtalk'
        version = $version
        platform = 'win-x64'
        tag = $tag
        packageUrl = $url
        sha256 = $hash
        size = $size
        notes = $Notes
        publishedAt = $publishedAt
    }
    $descriptorPath = Join-Path $output 'thodtalk-release.json'
    [IO.File]::WriteAllText($descriptorPath, ($descriptor | ConvertTo-Json -Depth 5 -Compress), [Text.UTF8Encoding]::new($false))

    # Old ThodTalk builds and ThodHUB read this format.
    $legacy = [ordered]@{
        version = $version
        packageUrl = $url
        sha256 = $hash
        size = $size
        notes = $Notes
        platform = 'win-x64'
        hubApi = 1
    }
    $legacyPath = Join-Path $output 'thodhub-release.json'
    [IO.File]::WriteAllText($legacyPath, ($legacy | ConvertTo-Json -Depth 5 -Compress), [Text.UTF8Encoding]::new($false))

    $shaPath = "$zip.sha256"
    [IO.File]::WriteAllText($shaPath, "$hash  $name`n", [Text.UTF8Encoding]::new($false))

    $exeHash = (Get-FileHash -LiteralPath (Join-Path $app 'thodtalk.exe') -Algorithm SHA256).Hash.ToLowerInvariant()
    $verification = @(
        "ThodTalk $version",
        "SHA-256 ZIP: $hash",
        "SHA-256 EXE: $exeHash",
        "Size: $size",
        "Manifest: hub/manifest.json",
        "Tag: $tag",
        ""
    )
    $verificationPath = Join-Path $output 'verification.txt'
    [IO.File]::WriteAllLines($verificationPath, $verification, [Text.UTF8Encoding]::new($false))

    $package = [ordered]@{
        version = $version
        tag = $tag
        zip = $zip
        descriptor = $descriptorPath
        legacyDescriptor = $legacyPath
        checksum = $shaPath
        verification = $verificationPath
        stage = $stage
        sha256 = $hash
        size = $size
    }
    $summaryPath = Join-Path $output 'package.json'
    [IO.File]::WriteAllText($summaryPath, ($package | ConvertTo-Json -Depth 5 -Compress), [Text.UTF8Encoding]::new($false))
    Write-Output "Пакет: $zip"
    Write-Output "SHA-256: $hash"
    Write-Output "Описание: $descriptorPath"
} finally {
    Pop-Location
}

