#!/usr/bin/env bash
# Scenario 19: downloads that fail for a time. A mirror that refuses
# connections and a source host that does not resolve do not fail the apply:
# pacnix tries the apply again with one download at a time and continues from
# the downloaded files, and the pacman.conf of the user does not change. A
# mirror that stays down stops pacnix after four attempts in about two
# minutes. A mirror without a file of the sync databases (404) stops pacnix at
# once and tells to run pacnix apply without --config-only. A no-op apply
# downloads nothing. The package files stay in /var/lib/pacnix/cache after a
# failed step, for the next apply, and leave after an apply without a failure;
# the files in /var/cache/pacman/pkg do not change.
source "$(dirname "$0")/../lib.sh"
fresh $h
apply $h >/dev/null || fail "first apply failed"
conf=$(on $h 'md5sum /etc/pacman.conf')
cache=$(cache_list $h)
# Runs a command in the VM when pacnix shows its next failed first attempt, so
# that the next attempt, after 10 s, succeeds.
after_failure() { on $h "sudo systemd-run --quiet sh -c 'n=\$(grep -c \"Attempt 1 of\" /var/log/pacnix-test.log); until [ \$(grep -c \"Attempt 1 of\" /var/log/pacnix-test.log) -gt \$n ]; do sleep 0.2; done; $1'"; }

step "the mirror refuses connections until the first attempt fails"
on $h 'sudo cp /etc/pacman.d/mirrorlist /var/tmp/mirrorlist && echo "Server = http://127.0.0.1:9/\$repo/os/\$arch" | sudo tee /etc/pacman.d/mirrorlist >/dev/null'
after_failure 'cp /var/tmp/mirrorlist /etc/pacman.d/mirrorlist'
scenario $h '{ packages.repo = [ "cowsay" ]; }'
out=$(apply $h) || { echo "$out" | tail -20; fail "apply failed while the mirror refused connections"; }
matches "$out" "• Attempt 1 of 4 failed in the network: error: failed retrieving file '.*127.0.0.1.*Next attempt in 10 s, with one download at a time" || { echo "$out"; fail "apply does not try the download again"; }
check $h 'pacman -Q cowsay' "cowsay is not installed"
check $h 'grep -qx "ParallelDownloads = 1" /run/pacnix/pacman.conf' "the next attempt does not download one file at a time"
[[ $(on $h 'md5sum /etc/pacman.conf') == "$conf" ]] || fail "pacnix changed /etc/pacman.conf"
cache_kept $h "$cache"

step "the source host does not resolve until the first attempt fails"
on $h 'echo "127.0.0.1 fastly.mirror.pkgbuild.com" | sudo tee -a /etc/hosts >/dev/null'
after_failure 'sed -i "/fastly.mirror.pkgbuild.com/d" /etc/hosts'
scenario $h '{ packages.repo = [ "cowsay" ]; packages.local.remote = ./packages/remote; }'
out=$(apply $h) || { echo "$out" | tail -20; fail "apply failed while the source host did not resolve"; }
has "$out" '• Attempt 1 of 4 failed in the network: curl: (7) Failed to connect to fastly.mirror.pkgbuild.com' || { echo "$out"; fail "apply does not try the source download again"; }
check $h 'test -s /usr/share/remote/lastupdate' "the package with the remote source is not installed"
cache_kept $h "$cache"

step "a failed step keeps the downloaded package files until an apply without a failure"
scenario $h '{ packages.repo = [ "cowsay" "sl" ]; packages.local.remote = ./packages/remote; files."/etc/st.conf".text = "1\n"; triggers.stfail = { paths = [ "/etc/st.conf" ]; command = "test -e /etc/st-ok"; }; }'
apply $h >/dev/null && fail "apply succeeded with a failed trigger"
check $h 'pacman -Q sl && sudo ls /var/lib/pacnix/cache | grep -q "^sl-.*\.pkg\.tar\.zst$"' "the package file of sl is not in /var/lib/pacnix/cache after the failed step"
on $h 'sudo touch /etc/st-ok'
apply $h >/dev/null || fail "apply failed after the correction of the trigger"
cache_kept $h "$cache"

step "a mirror that stays down stops pacnix after four attempts"
on $h 'echo "Server = http://127.0.0.1:9/\$repo/os/\$arch" | sudo tee /etc/pacman.d/mirrorlist >/dev/null'
scenario $h '{ packages.repo = [ "cowsay" "sl" "fortune-mod" ]; packages.local.remote = ./packages/remote; }'
s=$(date +%s)
out=$(apply $h) && fail "apply succeeded without a mirror"
echo "stopped after $(($(date +%s) - s)) s"
has "$out" '• Attempt 3 of 4 failed in the network' || { echo "$out" | tail; fail "apply does not try three times again"; }
matches "$out" '• Error: pacman -S.*failed with exit status: 1. Its last output:' || fail "the error does not show the reason"

step "a mirror without the package of the sync databases: run pacnix apply without --config-only"
on $h 'echo "Server = https://fastly.mirror.pkgbuild.com/\$repo/os/\$arch/pacnix-missing" | sudo tee /etc/pacman.d/mirrorlist >/dev/null'
out=$(apply $h --config-only) && fail "apply succeeded without the package files"
has "$out" '• Error: the mirror does not have a package of the sync databases, thus the databases or the mirror are old. Run pacnix apply without --config-only, or use a different mirror: ' || { echo "$out" | tail; fail "the error does not tell to run pacnix apply without --config-only"; }
grep -q 'Attempt 1 of' <<<"$out" && fail "pacnix tried a missing file again"
on $h 'sudo cp /var/tmp/mirrorlist /etc/pacman.d/mirrorlist'

step "a no-op apply downloads nothing"
scenario $h '{ packages.repo = [ "cowsay" ]; packages.local.remote = ./packages/remote; }'
apply $h >/dev/null || fail "apply failed"
out=$(apply $h) || fail "no-op apply failed"
grep -q '^• Apply done. No changes.' <<<"$out" || { echo "$out"; fail "the second apply has changes"; }
grep -q '^• Downloading [0-9]' <<<"$out" && { echo "$out"; fail "the no-op apply downloads packages"; }
pass
