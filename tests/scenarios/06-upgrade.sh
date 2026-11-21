#!/usr/bin/env bash
# Scenario 6: pacnix apply of a host from the image of 2026-10-01, with a
# kernel update, DKMS builds from the repositories and the AUR, .pacnew files
# and a reboot. The summary of the upgrade tells to reboot for the new kernel.
size=(4 4096 1)
source "$(dirname "$0")/../lib.sh"
old=$cache/base-20261001.604814.qcow2
[[ -f $old ]] || "$vm" base "$cache/cloud-20261001.604814.qcow2" 2026/10/01 >/dev/null
PACNIX_VM_MIRROR=$("$vm" mirror 2026/10/01) fresh old "$old"
step "bootstrap the old system with DKMS modules"
apply old >/dev/null || fail "first apply failed"
kernel=$(on old 'pacman -Q linux | cut -d" " -f2')
check old "dkms status | grep -q 'v4l2loopback.*$(on old uname -r).*installed'" "the DKMS module of the repositories is not built"
check old "dkms status | grep -q 'corsair.*installed'" "the DKMS module of the AUR is not built"
# A backup file that the upgrade changes: the declared one gets a .pacnew,
# which apply deletes. An unmanaged one that the user changed stays for the audit.
on old 'echo "# local change" | sudo tee -a /etc/makepkg.conf >/dev/null'
step "switch to the mirrors of the other VMs and upgrade"
servers=$(for m in $("$vm" mirror); do printf 'Server = %s/$repo/os/$arch\\n' "$m"; done)
scenario old "{ files.\"/etc/pacman.d/mirrorlist\".text = \"$servers\"; features.overrides.enable = true; }"
apply old >/dev/null || fail "apply of the mirrors failed"
out=$(pacnix old apply --yes) || { echo "$out" | tail -20; fail "upgrade failed"; }
new=$(on old 'pacman -Q linux | cut -d" " -f2')
[[ $new != "$kernel" ]] || fail "the kernel did not change ($kernel)"
echo "kernel $kernel -> $new"
running=$(on old uname -r)
has "$out" "the running kernel $running is not installed" || { grep '^• Reboot' <<<"$out"; fail "the summary does not tell to reboot for the kernel"; }
matches "$out" '• Reboot to use the changes: .*the module v4l2loopback was built for the kernel [^ ]*, which does not run' || fail "the summary does not name the DKMS module for the new kernel"
check old "dkms status | grep -q 'v4l2loopback.*${new%.arch*}.*installed' || dkms status | grep v4l2loopback | grep -q '$(echo $new | sed 's/\.arch/-arch/')'" "the DKMS module is not built for the new kernel"
# The backup files did not change upstream in this period. A changed backup
# checksum in the local database makes the reinstall of pacman act as if they did.
on old 'sudo sed -i -E "/^etc\/(pacman|makepkg)\.conf\t/s/\t.*/\t00000000000000000000000000000000/" /var/lib/pacman/local/pacman-[0-9]*/files && sudo pacman -S --noconfirm pacman >/dev/null 2>&1'
check old 'test -e /etc/pacman.conf.pacnew && test -e /etc/makepkg.conf.pacnew' "the reinstall made no .pacnew files"
apply old >/dev/null || fail "apply after the .pacnew files failed"
refute old 'test -e /etc/pacman.conf.pacnew' "the .pacnew of a managed file remains"
audit_has "$(pacnix old audit)" 'File .pacnew' /etc/makepkg.conf.pacnew || fail "the audit does not list the .pacnew of an unmanaged file"
empty_plan old
step "reboot into the new kernel"
"$vm" reboot old
check old "test \$(uname -r) = $(echo "$new" | sed 's/\.arch/-arch/')" "the new kernel does not run"
check old 'sudo modprobe v4l2loopback && lsmod | grep -q v4l2loopback' "the DKMS module does not load"
check old 'test "$(systemctl is-system-running)" = running' "the system is not healthy"
empty_plan old
pass
