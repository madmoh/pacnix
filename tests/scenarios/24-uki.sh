#!/usr/bin/env bash
# Scenario 24: pacnix takes over a system that boots from a UKI. A live system
# installs by hand an encrypted Btrfs root with the subvolume @ (LUKS with a key
# file in the initramfs, because the VM has no TPM), the EFI system partition
# at /efi, a mkinitcpio preset that builds a UKI, loader.conf and an old entry.
# The configuration declares the EFI system partition at /boot, UKIs and the
# LUKS parameters. The plan refuses while /boot is not a mount point. After the
# user mounts the partition at /boot, the apply builds the UKI with the
# complete command line, and the system boots it. A change of the command line
# and a kernel installation build the UKI again. A change to entries deletes
# the UKI, and the system boots the entry. Each plan after an apply is empty.
size=(6 4096 1)
source "$(dirname "$0")/../lib.sh"
fresh ukilive "$cache/base-20261101.620452.qcow2"
"$vm" blank ukilive 12G
key=$(cat "$cache/id_ed25519.pub")

step "install by hand: LUKS, Btrfs with the subvolume @ and a UKI on the EFI system partition at /efi"
on ukilive "set -e
sudo pacman -S --noconfirm --needed btrfs-progs >/dev/null 2>&1
printf 'label: gpt\n,512M,U\n,,L\n' | sudo sfdisk -q /dev/vdb
sudo mkfs.fat -F32 -n ESP /dev/vdb1 >/dev/null
sudo dd if=/dev/urandom of=/root/root.key bs=64 count=1 status=none
sudo cryptsetup luksFormat -q --type luks2 --pbkdf pbkdf2 --pbkdf-force-iterations 1000 /dev/vdb2 /root/root.key
sudo cryptsetup open --key-file /root/root.key /dev/vdb2 root
sudo mkfs.btrfs -q -L root /dev/mapper/root
sudo mount /dev/mapper/root /mnt && sudo btrfs -q subvolume create /mnt/@ && sudo umount /mnt
sudo mount -o subvol=/@,compress=zstd /dev/mapper/root /mnt && sudo mount --mkdir /dev/vdb1 /mnt/efi
sudo pacstrap -K /mnt base linux openssh sudo zsh git btrfs-progs systemd-ukify >/dev/null 2>&1
sudo genfstab -U /mnt | sudo tee -a /mnt/etc/fstab >/dev/null
sudo install -Dm400 /root/root.key /mnt/etc/cryptsetup-keys.d/root.key
uuid=\$(sudo cryptsetup luksUUID /dev/vdb2)
sudo cp -a /srv/pacnix /mnt/srv/ && sudo install -Dm644 /etc/xdg/nix/nix.conf /mnt/etc/xdg/nix/nix.conf && sudo cp -a /etc/pacnix /mnt/etc/
sudo arch-chroot /mnt sh -ec '
ln -sf /usr/share/zoneinfo/UTC /etc/localtime
echo \"en_US.UTF-8 UTF-8\" >/etc/locale.gen && locale-gen >/dev/null
echo LANG=en_US.UTF-8 >/etc/locale.conf
echo ukibox >/etc/hostname
printf \"HOOKS=(base systemd autodetect microcode modconf kms keyboard block sd-encrypt filesystems fsck)\nFILES=(/etc/cryptsetup-keys.d/root.key)\n\" >/etc/mkinitcpio.conf.d/10-encrypt.conf
echo \"root=/dev/mapper/root rootflags=subvol=/@ rw rd.luks.name='\$uuid'=root rd.luks.key='\$uuid'=/etc/cryptsetup-keys.d/root.key\" >/etc/kernel/cmdline
printf \"ALL_kver=/boot/vmlinuz-linux\nPRESETS=(default)\ndefault_uki=/efi/EFI/Linux/arch-linux.efi\n\" >/etc/mkinitcpio.d/linux.preset
mkinitcpio -P >/dev/null 2>&1
bootctl install >/dev/null 2>&1
printf \"default arch-linux.efi\ntimeout 3\n\" >/efi/loader/loader.conf
printf \"title Old system\nlinux /vmlinuz-old\ninitrd /initramfs-old.img\noptions root=/dev/sda1\n\" >/efi/loader/entries/arch-old.conf
useradd -m -G wheel arch && chown -R arch:arch /etc/pacnix
echo \"%wheel ALL=(ALL:ALL) NOPASSWD: ALL\" >/etc/sudoers.d/wheel
install -d -o arch -g arch -m 700 /home/arch/.ssh && echo \"$key\" | install -o arch -g arch -m 600 /dev/stdin /home/arch/.ssh/authorized_keys
printf \"[Match]\nName=en*\n[Network]\nDHCP=yes\n\" >/etc/systemd/network/20-wired.network
systemctl enable -q sshd systemd-networkd systemd-resolved
printf \"\n[pacnix]\nServer = file:///srv/pacnix\n\" >>/etc/pacman.conf
pacman-key --add /srv/pacnix/test-key.asc >/dev/null 2>&1 && pacman-key --lsign-key \"pacnix test\" >/dev/null 2>&1
pacman -S --noconfirm pacnix >/dev/null 2>&1'
sudo test -s /mnt/efi/EFI/Linux/arch-linux.efi
sudo ln -sf /run/systemd/resolve/stub-resolv.conf /mnt/etc/resolv.conf" || fail "the installation by hand failed"
on ukilive 'sudo gpgconf --homedir /mnt/etc/pacman.d/gnupg --kill gpg-agent && sudo umount -R /mnt && sudo cryptsetup close root && sudo systemctl poweroff' || true
while virsh -c qemu:///system domstate pacnix-ukilive 2>/dev/null | grep -q running; do sleep 2; done
mv "$cache/pacnix-ukilive-vdb.qcow2" "$cache/ukibox-disk.qcow2"
"$vm" down ukilive

