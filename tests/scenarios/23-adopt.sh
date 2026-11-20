#!/usr/bin/env bash
# Scenario 23: pacnix adopts a system that the user configured by hand. Before
# the first apply, the user changed /etc/fstab of the package filesystem, wrote loader.conf on the EFI system
# partition (vfat), a sudoers rule with the mode 0440 and a unit. The
# configuration declares these files with a different content, a mount, a
# swap file, a group and a user. The plan shows that pacnix keeps the originals, the apply replaces
# the files, and the next plan is empty. When the declarations go, the
# originals come back with their content, mode and owner, the swap file goes
# as the data of a removed feature, and the user and group that pacnix created
# go, without the home directory. A new user without a terminal has no password.
source "$(dirname "$0")/../lib.sh"
fresh $h
paths="/etc/fstab /efi/loader/loader.conf /etc/sudoers.d/10-hand /etc/systemd/system/hand.service"
on $h 'set -e
sudo mkdir -p /efi/loader && printf "timeout 5\n" | sudo tee /efi/loader/loader.conf >/dev/null
echo "%wheel ALL=(ALL:ALL) ALL" | sudo install -m 440 /dev/stdin /etc/sudoers.d/10-hand
printf "[Service]\nType=oneshot\nExecStart=/usr/bin/true\n" | sudo tee /etc/systemd/system/hand.service >/dev/null' || fail "the configuration by hand failed"
sum="sudo stat -c '%n %a %U %s' $paths; sudo md5sum $paths"
before=$(on $h "$sum")
step "adopt the files"
scenario $h '{
  fileSystems."/mnt/hand" = { device = "tmpfs"; fsType = "tmpfs"; };
  swapDevices = [ { device = "/swapfile"; size = 64; } ];
  files."/efi/loader/loader.conf" = { text = "timeout 1\n"; mode = null; };
  files."/etc/sudoers.d/10-hand" = { text = "%wheel ALL=(ALL:ALL) NOPASSWD: ALL\n"; mode = "0440"; };
  files."/etc/systemd/system/hand.service".text = "[Service]\nType=oneshot\nExecStart=/usr/bin/echo adopted\n";
  users.groups.team = { };
  users.users.alice = { extraGroups = [ "team" "wheel" ]; description = "Alice"; };
}'
out=$(plan $h)
for l in '/etc/fstab| of package filesystem' /efi/loader/loader.conf /etc/sudoers.d/10-hand /etc/systemd/system/hand.service; do
	p=${l%%|*} of=${l#*|}; [[ $l == *'|'* ]] || of=''
	has "$out" "Changing file $p (pacnix keeps the original$of" || { echo "$out"; fail "the plan does not keep the original of $p"; }
done
out=$(apply $h) || { echo "$out" | tail -20; fail "the adoption failed"; }
grep -qx '• the new user alice has no password. Set it with: passwd alice' <<<"$out" || fail "the summary does not tell to set the password"
check $h 'test "$(id -nG alice | tr " " "\n" | sort | xargs)" = "alice team wheel" && test "$(getent passwd alice | cut -d: -f5)" = Alice' "the new user is wrong"
check $h 'grep -qx "tmpfs /mnt/hand tmpfs defaults 0 0" /etc/fstab && systemctl cat mnt-hand.mount >/dev/null' "the declared mount is not in /etc/fstab"
check $h 'test -f /swapfile && systemctl cat swapfile.swap >/dev/null' "the swap file is not made"
check $h 'sudo grep -qx "timeout 1" /efi/loader/loader.conf && sudo grep -q NOPASSWD /etc/sudoers.d/10-hand && grep -q adopted /etc/systemd/system/hand.service' "the declared files are not written"
check $h "test \"\$(sudo stat -c %a /etc/sudoers.d/10-hand)\" = 440" "the mode of the sudoers rule is wrong"
empty_plan $h
step "remove the declarations"
scenario $h ''
out=$(plan $h)
for p in $paths; do
	has "$out" "Restoring file $p (the original from before pacnix)" || { echo "$out"; fail "the plan does not restore the original of $p"; }
done
apply $h >/dev/null || fail "the restore failed"
after=$(on $h "$sum")
[[ $before == "$after" ]] || { diff <(echo "$before") <(echo "$after"); fail "the originals did not come back"; }
refute $h 'test -e /swapfile' "the swap file stays"
refute $h 'getent passwd alice || getent group team' "the user or the group that pacnix created stays"
check $h 'test -d /home/alice' "the home directory of the deleted user is gone"
refute $h 'sudo find /var/lib/pacnix/originals -name fstab -o -name loader.conf -o -name "*hand*" | grep -q .' "the kept originals stay"
empty_plan $h
pass
