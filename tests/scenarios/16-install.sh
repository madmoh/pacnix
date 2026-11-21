#!/usr/bin/env bash
# Scenario 16: pacnix takes over a system that the installation guide made by
# hand. A live system (the role of the Arch ISO) partitions, formats and mounts
# a disk with an EFI system partition, pacstraps base, linux and the packages
# for network and SSH, and in arch-chroot configures by hand: genfstab, time
# zone, locale, host name, systemd-boot, a user with sudo and SSH, the network,
# and [pacnix]. pacnix apply in arch-chroot refuses and changes nothing. The
# disk boots with UEFI, without cloud-init. The user clones the configuration
# (a repository on the disk takes the role of GitHub), and the plan shows that
# pacnix keeps the originals of the files that it replaces. pacnix apply, as
# the user without --yes, asks nothing. The local package slow first fails
# with a message that has "timed out": pacnix shows the reason, does not try
# again, applies the rest of the configuration and exits with status 4, and
# the download stays for the next attempt. After
# the correction, the build is silent for 65 s, then redraws a progress meter
# for 65 s: the output shows each step, a line for the silence and a line with
# the last state of the meter, without the output of the build, Nix notes or
# escape sequences. The download and the build directory go after the install. After a reboot, the system
# boots the declared loader entry, and has the declared swap, locale, user and
# home configuration and an empty plan.
size=(6 4096 1)
source "$(dirname "$0")/../lib.sh"
fresh live "$cache/base-20261101.620452.qcow2"
"$vm" blank live 12G
on live "sudo tee /etc/pacnix/scenario.nix >/dev/null" <<<"{ features.machine.sshKey = \"$(cat "$cache/id_ed25519.pub")\n\"; }"
key=$(cat "$cache/id_ed25519.pub")

step "install by hand with the installation guide"
on live "set -e
printf 'label: gpt\n,512M,U\n,,L\n' | sudo sfdisk -q /dev/vdb
sudo mkfs.fat -F32 -n ESP /dev/vdb1 >/dev/null && sudo mkfs.ext4 -qL root /dev/vdb2
sudo mount /dev/vdb2 /mnt && sudo mount --mkdir /dev/vdb1 /mnt/boot
sudo pacstrap -K /mnt base linux openssh sudo zsh git >/dev/null
sudo genfstab -U /mnt | sudo tee -a /mnt/etc/fstab >/dev/null
# Not steps of the guide: the test repository [pacnix] is a directory, Nix downloads
# through the cache of the tests, and a Git repository takes the role of GitHub.
sudo cp -a /srv/pacnix /mnt/srv/ && sudo install -Dm644 /etc/xdg/nix/nix.conf /mnt/etc/xdg/nix/nix.conf
sudo git -C /etc/pacnix init -q && sudo git -C /etc/pacnix add -A && sudo git -C /etc/pacnix -c user.name=test -c user.email=test@pacnix commit -qm config
sudo git clone -q --bare /etc/pacnix /mnt/srv/config.git
sudo arch-chroot /mnt sh -ec '
ln -sf /usr/share/zoneinfo/UTC /etc/localtime
echo \"en_US.UTF-8 UTF-8\" >/etc/locale.gen && locale-gen >/dev/null
echo LANG=en_US.UTF-8 >/etc/locale.conf
echo newbox >/etc/hostname
bootctl install >/dev/null 2>&1
printf \"default arch.conf\ntimeout 3\n\" >/boot/loader/loader.conf
printf \"title Arch Linux\nlinux /vmlinuz-linux\ninitrd /initramfs-linux.img\noptions root=LABEL=root rw\n\" >/boot/loader/entries/arch.conf
useradd -m -G wheel arch && chown -R arch:arch /srv/config.git
echo \"%wheel ALL=(ALL:ALL) NOPASSWD: ALL\" >/etc/sudoers.d/wheel
install -d -o arch -g arch -m 700 /home/arch/.ssh && echo \"$key\" | install -o arch -g arch -m 600 /dev/stdin /home/arch/.ssh/authorized_keys
printf \"[Match]\nName=en*\n[Network]\nDHCP=yes\n\" >/etc/systemd/network/20-wired.network
systemctl enable -q sshd systemd-networkd systemd-resolved
printf \"\n[pacnix]\nServer = file:///srv/pacnix\n\" >>/etc/pacman.conf
pacman-key --add /srv/pacnix/test-key.asc >/dev/null 2>&1 && pacman-key --lsign-key \"pacnix test\" >/dev/null 2>&1
pacman -S --noconfirm pacnix >/dev/null 2>&1'
sudo ln -sf /run/systemd/resolve/stub-resolv.conf /mnt/etc/resolv.conf" || fail "the installation by hand failed"
step "pacnix apply in arch-chroot refuses"
before=$(on live 'sudo find /mnt -xdev -newer /mnt/etc/hostname 2>/dev/null | grep -v "^/mnt/\(var/log\|proc\|sys\|dev\|run\|tmp\)" | sort')
out=$(on live 'sudo arch-chroot /mnt pacnix apply --yes 2>&1') && fail "pacnix apply ran in arch-chroot"
has "$out" '• Error: pacnix runs only on the booted system. Boot the installed system, then run pacnix again' || { echo "$out"; fail "the refusal does not tell to boot"; }
[[ $before == "$(on live 'sudo find /mnt -xdev -newer /mnt/etc/hostname 2>/dev/null | grep -v "^/mnt/\(var/log\|proc\|sys\|dev\|run\|tmp\)" | sort')" ]] || fail "the refused apply changed the system"
# pacstrap -K starts a gpg-agent for the new keyring, which keeps /mnt busy.
on live 'sudo gpgconf --homedir /mnt/etc/pacman.d/gnupg --kill gpg-agent && sudo umount -R /mnt && sudo systemctl poweroff' || true
while virsh -c qemu:///system domstate pacnix-live 2>/dev/null | grep -q running; do sleep 2; done
mv "$cache/pacnix-live-vdb.qcow2" "$cache/newbox-disk.qcow2"
"$vm" down live

