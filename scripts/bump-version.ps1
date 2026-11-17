param(
    [Parameter(Mandatory=$true)][string]$Version
)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$OutputEncoding = [System.Text.UTF8Encoding]::new($false)

if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw 'Версия должна быть в формате X.Y.Z' }
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
    $cargoPath = Join-Path $root 'Cargo.toml'
    $cargo = [IO.File]::ReadAllText($cargoPath)
    if ($cargo -notmatch '(?m)^version\s*=\s*"([^"]+)"') { throw 'В Cargo.toml нет version' }
    $old = $Matches[1]
    $cargo = [regex]::Replace($cargo, '(?m)^version\s*=\s*"[^"]+"', "version = `"$Version`"")
    [IO.File]::WriteAllText($cargoPath, $cargo, [Text.UTF8Encoding]::new($false))

    $lockPath = Join-Path $root 'Cargo.lock'
    $lock = [IO.File]::ReadAllText($lockPath)
    $pattern = '(?ms)(\[\[package\]\]\s*name = "thodtalk"\s*version = ")[^"]+(")'
    if ($lock -notmatch $pattern) { throw 'В Cargo.lock нет пакета thodtalk' }
    $lock = [regex]::Replace($lock, $pattern, "`${1}$Version`${2}")
    [IO.File]::WriteAllText($lockPath, $lock, [Text.UTF8Encoding]::new($false))

    $manifestPath = Join-Path $root 'hub/manifest.json'
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    $manifest.version = $Version
    [IO.File]::WriteAllText(
        $manifestPath,
        (($manifest | ConvertTo-Json -Depth 5 -Compress) + "`n"),
        [Text.UTF8Encoding]::new($false)
    )

    Write-Output "Версия: $old -> $Version"
    Write-Output 'Обновите README.md, release notes и проверьте: git diff'
} finally {
    Pop-Location
}
