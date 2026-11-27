#!/usr/bin/env bash
# Scenario 5: kill apply during the pacman transaction, during file writes and
# during a build, and power the VM off during apply. The next apply converges.
# The next build removes the copies of the build chroot that the killed build
# left, as Btrfs subvolumes or as directories, and keeps a copy that a build
# locks.
size=(2 3072 1)
source "$(dirname "$0")/../lib.sh"
fresh $h
apply $h >/dev/null || fail "first apply failed"
# Starts apply in a transient unit, so that a kill reaches pacnix and pacman.
# The configuration is on the disk first, so that it stays after a power off.
start() { on $h 'sync && sudo systemd-run --quiet --unit pacnix-test --collect pacnix apply'; }
# Waits until a shell condition is true in the VM.
until_true() { on $h "for i in \$(seq 900); do $1 && exit 0; sleep 0.2; done; exit 1" || fail "timeout: $1"; }
converge() {
	apply $h >/dev/null || fail "apply after the interruption failed"
	packages_match $h
	empty_plan $h
	refute $h "test -e /var/lib/pacnix/pending" "the pending generation is not cleared"
}

step "kill apply during the pacman transaction"
# biber brings more than 100 small packages: the kill comes in the middle of the transaction.
scenario $h '{ packages.repo = [ "biber" ]; }'
on $h 'sudo truncate -s0 /var/log/pacman.log'
start
until_true 'test $(grep -c "\[ALPM\] installed" /var/log/pacman.log) -ge 20'
on $h 'sudo systemctl kill -s KILL pacnix-test; sudo systemctl reset-failed pacnix-test 2>/dev/null; true'
on $h 'grep -c "\[ALPM\] installed" /var/log/pacman.log; ls /var/lib/pacman/db.lck 2>&1' || true
converge

step "kill apply during file writes"
scenario $h '{ lib, ... }: { files = lib.genAttrs (map (i: "/etc/pacnix-many/f${toString i}") (lib.range 1 4000)) (p: { text = p; }); }'
start
until_true 'test $(ls /etc/pacnix-many 2>/dev/null | wc -l) -gt 200'
on $h 'sudo systemctl kill -s KILL pacnix-test; sudo systemctl reset-failed pacnix-test 2>/dev/null; ls /etc/pacnix-many | wc -l'
converge
check $h 'test $(ls -A /etc/pacnix-many | wc -l) = 4000' "the files are not complete, or temporary files remain"

step "kill apply during a build"
chroots=/var/lib/aurbuild/x86_64
check $h "test \"\$(stat -f -c %T $chroots/root)\" = btrfs && test \"\$(stat -c %i $chroots/root)\" = 256" "the build chroot is not a Btrfs subvolume"
scenario $h '{ packages.local.slow = ./packages/slow; }'
start
until_true "sudo sh -c 'ls -d $chroots/*/build/slow/src' >/dev/null 2>&1"
on $h 'sudo systemctl kill -s KILL pacnix-test; sudo systemctl reset-failed pacnix-test 2>/dev/null; true'
until_true '! pgrep -x systemd-nspawn >/dev/null'
copy=$(on $h "cd $chroots && sudo sh -c 'ls -d */build/slow' | cut -d/ -f1")
check $h "test -n '$copy' && test -e $chroots/$copy.lock" "the killed build left no copy of the build chroot"
on $h "sudo btrfs -q subvolume create $chroots/$copy/nested && sudo mkdir -p $chroots/plain/build $chroots/busy && sudo touch $chroots/plain.lock $chroots/lone.lock && sudo systemd-run --quiet --unit pacnix-busy flock $chroots/busy.lock sleep 600"
scenario $h '{ packages.local.kg-a = ./packages/kg-a; }'
out=$(apply $h) || { echo "$out"; fail "apply after the killed build failed"; }
has "$out" "Removing the leftover build chroot $chroots/$copy." || { echo "$out"; fail "apply does not show the removal of the copy"; }
grep -q 'leftover build chroot .*\.lck' <<<"$out" && { echo "$out"; fail "apply shows a lock as a build chroot"; }
check $h "test \"\$(ls -A $chroots | tr '\n' ' ')\" = 'busy busy.lock root root.lock '" "a leftover copy stays, or the locked copy went"
on $h 'sudo systemctl stop pacnix-busy'
packages_match $h
empty_plan $h

step "power off during apply"
scenario $h '{ packages.repo = [ "biber" ]; }'
on $h 'sudo truncate -s0 /var/log/pacman.log'
start
until_true 'grep -q "transaction started" /var/log/pacman.log'
"$vm" reset $h
converge
check $h 'test ! -e /etc/pacnix-many' "dropped files remain"
check $h "test \"\$(ls -A $chroots | tr '\n' ' ')\" = 'root root.lock '" "the copy that a build locked stays after the lock"
pass
