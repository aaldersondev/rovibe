# Builds everything and assembles a portable app in dist\Essaim.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot

Push-Location "$root\ui"
npm install --no-fund --no-audit
npm run build
Pop-Location

# The UI is embedded into the binary, so it has to be built first.
cargo build --release --manifest-path "$root\vendor\sync\Cargo.toml"
cargo build --release --manifest-path "$root\Cargo.toml" -p essaim

$out = "$root\dist\Essaim"
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item "$root\target\release\essaim.exe" "$out\Essaim.exe" -Force
Copy-Item "$root\vendor\sync\target\release\rojo.exe" "$out\essaim-sync.exe" -Force
Copy-Item "$root\vendor\sync\LICENSE.txt" "$out\essaim-sync-LICENSE.txt" -Force

# The code checkers are downloaded once; they don't change with the app.
if (-not (Test-Path "$out\tools\selene.exe")) {
    & "$PSScriptRoot\get-tools.ps1"
}
Write-Host "App assemblée dans $out"
