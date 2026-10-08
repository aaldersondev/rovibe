# Creates the WSL distribution RoVibe runs isolated agents in.
#
# An agent started there sees one Windows folder, its project, and nothing
# else of the PC: drives aren't mounted, Windows programs can't be launched,
# and its user has no way to become root. The app closes the network to that
# user each time it starts an agent, except for the model's API (iptables).
#
# Run once. Downloads Ubuntu (about 350 MB) and Claude Code into it.
$ErrorActionPreference = 'Stop'
$distro = 'rovibe'

$existing = (wsl.exe --list --quiet) -replace "`0", ""
# A distribution made when the app had another name is kept and brought up
# to date: the app uses it as it is.
if ($existing -contains 'essaim') { $distro = 'essaim' }
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
# Codex, for everyone in the distribution: the standalone build its makers
# publish. Their npm package doesn't install whole with Ubuntu's own npm.
if ! command -v /usr/local/bin/codex >/dev/null; then
    build=codex-x86_64-unknown-linux-musl
    curl -fsSL -o /tmp/codex.tgz "https://github.com/openai/codex/releases/latest/download/$build.tar.gz"
    tar -xzf /tmp/codex.tgz -C /tmp
    install -m 755 "/tmp/$build" /usr/local/bin/codex
    rm -f /tmp/codex.tgz "/tmp/$build"
fi
mkdir -p /work /home/agent/.rovibe
chown agent:agent /home/agent/.rovibe

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
echo "codex: $(/usr/local/bin/codex --version)"
'@ -replace "`r", ""

$setup | wsl.exe -d $distro -u root -- bash -s
if ($LASTEXITCODE -ne 0) { throw "La configuration de la distribution a échoué" }

# wsl.conf is only read when the distribution starts.
wsl.exe --terminate $distro
Write-Host "Distribution « $distro » prête. Au premier agent isolé, connecte Claude Code ou Codex à ton compte dans son terminal."
