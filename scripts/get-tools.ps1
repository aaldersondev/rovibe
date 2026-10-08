# Fetches the code checkers Essaim runs for agents, from their official
# GitHub releases, into dist\Essaim\tools. Versions are pinned: bump them here.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
$tools = "$root\dist\Essaim\tools"
New-Item -ItemType Directory -Force $tools | Out-Null

$selene = '0.32.0'
$luauLsp = '1.70.1'

$downloads = @(
    @{ Url = "https://github.com/Kampfkarren/selene/releases/download/$selene/selene-$selene-windows.zip"; Zip = 'selene.zip' },
    @{ Url = "https://github.com/JohnnyMorganz/luau-lsp/releases/download/$luauLsp/luau-lsp-win64.zip"; Zip = 'luau-lsp.zip' }
)

foreach ($download in $downloads) {
    $zip = Join-Path $env:TEMP $download.Zip
    Invoke-WebRequest $download.Url -OutFile $zip -UseBasicParsing
    Expand-Archive $zip -DestinationPath $tools -Force
    Remove-Item $zip
}

# Roblox's API as type definitions: without it every `game` is an unknown global.
Invoke-WebRequest "https://raw.githubusercontent.com/JohnnyMorganz/luau-lsp/$luauLsp/scripts/globalTypes.d.luau" `
    -OutFile "$tools\globalTypes.d.luau" -UseBasicParsing

Get-ChildItem $tools | Select-Object Name, Length
