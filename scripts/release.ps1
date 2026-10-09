# Builds a signed installer for a version, plus the `latest.json` the app
# reads to find updates. Usage: .\scripts\release.ps1 0.2.0
#
# Needs the signing key created once with:
#   npx tauri signer generate --ci -w $env:USERPROFILE\.tauri\rovibe.key
# Whoever holds that key can ship updates to every installed copy: keep it
# out of the repository.
#
# That key proves an update comes from us; it says nothing to Windows. For
# Windows to show a publisher, set a code-signing certificate as described in
# sign.ps1: the app, the sync server and the installer are then signed too.
param(
    [Parameter(Mandatory)][string]$Version,
    [string]$Notes = "",
    # Extra Tauri config merged in, e.g. app\tauri.test.conf.json.
    [string]$Config = "",
    [string]$Repo = "aaldersondev/rovibe",
    # Builds the installer only: no update signature, hence no key needed and
    # nothing an installed copy would accept as an update. To try the build.
    [switch]$Unsigned
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
# The key that signs updates: given by the environment where the release is
# built by a machine, read from the user's own file otherwise.
$key = "$env:USERPROFILE\.tauri\rovibe.key"
if (-not $Unsigned -and -not $env:TAURI_SIGNING_PRIVATE_KEY) {
    if (-not (Test-Path $key)) { throw "Clé de signature introuvable : $key" }
    $env:TAURI_SIGNING_PRIVATE_KEY = Get-Content $key -Raw
}
# A running RoVibe is no obstacle: its server runs from a copy of its own,
# and nothing here writes over the window's program.

# The UI is embedded in the binary and the sync server ships beside it.
Push-Location "$root\ui"; npm install --no-fund --no-audit; npm run build; Pop-Location
# The bundler itself, on a machine that never built the app.
if (-not (Test-Path "$root\node_modules\@tauri-apps\cli")) { npm install --prefix $root --no-fund --no-audit }
cargo build --release --manifest-path "$root\vendor\sync\Cargo.toml"
New-Item -ItemType Directory -Force "$root\dist\RoVibe" | Out-Null
Copy-Item "$root\vendor\sync\target\release\rojo.exe" "$root\dist\RoVibe\rovibe-sync.exe" -Force
if (-not (Test-Path "$root\dist\RoVibe\tools\selene.exe")) { & "$PSScriptRoot\get-tools.ps1" }

# One version number, in the two places that carry it.
$conf = "$root\app\tauri.conf.json"
# Written without a byte-order mark: Tauri's and Cargo's parsers reject one.
$plain = New-Object System.Text.UTF8Encoding $false
[IO.File]::WriteAllText($conf, ((Get-Content $conf -Raw) -replace '"version": "[^"]+"', "`"version`": `"$Version`""), $plain)
foreach ($cargo in "$root\app\Cargo.toml", "$root\crates\core\Cargo.toml") {
    [IO.File]::WriteAllText($cargo, ((Get-Content $cargo -Raw) -replace '(?m)^version = "[^"]+"', "version = `"$Version`""), $plain)
}

# The command line ships beside the app; it is the same server, windowless.
cargo build --release --manifest-path "$root\Cargo.toml" -p rovibe-core
Copy-Item "$root\target\release\rovibe-cli.exe" "$root\dist\RoVibe\rovibe-cli.exe" -Force

$configs = @()
if ($Config) { $configs += @('--config', $Config) }
if ($Unsigned) {
    $plainConf = Join-Path ([IO.Path]::GetTempPath()) "rovibe-unsigned.conf.json"
    [IO.File]::WriteAllText($plainConf, '{ "bundle": { "createUpdaterArtifacts": false } }', $plain)
    $configs += @('--config', $plainConf)
}

# Authenticode. Tauri runs the command on the app, then on the installer,
# before it computes the update signature of the latter.
$signing = [bool]($env:ROVIBE_SIGN_THUMBPRINT -or $env:ROVIBE_SIGN_PFX)
if ($signing) {
    & "$PSScriptRoot\sign.ps1" "$root\dist\RoVibe\rovibe-sync.exe"
    & "$PSScriptRoot\sign.ps1" "$root\dist\RoVibe\rovibe-cli.exe"
    $command = @{ cmd = 'powershell'; args = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "$PSScriptRoot\sign.ps1", '%1') }
    $signConf = Join-Path ([IO.Path]::GetTempPath()) "rovibe-sign.conf.json"
    [IO.File]::WriteAllText($signConf, (@{ bundle = @{ windows = @{ signCommand = $command } } } | ConvertTo-Json -Depth 6), $plain)
    $configs += @('--config', $signConf)
}

if (-not $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD) { $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = "" }
Push-Location "$root\app"
npx --prefix $root tauri build @configs
if ($LASTEXITCODE -ne 0) { Pop-Location; throw "tauri build a échoué" }
Pop-Location

$bundle = "$root\target\release\bundle\nsis"
$installer = Get-ChildItem $bundle -Filter "RoVibe_${Version}_x64-setup.exe" | Select-Object -First 1
$out = "$root\dist\release\$Version"
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item $installer.FullName $out -Force

if ($Unsigned) {
    Write-Host "Installeur construit dans $out, sans signature de mise à jour : à essayer, pas à publier."
    exit 0
}

$feed = [ordered]@{
    version   = $Version
    notes     = $Notes
    pub_date  = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    platforms = @{
        "windows-x86_64" = @{
            # Read as a plain string: Get-Content attaches properties that
            # ConvertTo-Json would serialize along with the text.
            signature = [IO.File]::ReadAllText("$($installer.FullName).sig").Trim()
            url       = "https://github.com/$Repo/releases/download/v$Version/$($installer.Name)"
        }
    }
} | ConvertTo-Json -Depth 5
[IO.File]::WriteAllText("$out\latest.json", $feed, $plain)

Write-Host "Installeur et latest.json prêts dans $out"
$signature = Get-AuthenticodeSignature "$out\$($installer.Name)"
if ($signature.SignerCertificate) {
    Write-Host "Signature Authenticode : $($signature.SignerCertificate.Subject) ($($signature.Status))"
} else {
    Write-Host "Installeur non signé (Authenticode) : Windows affichera « Éditeur inconnu ». Voir scripts\sign.ps1."
}
Write-Host "Pour publier : gh release create v$Version `"$out\$($installer.Name)`" `"$out\latest.json`" --repo $Repo --title `"RoVibe $Version`""
