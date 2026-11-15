param([string]$HubCheckout = 'D:\Every\ThodHUB-contract', [string]$OutputDirectory = '')
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$env:Path = "$env:USERPROFILE\.cargo\bin;" + [Environment]::GetEnvironmentVariable('Path','User') + ';' + [Environment]::GetEnvironmentVariable('Path','Machine')
$env:CMAKE_GENERATOR = 'MinGW Makefiles'
$env:CMAKE_POLICY_VERSION_MINIMUM = '3.5'
$env:CFLAGS = '-std=c11'
Push-Location $root
try {
    cargo build --release --locked
    if ($LASTEXITCODE) {throw 'Сборка завершилась ошибкой'}
    $manifest = Get-Content hub/manifest.json -Raw | ConvertFrom-Json
    $stage = Join-Path $root ('dist/stage-' + $manifest.version + '-' + [Guid]::NewGuid().ToString('N'))
    $app = Join-Path $stage 'app'
    New-Item -ItemType Directory -Path $app -Force | Out-Null
    Copy-Item hub/manifest.json $stage
    Copy-Item hub/ПРОЧИТАЙТЕ.txt $stage
    Copy-Item target/release/thodtalk.exe $app
    Copy-Item README.md,LICENSE,NOTICE.md $app
    Copy-Item docs/SECURITY.md $app
    Copy-Item docs/TESTING.md $app
    Copy-Item assets/icon.svg,assets/artwork.svg $app
    $ico = Get-ChildItem target/release/build -Filter thodtalk.ico -Recurse | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (!$ico) {throw 'Иконка не собрана'}
    Copy-Item $ico.FullName (Join-Path $app 'thodtalk.ico')
    Copy-Item $ico.FullName assets/thodtalk.ico
    # All imports must be Windows system libraries; no SDK or developer PATH at runtime.
    $imports = objdump -p (Join-Path $app 'thodtalk.exe')
    if ($LASTEXITCODE) {throw 'Не удалось проверить PE'}
    foreach ($line in $imports) {
        if ($line -match 'DLL Name:\s*(\S+)') {
            $dll = $Matches[1]
            if ($dll -notlike 'api-ms-win-*' -and !(Test-Path -LiteralPath (Join-Path $env:WINDIR "System32/$dll"))) {throw "Не включена зависимость: $dll"}
        }
    }
    $metadata = cargo metadata --locked --offline --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE) {throw 'Не удалось получить список зависимостей'}
    $licenses = Join-Path $app 'licenses'
    New-Item -ItemType Directory -Path $licenses -Force | Out-Null
    $index = @('Лицензии компонентов ThodTalk. Включён полный список закреплённых зависимостей сборки, в том числе инструментов и тестов.','')
    foreach ($package in $metadata.packages | Sort-Object name,version) {
        if ($package.name -eq 'thodtalk') {continue}
        $prefix = "$($package.name)-$($package.version)"
        $index += "$prefix | $($package.license) | $($package.repository)"
        $source = Split-Path $package.manifest_path -Parent
        foreach ($file in Get-ChildItem -LiteralPath $source -File -Recurse | Where-Object { $_.Name -match '(?i)(^|[-_.])(license|licence|copying|notice|ofl|ufl)([-_.]|$)' -or ($_.Directory.Name -eq 'fonts' -and $_.Extension -eq '.txt') }) {
            $relative = [IO.Path]::GetRelativePath($source,$file.FullName)
            $dest = Join-Path (Join-Path $licenses $prefix) $relative
            New-Item -ItemType Directory -Path (Split-Path $dest -Parent) -Force | Out-Null
            Copy-Item -LiteralPath $file.FullName -Destination $dest
        }
    }
    [IO.File]::WriteAllLines((Join-Path $app 'THIRD-PARTY.txt'),$index,[Text.UTF8Encoding]::new($false))
    $sysroot = rustc --print sysroot
    if ($LASTEXITCODE) {throw 'Не удалось определить runtime Rust'}
    $runtimeNotices = Join-Path $licenses 'Rust-and-GNU-runtime'
    New-Item -ItemType Directory -Path $runtimeNotices -Force | Out-Null
    Copy-Item (Join-Path $sysroot 'share/doc/rust/COPYRIGHT-library.html') $runtimeNotices
    Copy-Item (Join-Path $sysroot 'share/doc/rust/licenses') $runtimeNotices -Recurse
    Copy-Item (Join-Path $sysroot 'share/doc/COPYING.RUNTIME'),(Join-Path $sysroot 'share/doc/COPYING3') $runtimeNotices
    $output = if ($OutputDirectory) {$OutputDirectory} else {Join-Path $root ('dist/' + $manifest.version)}
    & (Join-Path $HubCheckout 'scripts/package-module.ps1') -PackageDirectory $stage -Repository 'ThodProd/ThodTalk' -OutputDirectory $output -Notes 'Компактный интерфейс, Enter и изображения в чате, карточки участников и бан на сессию, настройки клавиш и GitHub-обновления. Голос через MLS/QUIC DATAGRAM; проверка живого владельца.'
    if ($LASTEXITCODE) {throw 'Проверка пакета не пройдена'}
    Write-Output "Каталог пакета: $stage"
} finally {Pop-Location}
