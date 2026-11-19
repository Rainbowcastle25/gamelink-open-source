#!/bin/sh
# Run inside a disposable Debian container with both signed repositories mounted.
set -eu

previous=$1
current=$2
agent_version=$3
state=/var/lib/gamelink-agent
proof=$state/apt-proof
unit=gamelink-agent.service
export DEBIAN_FRONTEND=noninteractive
install -m 0644 /proof/apt.gpg /usr/share/keyrings/gamelink.gpg
printf '%s\n' 'deb [signed-by=/usr/share/keyrings/gamelink.gpg] file:/repo stable main' \
    > /etc/apt/sources.list.d/gamelink.list

apt_get() {
    apt-get -o Dir::Etc::sourcelist=/etc/apt/sources.list.d/gamelink.list \
        -o Dir::Etc::sourceparts=- "$@"
}

if [ "$(cat /proc/1/comm)" = systemd ]; then
    # Use the packaged service and record which binary is on disk when
    # systemd stops and starts it.
    install -d /etc/systemd/system/gamelink-agent.service.d
    cat > /etc/systemd/system/gamelink-agent.service.d/apt-proof.conf <<'UNIT'
[Service]
ExecStartPre=/bin/sh -c 'stat -c %%i /usr/bin/gamelink-agent > /var/lib/gamelink-agent/apt-proof/start-inode'
ExecStop=/bin/sh -c 'stat -c %%i /usr/bin/gamelink-agent > /var/lib/gamelink-agent/apt-proof/stop-inode'
UNIT
else
    # Model systemd in the unprivileged Debian container used by CI.
    mkdir -p /run/systemd/system
    cat > /usr/bin/systemctl <<'SYSTEMCTL'
#!/bin/sh
set -eu
proof=/var/lib/gamelink-agent/apt-proof
case "$*" in
    daemon-reload) ;;
    'show --property=ActiveState --value gamelink-agent.service')
        if [ -e /run/gamelink-running ]; then echo active; else echo inactive; fi
        ;;
    'is-active --quiet gamelink-agent.service')
        test -e /run/gamelink-running
        ;;
    'stop gamelink-agent.service')
        if [ -e /run/gamelink-running ]; then
            stat -c %i /usr/bin/gamelink-agent > "$proof/stop-inode"
            if [ -f /run/gamelink-agent-restart ]; then
                test "$(stat -c '%u:%a' /run/gamelink-agent-restart)" = 0:600
            fi
            unlink /run/gamelink-running
        fi
        ;;
    'start gamelink-agent.service')
        stat -c %i /usr/bin/gamelink-agent > "$proof/start-inode"
        touch /run/gamelink-running
        ;;
    *) exit 1 ;;
esac
SYSTEMCTL
    chmod 0755 /usr/bin/systemctl
fi
cat > /usr/sbin/policy-rc.d <<'POLICY'
#!/bin/sh
[ "$1" = gamelink-agent.service ] && exit 0
exit 101
POLICY
chmod 0755 /usr/sbin/policy-rc.d

check_expiration() {
    release=$1/dists/stable/Release
    issued=$(date -u -d "$(sed -n 's/^Date: //p' "$release")" +%s)
    expires=$(date -u -d "$(sed -n 's/^Valid-Until: //p' "$release")" +%s)
    test "$((expires - issued))" -eq "$((60 * 24 * 60 * 60))"
}
check_expiration /proof/first
check_expiration /proof/upgrade
ln -s /proof/first /repo
apt_get update

reject_install() {
    if apt_get install -y gamelink-agent > /tmp/gamelink-install.log 2>&1; then
        echo "GameLink install unexpectedly succeeded. Expected '$1'." >&2
        exit 1
    fi
    if ! grep -F "$1" /tmp/gamelink-install.log; then
        cat /tmp/gamelink-install.log >&2
        exit 1
    fi
}

# Reject each manual artifact before unpacking, including dangling links.
groupadd --system gamelink
useradd --system --gid gamelink --no-create-home --shell /usr/sbin/nologin gamelink
install -d -o root -g gamelink -m 1770 "$state"
install -d -o root -g gamelink -m 0750 "$state/update"
install -d -o gamelink -g gamelink -m 0700 "$proof"
runuser -u gamelink -- sh -ec 'umask 077; echo keep-me > "$1/proof"' sh "$proof"
mkdir -p /usr/local/bin /etc/systemd/system
for path in /usr/local/bin/gamelink-agent /etc/systemd/system/gamelink-agent.service \
    /etc/systemd/system/gamelink-agent-update.path /etc/systemd/system/gamelink-agent-update.service \
    /usr/local/bin/.gamelink-agent-update-helper; do
    ln -s /nonexistent "$path"
    reject_install "GameLink manual installation found at $path"
    test ! -e /usr/bin/gamelink-agent
    test "$(stat -c '%U:%G:%a' "$state")" = root:gamelink:1770
    test "$(stat -c '%U:%G:%a' "$state/update")" = root:gamelink:750
    unlink "$path"
