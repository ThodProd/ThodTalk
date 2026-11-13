$ErrorActionPreference = 'Stop'
$env:CMAKE_GENERATOR = 'MinGW Makefiles'
$env:CMAKE_POLICY_VERSION_MINIMUM = '3.5'
$env:CFLAGS = '-std=c11'
$env:Path = "$env:USERPROFILE\.cargo\bin;" + [Environment]::GetEnvironmentVariable('Path', 'User') + ';' + [Environment]::GetEnvironmentVariable('Path', 'Machine')
Push-Location (Join-Path $PSScriptRoot '..')
try {
    cargo test --locked
    if ($LASTEXITCODE -ne 0) { throw 'Тесты не прошли' }
    cargo build --locked
    if ($LASTEXITCODE -ne 0) { throw 'Ошибка сборки' }
} finally { Pop-Location }
