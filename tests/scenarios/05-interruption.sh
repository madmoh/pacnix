#!/usr/bin/env bash
# Scenario 5: kill apply during the pacman transaction and during file writes,
# and power the VM off during apply. The next apply converges.
size=(2 4096 1)
source "$(dirname "$0")/../lib.sh"
fresh $h
apply $h >/dev/null || fail "first apply failed"
# Starts apply in a transient unit, so that a kill reaches pacnix and pacman.
start() { on $h 'sudo systemd-run --quiet --unit pacnix-test --collect pacnix apply --yes'; }
# Waits until a shell condition is true in the VM.
until_true() { on $h "for i in \$(seq 900); do $1 && exit 0; sleep 0.2; done; exit 1" || fail "timeout: $1"; }
converge() {
	apply $h >/dev/null || fail "apply after the interruption failed"
	packages_match $h
	empty_plan $h
	refute $h "test -e /var/lib/pacnix/pending" "the pending generation is not cleared"
}

step "kill apply during the pacman transaction"
scenario $h '{ packages.repo = [ "gimp" "inkscape" ]; }'
on $h 'sudo truncate -s0 /var/log/pacman.log'
start
until_true 'grep -q "transaction started" /var/log/pacman.log'
sleep 3
on $h 'sudo systemctl kill -s KILL pacnix-test; sudo systemctl reset-failed pacnix-test 2>/dev/null; true'
on $h 'grep -c "\[ALPM\] installed" /var/log/pacman.log; ls /var/lib/pacman/db.lck 2>&1' || true
converge

step "kill apply during file writes"
scenario $h '{ lib, ... }: { packages.repo = [ "gimp" "inkscape" ]; files = lib.genAttrs (map (i: "/etc/pacnix-many/f${toString i}") (lib.range 1 4000)) (p: { text = p; }); }'
start
until_true 'test $(ls /etc/pacnix-many 2>/dev/null | wc -l) -gt 200'
on $h 'sudo systemctl kill -s KILL pacnix-test; sudo systemctl reset-failed pacnix-test 2>/dev/null; ls /etc/pacnix-many | wc -l'
converge
check $h 'test $(ls -A /etc/pacnix-many | wc -l) = 4000' "the files are not complete, or temporary files remain"

step "power off during apply"
scenario $h '{ packages.repo = [ "libreoffice-fresh" ]; }'
on $h 'sudo truncate -s0 /var/log/pacman.log'
start
until_true 'grep -q "transaction started" /var/log/pacman.log'
sleep 4
"$vm" reset $h
converge
check $h 'test ! -e /etc/pacnix-many' "dropped files remain"
pass