done

# Reject a regular UID, wrong primary group, login shell and privileged group.
account_error='GameLink cannot reuse the existing gamelink account or group.'
system_uid=$(id -u gamelink)
usermod --uid 42424 gamelink
reject_install "$account_error"
usermod --uid "$system_uid" --gid root gamelink
reject_install "$account_error"
usermod --gid gamelink --shell /bin/sh gamelink
reject_install "$account_error"
usermod --shell /usr/sbin/nologin --append --groups sudo gamelink
reject_install "$account_error"
gpasswd -d gamelink sudo
test "$(stat -c '%U:%G:%a' "$state")" = root:gamelink:1770

# Reject a symlink in place of the state directory without touching its target.
mv "$state" "$state.manual"
install -d -m 0755 /tmp/gamelink-state-target
ln -s /tmp/gamelink-state-target "$state"
reject_install 'GameLink state path must be a directory:'
test -L "$state"
test "$(stat -c '%U:%G:%a' /tmp/gamelink-state-target)" = "root:root:755"
unlink "$state"
mv "$state.manual" "$state"
apt_get install -y gamelink-agent

check_install() {
    test "$(dpkg-query -W -f='${Version}' gamelink-agent)" = "$1"
    test "$(command -v gamelink-agent)" = /usr/bin/gamelink-agent
    test "$(gamelink-agent --version)" = "gamelink-agent $agent_version"
    getent passwd gamelink
    test "$(stat -c '%U:%G:%a' "$state")" = "gamelink:gamelink:700"
    test -f /lib/systemd/system/gamelink-agent.service
    test ! -e /etc/systemd/system/multi-user.target.wants/gamelink-agent.service
    test ! -e /lib/systemd/system/gamelink-agent-update.path
}
check_state() {
    test "$(cat "$proof/proof")" = keep-me
    test "$(stat -c '%U:%G:%a' "$proof/proof")" = "gamelink:gamelink:600"
    test "$(stat -c '%U:%G:%a' "$state/update")" = root:gamelink:750
}
check_inactive() {
    if systemctl is-active --quiet "$unit"; then
        echo "GameLink service is unexpectedly active" >&2
        exit 1
    fi
}

check_install "$previous"
check_state
check_inactive

ln -sfn /proof/upgrade /repo
apt_get update
state_before=$(stat -c '%i:%z' "$state")
apt_get install -y --only-upgrade gamelink-agent
check_install "$current"
check_inactive
test ! -e "$proof/start-inode"
test "$(stat -c '%i:%z' "$state")" = "$state_before"

apt_get install -y --allow-downgrades "gamelink-agent=$previous"
check_install "$previous"
check_inactive
test "$(stat -c '%i:%z' "$state")" = "$state_before"

runuser -u gamelink -- env GAMELINK_AGENT_DIR="$state/config" gamelink-agent init
systemctl start "$unit"
systemctl is-active --quiet "$unit"
before=$(stat -c %i /usr/bin/gamelink-agent)

# Force dpkg's unpack rollback after preinst by adding a file from base-files.
dpkg-deb -R "/proof/upgrade/pool/main/amd64/gamelink-agent_${agent_version}_amd64.deb" /tmp/gamelink-abort
install -D /dev/null /tmp/gamelink-abort/etc/debian_version
dpkg-deb --build --root-owner-group /tmp/gamelink-abort /tmp/gamelink-abort.deb
if dpkg --unpack /tmp/gamelink-abort.deb > /tmp/gamelink-abort.log 2>&1; then
    echo "Expected an unpack conflict" >&2
    exit 1
fi
grep -F "trying to overwrite '/etc/debian_version'" /tmp/gamelink-abort.log
check_install "$previous"
systemctl is-active --quiet "$unit"
test "$(cat "$proof/stop-inode")" = "$before"
test "$(cat "$proof/start-inode")" = "$before"
test ! -e /run/gamelink-agent-restart
unlink "$proof/stop-inode"
unlink "$proof/start-inode"

apt_get install -y --only-upgrade gamelink-agent
check_install "$current"
check_state
systemctl is-active --quiet "$unit"
after=$(stat -c %i /usr/bin/gamelink-agent)
test "$after" != "$before"
test "$(cat "$proof/stop-inode")" = "$before"
test "$(cat "$proof/start-inode")" = "$after"
test ! -e /run/gamelink-agent-restart

apt_get remove -y gamelink-agent
check_inactive
test "$(cat "$proof/stop-inode")" = "$after"
test ! -e /usr/bin/gamelink-agent
check_state
apt_get purge -y gamelink-agent
check_inactive
check_state

# Check that a standalone group must also be a system group, and that a
# fresh install creates the missing system account and group.
mv "$state" "$state.saved"
userdel gamelink
if getent group gamelink; then groupdel gamelink; fi
groupadd --gid 42424 gamelink
reject_install "$account_error"
groupdel gamelink
apt_get install -y gamelink-agent
check_install "$current"
check_inactive
