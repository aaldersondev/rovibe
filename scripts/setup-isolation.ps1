# Creates the WSL distribution Essaim runs isolated agents in.
#
# An agent started there sees one Windows folder, its project, and nothing
# else of the PC: drives aren't mounted, Windows programs can't be launched,
# and its user has no way to become root. The app closes the network to that
# user each time it starts an agent, except for the model's API (iptables).
#
# Run once. Downloads Ubuntu (about 350 MB) and Claude Code into it.
$ErrorActionPreference = 'Stop'
$distro = 'essaim'

$existing = (wsl.exe --list --quiet) -replace "`0", ""
if ($existing -notcontains $distro) {
    wsl.exe --install Ubuntu-24.04 --name $distro --no-launch
    if ($LASTEXITCODE -ne 0) { throw "L'installation de la distribution a échoué" }
}

# Runs as root, from outside: the agent's own user never gets that power.
$setup = @'
set -e
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq curl git ca-certificates iptables python3 >/dev/null

id agent >/dev/null 2>&1 || useradd --create-home --shell /bin/bash agent
gpasswd -d agent sudo >/dev/null 2>&1 || true
passwd -l root >/dev/null

su - agent -c 'command -v ~/.local/bin/claude >/dev/null || curl -fsSL https://claude.ai/install.sh | bash' >/dev/null
mkdir -p /work /home/agent/.essaim
chown agent:agent /home/agent/.essaim

cat > /etc/wsl.conf <<CONF
[automount]
enabled = false
mountFsTab = false

[interop]
enabled = false
appendWindowsPath = false

[user]
default = agent
CONF
echo "claude: $(su - agent -c '~/.local/bin/claude --version')"
'@ -replace "`r", ""

$setup | wsl.exe -d $distro -u root -- bash -s
if ($LASTEXITCODE -ne 0) { throw "La configuration de la distribution a échoué" }

# wsl.conf is only read when the distribution starts.
wsl.exe --terminate $distro
Write-Host "Distribution « $distro » prête. Au premier agent isolé, connecte Claude Code à ton compte dans son terminal."