step "boot the UKI that the user made, and the plan refuses while /boot is not a mount point"
started+=(ukibox)
"$vm" boot ukibox "$cache/ukibox-disk.qcow2"
uuid=$(on ukibox 'sudo cryptsetup luksUUID /dev/vda2')
params="\"rd.luks.name=$uuid=root\" \"rd.luks.key=$uuid=/etc/cryptsetup-keys.d/root.key\" \"rd.luks.options=discard\""
scenario ukibox "{ boot.kernelParams = [ $params ]; }"
out=$(plan ukibox) && fail "the plan accepts /boot without the EFI system partition"
has "$out" '• Error: /boot is not a mount point, and pacnix writes files below it only on the declared vfat file system. Mount it, then run pacnix again. A change of /etc/fstab mounts it only at the next boot' || { echo "$out"; fail "the refusal does not tell to mount /boot"; }

step "mount the EFI system partition at /boot, and the apply builds the UKI with the complete command line"
on ukibox 'sudo umount /efi && sudo mount --mkdir /dev/disk/by-label/ESP /boot'
out=$(plan ukibox) || { echo "$out"; fail "plan failed"; }
for p in /etc/mkinitcpio.d/linux.preset /etc/kernel/cmdline /boot/loader/loader.conf; do
	has "$out" "Changing file $p (pacnix keeps the original" || { echo "$out"; fail "the plan does not keep the original of $p"; }
done
changes "$out" | grep -q '^Run trigger initramfs: ' || { echo "$out"; fail "the plan does not build the initramfs"; }
apply ukibox >/dev/null || fail "apply failed"
cmdline="root=/dev/mapper/root rw rootflags=subvol=/@ rd.luks.name=$uuid=root rd.luks.key=$uuid=/etc/cryptsetup-keys.d/root.key rd.luks.options=discard"
uki() { on ukibox "sudo bootctl list --json=short | jq -r '.[] | select(.id == \"arch-linux.efi\") | .options'"; }
[[ $(uki) == "$cmdline" ]] || { uki; fail "the UKI does not have the complete command line"; }
check ukibox 'sudo grep -qx "default arch-linux.efi" /boot/loader/loader.conf && sudo test -e /boot/loader/entries/arch-old.conf' "loader.conf is wrong, or the unmanaged entry is gone"
check ukibox 'sudo test ! -e /boot/initramfs-linux.img && sudo test ! -e /boot/loader/entries/linux.conf' "an image or entry of the other format exists"
check ukibox '! systemctl is-enabled -q systemd-tpm2-clear.service && test "$(sudo cat /sys/class/tpm/tpm0/ppi/request)" = 0' "the apply enabled or started a unit that the configuration does not declare, which asks the firmware to clear the TPM"
empty_plan ukibox

step "reboot into the UKI"
"$vm" reboot ukibox
check ukibox 'test "$(systemctl is-system-running)" = running' "the system is not healthy after the reboot"
# The firmware of the VM adds console parameters through SMBIOS.
[[ $(on ukibox 'cat /proc/cmdline') == "$cmdline "* ]] || { on ukibox 'cat /proc/cmdline'; fail "the booted command line is not the declared one"; }
check ukibox 'test "$(sudo bootctl status 2>/dev/null | awk "/Current Entry:/ { print \$3 }")" = arch-linux.efi' "the system does not boot the UKI"
check ukibox 'test "$(findmnt -no FSTYPE /boot)" = vfat && test "$(findmnt -no FSTYPE,OPTIONS / | grep -o "^btrfs .*subvol=/@")"' "the mounts are wrong"
empty_plan ukibox

step "a change of the command line and a kernel installation build the UKI again"
scenario ukibox "{ boot.kernelParams = [ $params \"pacnix.test=1\" ]; }"
apply ukibox >/dev/null || fail "apply failed"
[[ $(uki) == "$cmdline pacnix.test=1" ]] || { uki; fail "the UKI does not have the new command line"; }
on ukibox 'sudo touch -d 2000-01-01 /boot/EFI/Linux/arch-linux.efi && sudo pacman -S --noconfirm linux >/dev/null 2>&1' || fail "the kernel installation failed"
check ukibox 'sudo test /boot/EFI/Linux/arch-linux.efi -nt /etc/kernel/cmdline' "the kernel installation does not build the UKI"
[[ $(uki) == "$cmdline pacnix.test=1" ]] || { uki; fail "the UKI of the kernel installation does not have the command line"; }
empty_plan ukibox

step "change to entries"
scenario ukibox "{ boot.kernelParams = [ $params ]; boot.loader.systemd-boot.uki = false; }"
apply ukibox >/dev/null || fail "apply failed"
check ukibox 'sudo test ! -e /boot/EFI/Linux/arch-linux.efi && sudo test -s /boot/initramfs-linux.img && sudo grep -qx "default linux.conf" /boot/loader/loader.conf' "the UKI or the image is wrong"
"$vm" reboot ukibox
check ukibox 'test "$(sudo bootctl status 2>/dev/null | awk "/Current Entry:/ { print \$3 }")" = linux.conf' "the system does not boot the entry"
[[ $(on ukibox 'cat /proc/cmdline') == *" $cmdline"* ]] || { on ukibox 'cat /proc/cmdline'; fail "the booted command line of the entry is wrong"; }
empty_plan ukibox
pass