step "boot, clone the configuration and look at the plan"
started+=(newbox)
"$vm" boot newbox "$cache/newbox-disk.qcow2"
on newbox 'sudo install -d -o arch -g arch /etc/pacnix && git clone -q /srv/config.git /etc/pacnix && echo fail >/etc/pacnix/packages/slow/mode' || fail "git clone failed"
out=$(plan newbox) || { echo "$out"; fail "plan failed"; }
for p in /etc/fstab /boot/loader/loader.conf /etc/localtime; do
	matches "$out" "Changing file $p \(pacnix keeps the original" || { echo "$out"; fail "the plan does not keep the original of $p"; }
done
has "$out" 'Change user arch: shell /usr/bin/zsh' || { echo "$out"; fail "the plan does not change the shell"; }
step "a build failure with \"timed out\" in its output does not stop the apply"
rc=0
out=$(pacnix newbox apply 2>&1) || rc=$?
echo "$out" | tail -8
((rc == 4)) || fail "the exit status of the apply with the failing build is $rc, not 4"
matches "$out" '• Error: build local package slow: aur chroot .* failed with exit status: 255' || fail "the error does not name the build and its status"
once "$out" '• Error: build local package slow:' || fail "the output does not show the failed build one time"
has "$out" '• Apply done, but 1 step failed.' || fail "the summary does not count the failed step"
check newbox 'test "$(readlink /etc/localtime)" = /usr/share/zoneinfo/Europe/Berlin' "the apply with the failing build did not apply the rest of the configuration"
grep -q '^  | error: the test build timed out while the file mode has the word fail' <<<"$out" || fail "the error does not show the reason of the failure"
grep -q 'attempt 1 of' <<<"$out" && fail "pacnix tried a build failure again"
grep -q 'aurutils' <<<"$out" && fail "the apply builds aurutils"
check newbox 'sudo test -s /var/lib/pacnix/build/sources/slow/lastupdate' "the download of the failed build does not stay for the next attempt"
step "the next apply builds the package and asks no question"
on newbox 'echo ok >/etc/pacnix/packages/slow/mode'
out=$(pacnix newbox apply 2>&1) || { echo "$out" | tail -30; fail "the apply after the correction failed"; }
for l in '• Building local package 1/1: slow' '• The step continues after 1 min without output: Building local package 1/1: slow'; do
	has "$out" "$l" || fail "the output does not show: $l"
done
grep -q '==> colored line\|progress done' <<<"$out" && fail "the output shows the output of a build that succeeded"
refute newbox 'sudo test -e /var/lib/pacnix/build/sources/slow' "the download stays after the build succeeded"
refute newbox 'sudo test -e /var/lib/pacnix/build/local/slow' "the build directory stays after the install"
matches "$out" '• The step continues after 2 min: Building local package 1/1: slow: downloaded [0-9]+ MB' || fail "the output does not show the progress meter after 60 s of redraws"
grep -q 'progress 10%\|not writing modified lock file\|Added input' <<<"$out" && fail "the output shows progress meter redraws or Nix notes"
grep -q $'\e\|\r' <<<"$out" && fail "the output has escape sequences or carriage returns"
check newbox 'sudo grep -q "^UUID=" /var/lib/pacnix/originals/etc/fstab && sudo grep -q "timeout 3" /var/lib/pacnix/originals/boot/loader/loader.conf' "the originals are not kept"
step "reboot"
"$vm" reboot newbox
check newbox 'test "$(systemctl is-system-running)" = running' "the system is not healthy after the reboot"
check newbox 'grep -q "root=LABEL=root" /proc/cmdline && test "$(sudo bootctl status 2>/dev/null | awk "/Current Entry:/ { print \$3 }")" = linux.conf' "the system does not boot the declared entry"
check newbox 'test "$(id -nG)" = "arch wheel" && test "$(getent passwd arch | cut -d: -f7)" = /usr/bin/zsh' "the user, its groups or its shell are wrong"
check newbox 'test "$(readlink /etc/localtime)" = /usr/share/zoneinfo/Europe/Berlin' "the time zone link is wrong"
check newbox 'locale -a | grep -qx en_US.utf8' "the locale is not generated"
check newbox 'swapon --show=NAME --noheadings | sort | tr "\n" " " | grep -qx "/dev/zram0 /swapfile "' "the swap file or zram is not active"
check newbox 'pacman -Q slow' "the local package is not installed"
check newbox 'test "$(dconf read /org/gnome/desktop/interface/color-scheme)" = "'"'prefer-dark'"'"' "the home configuration is not active"
refute newbox 'sudo find /home/arch -user root | grep -q .' "a file in the home belongs to root"
refute newbox 'sudo find /var/lib/pacnix/repo ! -user root | grep -q .' "the builder user owns a file of the local repository"
check newbox 'sudo gpg --homedir /var/lib/pacnix/gnupg --verify /var/lib/pacnix/repo/pacnix-local.db.sig /var/lib/pacnix/repo/pacnix-local.db' "the local repository is not signed"
check newbox 'pacman -Qi aurutils | grep -qx "Install Reason  : Installed as a dependency for another package"' "aurutils is not a dependency of pacnix"
refute newbox 'sudo -l -U pacnix | grep -q "may run"' "the builder user has a sudo rule"
packages_match newbox
empty_plan newbox
pass
