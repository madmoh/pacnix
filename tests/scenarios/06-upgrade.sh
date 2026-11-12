#!/usr/bin/env bash
# Scenario 6: pacnix upgrade of a host from the image of 2026-10-01, with a
# kernel update, DKMS builds from the repositories and the AUR, .pacnew files
# and a reboot.
size=(4 6144 1)
source "$(dirname "$0")/../lib.sh"
old=$cache/base-20261001.604814.qcow2
[[ -f $old ]] || "$vm" base "$cache/cloud-20261001.604814.qcow2" 2026/10/01 >/dev/null
PACNIX_VM_MIRROR=https://archive.archlinux.org/repos/2026/10/01 fresh old "$old"
step "bootstrap the old system with DKMS modules"
apply old >/dev/null || fail "first apply failed"
kernel=$(on old 'pacman -Q linux | cut -d" " -f2')
check old "dkms status | grep -q 'v4l2loopback.*$(on old uname -r).*installed'" "the DKMS module of the repositories is not built"
check old "dkms status | grep -q 'corsair.*installed'" "the DKMS module of the AUR is not built"
# A backup file that the upgrade changes: the declared one gets a .pacnew,
# which apply deletes. An unmanaged one that the user changed stays for the audit.
on old 'echo "# local change" | sudo tee -a /etc/makepkg.conf >/dev/null'
step "switch to current mirrors and upgrade"
scenario old '{ files."/etc/pacman.d/mirrorlist".text = "Server = https://fastly.mirror.pkgbuild.com/$repo/os/$arch\nServer = https://mirror.rackspace.com/archlinux/$repo/os/$arch\n"; features.overrides.enable = true; }'
apply old >/dev/null || fail "apply of the mirrors failed"
pacnix old upgrade --yes || fail "upgrade failed"
new=$(on old 'pacman -Q linux | cut -d" " -f2')
[[ $new != "$kernel" ]] || fail "the kernel did not change ($kernel)"
echo "kernel $kernel -> $new"
check old "dkms status | grep -q 'v4l2loopback.*${new%.arch*}.*installed' || dkms status | grep v4l2loopback | grep -q '$(echo $new | sed 's/\.arch/-arch/')'" "the DKMS module is not built for the new kernel"
# The backup files did not change upstream in this period. A changed backup
# checksum in the local database makes the reinstall of pacman act as if they did.
on old 'sudo sed -i -E "/^etc\/(pacman|makepkg)\.conf\t/s/\t.*/\t00000000000000000000000000000000/" /var/lib/pacman/local/pacman-[0-9]*/files && sudo pacman -S --noconfirm pacman >/dev/null 2>&1'
check old 'test -e /etc/pacman.conf.pacnew && test -e /etc/makepkg.conf.pacnew' "the reinstall made no .pacnew files"
apply old >/dev/null || fail "apply after the .pacnew files failed"
refute old 'test -e /etc/pacman.conf.pacnew' "the .pacnew of a managed file remains"
pacnix old audit | grep -qx 'pacnew file: /etc/makepkg.conf.pacnew' || fail "the audit does not list the .pacnew of an unmanaged file"
empty_plan old
step "reboot into the new kernel"
on old 'sudo systemctl reboot' || true
sleep 10
"$vm" wait old
check old "test \$(uname -r) = $(echo "$new" | sed 's/\.arch/-arch/')" "the new kernel does not run"
check old 'sudo modprobe v4l2loopback && lsmod | grep -q v4l2loopback' "the DKMS module does not load"
check old 'test "$(systemctl is-system-running)" = running' "the system is not healthy"
empty_plan old
pass
