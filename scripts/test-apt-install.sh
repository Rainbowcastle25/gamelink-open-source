#!/bin/sh
# Run inside a disposable Debian container with both signed repositories mounted.
set -eu

previous=$1
current=$2
agent_version=$3
state=/var/lib/gamelink-agent
proof=$state/apt-proof
unit=gamelink-agent.service
apt_path=gamelink-agent-apt-update.path
apt_service=gamelink-agent-apt-update.service
apt_state=/var/lib/gamelink-agent-apt-update
restart=/run/gamelink-agent-restart
current_deb=/proof/upgrade/pool/main/amd64/gamelink-agent_${agent_version}_amd64.deb
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
wants=/etc/systemd/system/multi-user.target.wants/gamelink-agent.service
runtime_wants=/run/systemd/system/multi-user.target.wants/gamelink-agent.service
case "$*" in
    daemon-reload) ;;
    'is-enabled gamelink-agent-apt-update.path')
        if [ -e /run/gamelink-apt-watching ]; then echo enabled; else echo disabled; exit 1; fi
        ;;
    'enable --now gamelink-agent-apt-update.path') touch /run/gamelink-apt-watching ;;
    '--no-ask-password is-active gamelink-agent-apt-update.path')
        if [ -e /run/gamelink-apt-watching ]; then echo active; else echo inactive; exit 1; fi
        ;;
    'start gamelink-agent-apt-update.service')
        /var/lib/gamelink-agent-apt-update/installer update install-apt
        ;;
    '--no-ask-password is-enabled gamelink-agent.service')
        exec systemctl is-enabled gamelink-agent.service
        ;;
    '--no-ask-password show --property=ActiveState,InvocationID gamelink-agent.service')
        if [ -f /run/gamelink-proof-bad-version ] &&
            [ "$(cat /run/gamelink-proof-bad-version)" = "$(dpkg-query -W -f='${Version}' gamelink-agent)" ]; then
            if [ -e /run/gamelink-running ]; then unlink /run/gamelink-running; fi
        fi
        if [ -e /run/gamelink-running ]; then
            printf 'ActiveState=active\nInvocationID=1234567890abcdef1234567890abcdef\n'
        else
            echo ActiveState=inactive
        fi
        ;;
    'show --property=ActiveState --value gamelink-agent.service')
        if [ -e /run/gamelink-running ]; then echo active; else echo inactive; fi
        ;;
    'is-active --quiet gamelink-agent.service'|'--system --quiet is-active -- gamelink-agent.service')
        test -e /run/gamelink-running
        ;;
    'is-enabled gamelink-agent.service'|'--system is-enabled -- gamelink-agent.service')
        if [ -L /etc/systemd/system/gamelink-agent.service ] &&
            [ "$(readlink /etc/systemd/system/gamelink-agent.service)" = /dev/null ]; then
            echo masked
            exit 1
        elif [ -L "$wants" ]; then
            echo enabled
        elif [ -L "$runtime_wants" ]; then
            echo enabled-runtime
        else
            echo disabled
            exit 1
        fi
        ;;
    'enable gamelink-agent.service')
        install -d "${wants%/*}"
        ln -sf /lib/systemd/system/gamelink-agent.service "$wants"
        ;;
    'enable --runtime gamelink-agent.service')
        install -d "${runtime_wants%/*}"
        ln -sf /lib/systemd/system/gamelink-agent.service "$runtime_wants"
        ;;
    'disable gamelink-agent.service'|'disable --now gamelink-agent.service')
        if [ -L "$wants" ]; then unlink "$wants"; fi
        if [ "$2" = --now ]; then systemctl stop "$3"; fi
        ;;
    'disable --runtime gamelink-agent.service')
        if [ -L "$runtime_wants" ]; then unlink "$runtime_wants"; fi
        ;;
    'mask gamelink-agent.service')
        ln -s /dev/null /etc/systemd/system/gamelink-agent.service
        ;;
    'unmask gamelink-agent.service')
        unlink /etc/systemd/system/gamelink-agent.service
        ;;
    'stop gamelink-agent.service'|'--quiet --system stop gamelink-agent.service')
        if [ -e /run/gamelink-running ]; then
            stat -c %i /usr/bin/gamelink-agent > "$proof/stop-inode"
            unlink /run/gamelink-running
        fi
        ;;
    'start gamelink-agent.service'|'reload-or-restart gamelink-agent.service'|'--quiet --system start gamelink-agent.service')
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
[ "$1" = gamelink-agent.service ] || exit 101
[ ! -e /run/gamelink-deny-service ] || exit 101
if [ -e /run/gamelink-deny-start ]; then
    case "$2" in start|restart) exit 101 ;; esac
