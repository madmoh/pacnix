#!/usr/bin/env bash
# Scenario 19: downloads that fail for a time. A mirror that refuses
# connections and a source host that does not resolve do not fail the apply:
# pacnix tries the apply again with one download at a time and continues from
# the downloaded files, and the pacman.conf of the user does not change. A
# mirror that stays down stops pacnix after four attempts in about two
# minutes. A mirror without a file of the sync databases (404) stops pacnix at
# once and tells to run pacnix upgrade. A no-op apply downloads nothing.
source "$(dirname "$0")/../lib.sh"
fresh $h
apply $h >/dev/null || fail "first apply failed"
conf=$(on $h 'md5sum /etc/pacman.conf')

step "the mirror refuses connections for 25 s"
on $h 'sudo cp /etc/pacman.d/mirrorlist /var/tmp/mirrorlist && echo "Server = http://127.0.0.1:9/\$repo/os/\$arch" | sudo tee /etc/pacman.d/mirrorlist >/dev/null &&
	sudo systemd-run --quiet --on-active=25 cp /var/tmp/mirrorlist /etc/pacman.d/mirrorlist'
scenario $h '{ packages.repo = [ "cowsay" ]; }'
out=$(apply $h) || { echo "$out" | tail -20; fail "apply failed while the mirror refused connections"; }
grep -q "^pacnix: attempt 1 of 4 failed in the network: error: failed retrieving file '.*127.0.0.1.*Next attempt in 10 s, with one download at a time$" <<<"$out" || { echo "$out"; fail "apply does not try the download again"; }
check $h 'pacman -Q cowsay' "cowsay is not installed"
check $h 'grep -qx "ParallelDownloads = 1" /var/lib/pacnix/pacman.conf' "the next attempt does not download one file at a time"
[[ $(on $h 'md5sum /etc/pacman.conf') == "$conf" ]] || fail "pacnix changed /etc/pacman.conf"

step "the source host does not resolve for 25 s"
on $h 'echo "127.0.0.1 fastly.mirror.pkgbuild.com" | sudo tee -a /etc/hosts >/dev/null &&
	sudo systemd-run --quiet --on-active=25 sed -i "/fastly.mirror.pkgbuild.com/d" /etc/hosts'
scenario $h '{ packages.repo = [ "cowsay" ]; packages.local.remote = ./packages/remote; }'
out=$(apply $h) || { echo "$out" | tail -20; fail "apply failed while the source host did not resolve"; }
grep -q '^pacnix: attempt 1 of 4 failed in the network: curl: (7) Failed to connect to fastly.mirror.pkgbuild.com' <<<"$out" || { echo "$out"; fail "apply does not try the source download again"; }
check $h 'test -s /usr/share/remote/lastupdate' "the package with the remote source is not installed"

step "a mirror that stays down stops pacnix after four attempts"
on $h 'echo "Server = http://127.0.0.1:9/\$repo/os/\$arch" | sudo tee /etc/pacman.d/mirrorlist >/dev/null'
scenario $h '{ packages.repo = [ "cowsay" "sl" ]; packages.local.remote = ./packages/remote; }'
s=$(date +%s)
out=$(apply $h) && fail "apply succeeded without a mirror"
echo "stopped after $(($(date +%s) - s)) s"
grep -q '^pacnix: attempt 3 of 4 failed in the network' <<<"$out" || { echo "$out" | tail; fail "apply does not try three times again"; }
grep -q "^pacnix: error: pacman -Sw .*failed with exit status: 1. Its last output:" <<<"$out" || fail "the error does not show the reason"

step "a mirror without the package of the sync databases: run pacnix upgrade"
on $h 'echo "Server = https://fastly.mirror.pkgbuild.com/\$repo/os/\$arch/pacnix-missing" | sudo tee /etc/pacman.d/mirrorlist >/dev/null'
out=$(apply $h) && fail "apply succeeded without the package files"
grep -q '^pacnix: error: the mirror does not have a package of the sync databases, thus the databases are old. Run pacnix upgrade: ' <<<"$out" || { echo "$out" | tail; fail "the error does not tell to run pacnix upgrade"; }
grep -q 'attempt 1 of' <<<"$out" && fail "pacnix tried a missing file again"
on $h 'sudo cp /var/tmp/mirrorlist /etc/pacman.d/mirrorlist'

step "a no-op apply downloads nothing"
scenario $h '{ packages.repo = [ "cowsay" ]; packages.local.remote = ./packages/remote; }'
apply $h >/dev/null || fail "apply failed"
out=$(apply $h) || fail "no-op apply failed"
grep -qx 'No changes.' <<<"$out" || { echo "$out"; fail "the second apply has changes"; }
grep -q 'download' <<<"$out" && { echo "$out"; fail "the no-op apply downloads"; }
pass
