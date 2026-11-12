#!/usr/bin/env bash
# Scenarios 1 and 2: a fresh machine reaches the declared state with one command,
# and a second apply changes nothing.
# On success the VM becomes the base disk of the other scenarios.
size=(6 6144 1)
source "$(dirname "$0")/../lib.sh"
export PACNIX_SOURCE_BUILD=1
fresh $h "$cache/base-20261101.620452.qcow2"
step "bootstrap with one command"
apply $h || fail "apply failed"
packages_match $h
for u in sshd.service nix-daemon.socket systemd-networkd.service; do
	check $h "systemctl is-enabled $u" "$u is not enabled"
done
check $h "test \$(systemctl is-enabled systemd-time-wait-sync.service) = disabled" "an undeclared unit stays enabled"
pass
name=02-idempotency
step "second apply changes nothing"
out=$(apply $h) || fail "second apply failed"
grep -qx 'No changes.' <<<"$out" || { echo "$out"; fail "second apply has changes"; }
empty_plan $h
pass
"$vm" snapshot $h "$cache/base-boot.qcow2"