fi
exit 0
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
useradd --system --no-create-home gamelink-collision
gpasswd -a gamelink-collision gamelink
reject_install "$account_error"
gpasswd -d gamelink-collision gamelink
# Reject a second name for the same numeric GID, with or without members.
groupadd --non-unique --gid "$(id -g gamelink)" gamelink-alias
gpasswd -a gamelink-collision gamelink-alias
reject_install "$account_error"
gpasswd -d gamelink-collision gamelink-alias
reject_install "$account_error"
groupdel --force gamelink-alias
usermod --gid gamelink gamelink-collision
reject_install "$account_error"
userdel gamelink-collision
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
    test "$(dpkg-query -W -f='${db:Status-Status} ${Version}' gamelink-agent)" = "installed $1"
    test "$(command -v gamelink-agent)" = /usr/bin/gamelink-agent
    test "$(gamelink-agent --version)" = "gamelink-agent $agent_version"
    getent passwd gamelink
    test "$(stat -c '%U:%G:%a' "$state")" = "gamelink:gamelink:700"
    test -f /lib/systemd/system/gamelink-agent.service
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
check_disabled() {
    test "$(systemctl is-enabled "$unit" || :)" = disabled
}

check_install "$previous"
check_state
check_inactive
check_disabled

ln -sfn /proof/upgrade /repo
apt_get update
state_before=$(stat -c '%i:%z' "$state")
# An apt-managed unit can be customized with systemctl edit --full.
cp "/lib/systemd/system/$unit" "/etc/systemd/system/$unit"
printf '\n# Local full override\n' >> "/etc/systemd/system/$unit"
apt_get install -y --only-upgrade gamelink-agent
check_install "$current"
grep -Fx '# Local full override' "/etc/systemd/system/$unit"
unlink "/etc/systemd/system/$unit"
check_inactive
check_disabled
test ! -e "$proof/start-inode"

test "$(stat -c '%i:%z' "$state")" = "$state_before"

apt_get install -y --allow-downgrades "gamelink-agent=$previous"
check_install "$previous"
check_inactive
check_disabled
test "$(stat -c '%i:%z' "$state")" = "$state_before"

runuser -u gamelink -- env GAMELINK_AGENT_DIR="$state/config" gamelink-agent init
systemctl enable "$unit"
systemctl start "$unit"
systemctl is-active --quiet "$unit"
before=$(stat -c %i /usr/bin/gamelink-agent)

touch /run/gamelink-deny-service
if dpkg --unpack "$current_deb" > /tmp/gamelink-policy.log 2>&1; then
    echo "Upgrade unexpectedly succeeded when policy blocked stopping GameLink" >&2
    exit 1
fi
grep -F 'GameLink could not stop the service before upgrading' /tmp/gamelink-policy.log
check_install "$previous"
systemctl is-active --quiet "$unit"
test "$(stat -c %i /usr/bin/gamelink-agent)" = "$before"
test ! -e "$restart"
unlink /run/gamelink-deny-service

reject_unpack() {
    if dpkg --unpack "$1" > /tmp/gamelink-abort.log 2>&1; then
        echo "Expected an unpack conflict" >&2
        exit 1
    fi
    grep -F "trying to overwrite '/etc/debian_version'" /tmp/gamelink-abort.log
}

# Force dpkg's unpack rollback after preinst by adding a file from base-files.
dpkg-deb -R "$current_deb" /tmp/gamelink-abort
install -D /dev/null /tmp/gamelink-abort/etc/debian_version
dpkg-deb --build --root-owner-group /tmp/gamelink-abort /tmp/gamelink-abort.deb
reject_unpack /tmp/gamelink-abort.deb
check_install "$previous"
systemctl is-active --quiet "$unit"
test "$(cat "$proof/stop-inode")" = "$before"
test "$(cat "$proof/start-inode")" = "$before"
test ! -e "$restart"
unlink "$proof/stop-inode"
unlink "$proof/start-inode"

# Policy can deny start and restart while still allowing reload-or-restart.
touch /run/gamelink-deny-start
reject_unpack /tmp/gamelink-abort.deb
check_install "$previous"
check_inactive
test ! -e "$proof/start-inode"
test ! -e "$restart"
unlink /run/gamelink-deny-start
systemctl start "$unit"

dpkg --unpack "$current_deb"
test "$(stat -c '%u:%a' "$restart")" = 0:600
dpkg --configure gamelink-agent
check_install "$current"
check_state
systemctl is-active --quiet "$unit"
after=$(stat -c %i /usr/bin/gamelink-agent)
test "$after" != "$before"
test "$(cat "$proof/stop-inode")" = "$before"
test "$(cat "$proof/start-inode")" = "$after"
test ! -e "$restart"

