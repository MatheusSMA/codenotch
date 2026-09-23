# Builds the native pill and its hook, then swaps the installed binaries in place.
#
# The swap is a rename, not a copy over the top. Copying deletes the target first, and Claude Code
# fires codenotch-hook.exe several times a second: a hook that lands inside that window finds no
# file and prints "No such file or directory" into the user's chat. A rename on the same volume is
# atomic, so the hook either runs the old binary or the new one.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$dest = Join-Path $env:LOCALAPPDATA 'CodenotchNative'

Push-Location $root
try {
    cargo build --release -p codenotch-native -p codenotch-hook
    if ($LASTEXITCODE -ne 0) { throw "build failed" }
} finally { Pop-Location }

taskkill /IM codenotch-native.exe /F 2>$null | Out-Null
Start-Sleep -Milliseconds 500

foreach ($exe in 'codenotch-native.exe', 'codenotch-hook.exe') {
    $src = Join-Path $root "target\release\$exe"
    $tmp = Join-Path $dest "$exe.new"
    Copy-Item $src $tmp -Force
    Move-Item $tmp (Join-Path $dest $exe) -Force
}

Start-Process -FilePath (Join-Path $dest 'codenotch-native.exe')
Write-Host "published to $dest"
