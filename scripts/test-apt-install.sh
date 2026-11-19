#!/bin/sh
# Run inside a disposable Debian container with both signed repositories mounted.
set -eu

previous=$1
current=$2
agent_version=$3
export DEBIAN_FRONTEND=noninteractive
truncate -s 0 /etc/apt/sources.list.d/debian.sources
install -m 0644 /proof/apt.gpg /usr/share/keyrings/gamelink.gpg
printf '%s\n' 'deb [signed-by=/usr/share/keyrings/gamelink.gpg] file:/repo stable main' \
    > /etc/apt/sources.list.d/gamelink.list
# Exercise the maintainer scripts' systemd branch in a container without
# systemd as PID 1. The stub records calls and models a running unit.
mkdir -p /run/systemd/system
cat > /usr/bin/systemctl <<'SYSTEMCTL'
#!/bin/sh
printf '%s\n' "$*" >> /run/gamelink-systemctl
case "$*" in
    daemon-reload) ;;
    'try-restart gamelink-agent.service')
        if [ -e /run/gamelink-running ]; then
            echo restarted >> /run/gamelink-systemctl
        fi
        ;;
    *) exit 1 ;;
esac
SYSTEMCTL
cat > /usr/sbin/policy-rc.d <<'POLICY'
#!/bin/sh
[ "$1" = gamelink-agent.service ] && exit 0
exit 101
POLICY
chmod 0755 /usr/bin/systemctl /usr/sbin/policy-rc.d

ln -s /proof/first /repo
apt-get update

# Reject a symlink in place of the state directory without touching its target.
install -d -m 0755 /tmp/gamelink-state-target
ln -s /tmp/gamelink-state-target /var/lib/gamelink-agent
if apt-get install -y gamelink-agent > /tmp/gamelink-install.log 2>&1; then
    echo "GameLink accepted a symlink as its state directory" >&2
    exit 1
fi
grep -F 'GameLink state path must be a directory:' /tmp/gamelink-install.log
test -L /var/lib/gamelink-agent
test "$(stat -c '%U:%G:%a' /tmp/gamelink-state-target)" = "root:root:755"
unlink /var/lib/gamelink-agent
apt-get install -y gamelink-agent

check_install() {
    test "$(dpkg-query -W -f='${Version}' gamelink-agent)" = "$1"
    test "$(command -v gamelink-agent)" = /usr/bin/gamelink-agent
    test "$(gamelink-agent --version)" = "gamelink-agent $agent_version"
    getent passwd gamelink
    test "$(stat -c '%U:%G:%a' /var/lib/gamelink-agent)" = "gamelink:gamelink:700"
    test -f /lib/systemd/system/gamelink-agent.service
    test ! -e /etc/systemd/system/multi-user.target.wants/gamelink-agent.service
    test ! -e /lib/systemd/system/gamelink-agent-update.path
    if grep -qsx gamelink-agent /proc/[0-9]*/comm; then
        echo "GameLink started before configuration" >&2
        exit 1
    fi
}
check_install "$previous"
test "$(cat /run/gamelink-systemctl)" = daemon-reload
runuser -u gamelink -- sh -ec 'umask 077; echo keep-me > /var/lib/gamelink-agent/proof'

touch /run/gamelink-running
truncate -s 0 /run/gamelink-systemctl
ln -sfn /proof/upgrade /repo
apt-get update
apt-get upgrade -y
check_install "$current"
test "$(cat /var/lib/gamelink-agent/proof)" = keep-me
test "$(stat -c '%U:%G:%a' /var/lib/gamelink-agent/proof)" = "gamelink:gamelink:600"
test "$(cat /run/gamelink-systemctl)" = 'daemon-reload
try-restart gamelink-agent.service
restarted'
