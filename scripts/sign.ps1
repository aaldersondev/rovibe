# Signs one file with an Authenticode certificate, so Windows shows a
# publisher instead of "Unknown publisher" and SmartScreen builds a
# reputation for it. Usage: .\scripts\sign.ps1 path\to\file.exe
#
# release.ps1 calls it for the app, the sync server and the installer when
# one of these is set; without them a release is simply left unsigned.
#
#   ESSAIM_SIGN_THUMBPRINT     SHA-1 thumbprint of a certificate in the Windows
#                              store. The usual case: since 2023 certificate
#                              authorities deliver keys on a USB token or in a
#                              cloud HSM, both of which show up there.
#   ESSAIM_SIGN_PFX            Path of a .pfx file, for a certificate that is
#   ESSAIM_SIGN_PFX_PASSWORD   still a file. Its password, if it has one.
#
#   ESSAIM_SIGN_TIMESTAMP      Timestamp server, default DigiCert's. A
#                              timestamp keeps the signature valid after the
#                              certificate expires. `none` skips it.
param([Parameter(Mandatory)][string]$File)
$ErrorActionPreference = 'Stop'

$thumbprint = $env:ESSAIM_SIGN_THUMBPRINT
$pfx = $env:ESSAIM_SIGN_PFX
if (-not $thumbprint -and -not $pfx) {
    Write-Host "Signature : aucun certificat configuré, $([IO.Path]::GetFileName($File)) reste non signé"
    exit 0
}
if (-not (Test-Path $File)) { throw "Fichier à signer introuvable : $File" }

# signtool ships with the Windows SDK, one copy per SDK version.
$signtool = (Get-Command signtool.exe -ErrorAction SilentlyContinue).Source
if (-not $signtool) {
    $signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue |
        Sort-Object { [version]$_.Directory.Parent.Name } | Select-Object -Last 1 -ExpandProperty FullName
}
if (-not $signtool) { throw "signtool.exe introuvable : installe le SDK Windows (composant « Signing Tools »)" }

$arguments = @('sign', '/fd', 'SHA256')
if ($thumbprint) {
    $arguments += @('/sha1', ($thumbprint -replace '\s', ''))
} else {
    if (-not (Test-Path $pfx)) { throw "Certificat introuvable : $pfx" }
    $arguments += @('/f', $pfx)
    if ($env:ESSAIM_SIGN_PFX_PASSWORD) { $arguments += @('/p', $env:ESSAIM_SIGN_PFX_PASSWORD) }
}
$timestamp = if ($env:ESSAIM_SIGN_TIMESTAMP) { $env:ESSAIM_SIGN_TIMESTAMP } else { 'http://timestamp.digicert.com' }
if ($timestamp -ne 'none') { $arguments += @('/tr', $timestamp, '/td', 'SHA256') }
$arguments += $File

# signtool's own output would echo the password back on a failure.
# Windows PowerShell turns anything a program writes to stderr into an error
# of its own when errors stop the script.
$ErrorActionPreference = 'Continue'
$output = & $signtool @arguments 2>&1
$ErrorActionPreference = 'Stop'
if ($LASTEXITCODE -ne 0) {
    $shown = ($output | Out-String)
    if ($env:ESSAIM_SIGN_PFX_PASSWORD) { $shown = $shown.Replace($env:ESSAIM_SIGN_PFX_PASSWORD, '***') }
    throw "La signature de $File a échoué :`n$shown"
}
Write-Host "Signé : $([IO.Path]::GetFileName($File))"
