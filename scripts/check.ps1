$ErrorActionPreference = 'Stop'
$env:Path = "$env:USERPROFILE\.cargo\bin;" + [Environment]::GetEnvironmentVariable('Path', 'User') + ';' + [Environment]::GetEnvironmentVariable('Path', 'Machine')
Push-Location (Join-Path $PSScriptRoot '..')
try {
    cargo test --locked
    if ($LASTEXITCODE -ne 0) { throw 'Тесты не прошли' }
    cargo build --locked
    if ($LASTEXITCODE -ne 0) { throw 'Ошибка сборки' }
} finally { Pop-Location }