touch /run/gamelink-deny-service
if dpkg --remove gamelink-agent > /tmp/gamelink-policy.log 2>&1; then
    echo "Removal unexpectedly succeeded when policy blocked stopping GameLink" >&2
    exit 1
fi
grep -F 'GameLink could not stop the service before removing' /tmp/gamelink-policy.log
check_install "$current"
systemctl is-active --quiet "$unit"
test "$(stat -c %i /usr/bin/gamelink-agent)" = "$after"
unlink /run/gamelink-deny-service

systemctl disable "$unit"
apt_get remove -y gamelink-agent
check_inactive
test "$(cat "$proof/stop-inode")" = "$after"
test ! -e /usr/bin/gamelink-agent
check_state
# A package left in config-files state does not own a unit on disk.
test "$(dpkg-query -W -f='${db:Status-Status}' gamelink-agent)" = config-files
test ! -e "/lib/systemd/system/$unit"
for path in "/etc/systemd/system/$unit" /usr/local/bin/gamelink-agent; do
    ln -s /nonexistent "$path"
    reject_install "GameLink manual installation found at $path"
    test ! -e /usr/bin/gamelink-agent
    unlink "$path"
done
apt_get purge -y gamelink-agent
check_inactive
check_state

# A standalone group is a collision even if it has a system GID.
mv "$state" "$state.saved"
userdel gamelink
if getent group gamelink; then groupdel gamelink; fi
groupadd --system gamelink
reject_install "$account_error"
groupdel gamelink
apt_get install -y gamelink-agent
check_install "$current"
check_inactive
check_disabled

# v2 is still unpacked after v3 rolls back. v1's restart request
# cannot restore v2 as part of v3's rollback or a later configure.
apt_get install -y --allow-downgrades "gamelink-agent=$previous"
runuser -u gamelink -- env GAMELINK_AGENT_DIR="$state/config" gamelink-agent init
install -d -o gamelink -g gamelink -m 0700 "$proof"
systemctl enable "$unit"
systemctl start "$unit"
unlink "$proof/start-inode"
dpkg --unpack "$current_deb"
check_inactive
test -f "$restart"
sed -i "s/^Version:.*/Version: $current+rollback-test/" /tmp/gamelink-abort/DEBIAN/control
dpkg-deb --build --root-owner-group /tmp/gamelink-abort /tmp/gamelink-abort-newer.deb
reject_unpack /tmp/gamelink-abort-newer.deb
test "$(dpkg-query -W -f='${db:Status-Status} ${Version}' gamelink-agent)" = "unpacked $current"
check_inactive
test ! -e "$proof/start-inode"
test ! -e "$restart"
dpkg --configure gamelink-agent
check_install "$current"
check_inactive

# Unpacking the same version twice must discard the first restart request
# before the second unpack can roll back to an unconfigured package.
systemctl start "$unit"
unlink "$proof/start-inode"
dpkg --unpack "$current_deb"
check_inactive
test -f "$restart"
reject_unpack /tmp/gamelink-abort.deb
test "$(dpkg-query -W -f='${db:Status-Status} ${Version}' gamelink-agent)" = "unpacked $current"
check_inactive
test ! -e "$proof/start-inode"
test ! -e "$restart"
dpkg --configure gamelink-agent
check_install "$current"
check_inactive

# A matching configure request also obeys policy when restoring the service.
systemctl start "$unit"
unlink "$proof/start-inode"
dpkg --unpack "$current_deb"
check_inactive
test -f "$restart"
touch /run/gamelink-deny-start
dpkg --configure gamelink-agent
check_install "$current"
check_inactive
test ! -e "$proof/start-inode"
test ! -e "$restart"
unlink /run/gamelink-deny-start

# Respect enablement changes made after unpack stopped the running unit.
for change in disable mask runtime; do
    systemctl start "$unit"
    unlink "$proof/start-inode"
    dpkg --unpack "$current_deb"
    check_inactive
    test -f "$restart"
    case "$change" in
        disable) systemctl disable --now "$unit" ;;
        mask) systemctl mask "$unit" ;;
        runtime)
            systemctl disable "$unit"
            systemctl enable --runtime "$unit"
            ;;
    esac
    dpkg --configure gamelink-agent
    check_install "$current"
    check_inactive
    test ! -e "$proof/start-inode"
    test ! -e "$restart"
    case "$change" in
        disable) check_disabled; systemctl enable "$unit" ;;
        mask)
            test "$(systemctl is-enabled "$unit" || :)" = masked
            systemctl unmask "$unit"
            ;;
        runtime)
            test "$(systemctl is-enabled "$unit")" = enabled-runtime
            systemctl disable --runtime "$unit"
            systemctl enable "$unit"
            ;;
    esac
done

# A manually started disabled unit is stopped for unpack and stays stopped.
systemctl disable "$unit"
systemctl start "$unit"
unlink "$proof/start-inode"
dpkg --unpack "$current_deb"
check_inactive
test ! -e "$restart"
dpkg --configure gamelink-agent
check_install "$current"
check_inactive
check_disabled
test ! -e "$proof/start-inode"

# Exercise desktop-triggered apt installation without a live GitHub release.
# Supply the exact stage files produced from the throwaway-signed manifest.
apt_get install -y --allow-downgrades "gamelink-agent=$previous"
check_install "$previous"
test "$(cat /usr/share/gamelink-agent/install-method)" = apt
test "$(stat -c '%u:%a' /usr/share/gamelink-agent/install-method)" = 0:644
test -f "/lib/systemd/system/$apt_path"
test -f "/lib/systemd/system/$apt_service"
test "$(systemctl is-enabled "$apt_path" || :)" = disabled
systemctl enable "$unit"
systemctl start "$unit"
install -d -o root -g gamelink -m 0750 "$apt_state"
install -d -o gamelink -g gamelink -m 0700 "$apt_state/staged"
install -m 0644 /dev/null "$apt_state/swap.lock"
install -m 0755 /usr/bin/gamelink-agent "$apt_state/installer"
agent_status() {
    runuser -u gamelink -- env GAMELINK_AGENT_DIR="$state/config" gamelink-agent status --json
}
agent_status | grep -Fq '"supported":false'
systemctl enable --now "$apt_path"
agent_status | grep -Fq '"supported":true'
check_install "$previous"

request_upgrade() {
    id=$1
    attempt=$2
    runuser -u gamelink -- sh -ec '
        umask 077
        cd "$1/staged"
        cp /proof/release-manifest.json /proof/release-manifest.json.minisig .
        printf "{\"state\":\"staged\",\"attempt\":%s,\"attempt_id\":\"%s\",\"version\":\"%s\",\"updated_at\":%s}\n" \
            "$3" "$2" "$4" "$(date +%s)" > status.json
        printf "{\"attempt_id\":\"%s\",\"version\":\"%s\"}\n" "$2" "$4" > request.next
        mv request.next request
    ' sh "$apt_state" "$id" "$attempt" "$agent_version"
    if [ "$(cat /proc/1/comm)" != systemd ]; then
        systemctl start "$apt_service" || :
    fi
}

wait_for_outcome() {
    id=$1
    phase=$2
    state_name=$3
    record=$apt_state/install-status.json
    count=0
    until [ -f "$record" ] && grep -Fq "\"attempt_id\":\"$id\"" "$record"; do
        count=$((count + 1))
        if [ "$count" -ge 40 ]; then
            journalctl --no-pager -u "$apt_service" || :
            exit 1
        fi
        sleep 1
    done
    grep -F "\"phase\":\"$phase\"" "$record"
    test "$(stat -c '%u:%a' "$record")" = 0:644
    status=$(agent_status)
    printf '%s\n' "$status" | grep -Fq "\"attemptId\":\"$id\""
    printf '%s\n' "$status" | grep -Fq "\"state\":\"$state_name\""
    systemctl is-active --quiet "$unit"
}

request_upgrade 1234567890abcdef1234567890abcdef 1
wait_for_outcome 1234567890abcdef1234567890abcdef committed succeeded
check_install "$current"

apt_get install -y --allow-downgrades "gamelink-agent=$previous"
check_install "$previous"
# Reset only the test fixture's cooldown so the next attempt can run at once.
sed -i 's/"updated_at":[0-9]*/"updated_at":0/' "$apt_state/install-status.json"
printf '%s\n' "$current" > /run/gamelink-proof-bad-version
if [ "$(cat /proc/1/comm)" = systemd ]; then
    cat > /etc/systemd/system/gamelink-agent.service.d/bad-version.conf <<'UNIT'
[Service]
ExecStart=
ExecStart=/bin/sh -c 'if test "$(dpkg-query -W gamelink-agent | cut -f 2)" = "$(cat /run/gamelink-proof-bad-version)"; then exec /bin/sleep 2; fi; exec /usr/bin/gamelink-agent serve'
UNIT
    systemctl daemon-reload
fi
request_upgrade abcdef1234567890abcdef1234567890 2
wait_for_outcome abcdef1234567890abcdef1234567890 rolled-back rolledBack
check_install "$previous"
