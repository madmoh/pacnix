#!/usr/bin/env bash
# Scenario 26: Secure Boot and a TPM2 key with a PIN. A live system installs by
# hand an encrypted root (LUKS with a passphrase) and systemd-boot with a UKI,
# an old entry with a kernel and an EFI tool. The firmware has Secure Boot in
# Setup Mode and the VM has a TPM 2.0. The first apply creates the keys and signs
# the boot files, also the kernel of the old entry and the declared tool. The
# default enrolls the Microsoft keys. Without db additions, sbctl refuses the
# keys because the firmware loads an option ROM, and the firmware stays in Setup
# Mode; the output shows the failure one time, with the next step. With the
# hashes of the TPM event log, the apply enrolls the keys and the TPM2 key
# waits. The apply asks for the passphrase and shows the recovery key one time
# at the start, before the upgrade, until the user types written; the end of the
# input stops the apply before the upgrade and keeps the enrolled key. A new plan
# after the upgrade asks nothing. Without a terminal, the recovery key waits.
# After a reboot, the firmware checks the signatures, and the
# apply asks for the passphrase and the PIN, enrolls the TPM2 key with the PIN
# and a recovery key. The system unlocks with the PIN. After a kernel update,
# a systemd-boot update and new Secure Boot keys, it still boots; the new keys
# change PCR 7, and the apply enrolls the TPM2 key again. A file that sbctl
# cannot sign and the removal of Secure Boot while the firmware checks the
# signatures are refused before any change. A declared PCR that cannot be read,
# that has no measurement (PCR 23) or that has no measurement event in the log
# makes the TPM2 key wait, without a question and without a change, and the
# wait is not a change: the key stays, and a device without a key gets none.
# After a logged measurement, the key seals to the values that pacnix checked
# and unseals. A TPM2 key without a PIN on PCRs 0 and 7 unlocks with no
# prompt. Without the TPM2 declaration, the TPM2 key that pacnix enrolled goes, and sbctl stops signing the files that are not boot
# files of the configuration. A TPM2 key that the user enrolls by hand and a
# file that the user saves in sbctl stay.
size=(4 4096 1)
source "$(dirname "$0")/../lib.sh"
fresh sblive "$cache/base-20261101.620452.qcow2"
"$vm" blank sblive 8G
key=$(cat "$cache/id_ed25519.pub")

step "install by hand: LUKS with a passphrase and a UKI"
on sblive "set -e
for i in \$(seq 30); do test -b /dev/vdb && break; sleep 1; done
printf 'label: gpt\n,512M,U\n,,L\n' | sudo sfdisk -q /dev/vdb
sudo mkfs.fat -F32 -n ESP /dev/vdb1 >/dev/null
printf pass1 | sudo cryptsetup luksFormat -q --type luks2 --pbkdf pbkdf2 --pbkdf-force-iterations 1000 /dev/vdb2 -
printf pass1 | sudo cryptsetup open /dev/vdb2 root -
sudo mkfs.ext4 -qL root /dev/mapper/root
sudo mount /dev/mapper/root /mnt && sudo mount --mkdir /dev/vdb1 /mnt/boot
sudo pacstrap -K /mnt base linux openssh sudo zsh git systemd-ukify >/dev/null 2>&1
sudo genfstab -U /mnt | sudo tee -a /mnt/etc/fstab >/dev/null
uuid=\$(sudo cryptsetup luksUUID /dev/vdb2)
sudo cp -a /srv/pacnix /mnt/srv/ && sudo install -Dm644 /etc/xdg/nix/nix.conf /mnt/etc/xdg/nix/nix.conf && sudo cp -a /etc/pacnix /mnt/etc/
sudo arch-chroot /mnt sh -ec '
echo sbbox >/etc/hostname
printf \"HOOKS=(base systemd autodetect microcode modconf kms keyboard block sd-encrypt filesystems fsck)\n\" >/etc/mkinitcpio.conf.d/10-encrypt.conf
echo \"root UUID='\$uuid' none\" >/etc/crypttab.initramfs
echo \"root=/dev/mapper/root rw console=tty0 console=ttyS0\" >/etc/kernel/cmdline
printf \"ALL_kver=/boot/vmlinuz-linux\nPRESETS=(default)\ndefault_uki=/boot/EFI/Linux/arch-linux.efi\n\" >/etc/mkinitcpio.d/linux.preset
mkinitcpio -P >/dev/null 2>&1
bootctl install >/dev/null 2>&1
cp /boot/vmlinuz-linux /boot/vmlinuz-old
printf \"title Old system\nlinux /vmlinuz-old\noptions root=/dev/mapper/root rw\n\" >/boot/loader/entries/old.conf
install -D /usr/lib/systemd/boot/efi/systemd-bootx64.efi /boot/EFI/tools/tool.efi
useradd -m -G wheel arch && chown -R arch:arch /etc/pacnix
echo \"%wheel ALL=(ALL:ALL) NOPASSWD: ALL\" >/etc/sudoers.d/wheel
install -d -o arch -g arch -m 700 /home/arch/.ssh && echo \"$key\" | install -o arch -g arch -m 600 /dev/stdin /home/arch/.ssh/authorized_keys
printf \"[Match]\nName=en*\n[Network]\nDHCP=yes\n\" >/etc/systemd/network/20-wired.network
systemctl enable -q sshd systemd-networkd systemd-resolved
printf \"\n[pacnix]\nServer = file:///srv/pacnix\n\" >>/etc/pacman.conf
pacman-key --add /srv/pacnix/test-key.asc >/dev/null 2>&1 && pacman-key --lsign-key \"pacnix test\" >/dev/null 2>&1
pacman -S --noconfirm pacnix >/dev/null 2>&1'
sudo ln -sf /run/systemd/resolve/stub-resolv.conf /mnt/etc/resolv.conf" || fail "the installation by hand failed"
on sblive 'sudo gpgconf --homedir /mnt/etc/pacman.d/gnupg --kill gpg-agent; sudo umount -R /mnt && sudo cryptsetup close root && sudo systemctl poweroff' || true
while virsh -c qemu:///system domstate pacnix-sblive 2>/dev/null | grep -q running; do sleep 2; done
mv "$cache/pacnix-sblive-vdb.qcow2" "$cache/sbbox-disk.qcow2"
"$vm" down sblive

# Reboots and types the answers at the prompts of the initramfs: prompt, text, prompt, text...
reboot_with() {
	on sbbox 'sudo systemctl reboot' || true
	"$vm" answer sbbox "$@" || fail "the boot does not show a prompt"
	"$vm" wait sbbox >/dev/null || fail "the system does not boot"
}
# Runs pacnix apply in a terminal, which types the lines of the arguments at its prompts.
tty_apply() { printf '%s\n' "$@" | "$vm" ssh sbbox -tt 'NO_COLOR=1 pacnix apply 2>&1' | tr -d '\r'; }
# Fails unless an apply asked for the passphrase and showed the recovery key one
# time, before the system upgrade, and asked nothing after it.
order() {
	once "$1" '• the recovery key of LUKS device root:' || { echo "$1"; fail "the apply does not show the recovery key one time"; }
	at() { grep -n -m1 -- "$1" <<<"$2" | cut -d: -f1; }
	(($(at 'Passphrase of the LUKS device root' "$1") < $(at '• the recovery key of LUKS device root:' "$1") && $(at '• the recovery key of LUKS device root:' "$1") < $(at '^• \(Refreshing package databases\|Upgrading the system\|system upgrade\)' "$1"))) || { echo "$1"; fail "the questions and the recovery key do not come before the system upgrade"; }
	sed -n '/^• \(Refreshing package databases\|Upgrading the system\|system upgrade\)/,$p' <<<"$1" | grep -q 'Passphrase\|PIN\|Type written' && { echo "$1"; fail "the apply asks after the system upgrade"; }
	return 0
}
# Fails unless sbctl has the given number of files and each is signed.
signed() { check sbbox "sudo sbctl list-files --json | jq -e 'length == $1 and all(.is_signed)'" "$2"; }
secure_boot() { on sbbox 'bootctl status 2>/dev/null | sed -n "s/^ *Secure Boot: //p" | head -1'; }

step "boot in Setup Mode, and the first apply creates, signs and enrolls the keys"
started+=(sbbox)
PACNIX_VM_SECUREBOOT=1 PACNIX_VM_NOWAIT=1 "$vm" boot sbbox "$cache/sbbox-disk.qcow2"
"$vm" answer sbbox "Please enter" pass1 || fail "the first boot does not ask for the passphrase"
"$vm" wait sbbox >/dev/null
[[ $(secure_boot) == "disabled (setup)" ]] || fail "the firmware is not in Setup Mode: $(secure_boot)"
uuid=$(on sbbox 'sudo cryptsetup luksUUID /dev/vda2')
luks="boot.initrd.luks.devices.root = { device = \"UUID=$uuid\"; tpm2 = { enable = true; pin = true; }; recoveryKey = true; };"
scenario sbbox "{ boot.loader.systemd-boot.secureBoot = { enable = true; extraFiles = [ \"/boot/EFI/tools/tool.efi\" ]; }; $luks }"
out=$(plan sbbox) || { echo "$out"; fail "plan failed"; }
for l in 'create the Secure Boot keys of this host with sbctl' 'sign boot file /boot/EFI/Linux/arch-linux.efi' 'sign boot file /boot/vmlinuz-old' 'sign boot file /boot/EFI/tools/tool.efi' \
	'sign boot file /usr/lib/systemd/boot/efi/systemd-bootx64.efi as /usr/lib/systemd/boot/efi/systemd-bootx64.efi.signed' 'enroll the Secure Boot keys of this host in the firmware, with microsoft'; do
	has "$out" "$l" || { echo "$out"; fail "the plan does not show: $l"; }
done
scenario sbbox "{ boot.loader.systemd-boot.secureBoot = { enable = true; dbAdditions = [ ]; extraFiles = [ \"/boot/EFI/tools/tool.efi\" ]; }; $luks }"
rc=0
out=$(apply sbbox) || rc=$?
echo "$out" | tail -12
((rc == 4)) || fail "the exit status is $rc, not 4"
once "$out" '• error: enroll the Secure Boot keys of this host in the firmware:' || fail "the output does not show the failed enrollment one time"
once "$out" 'Found OptionROM in the bootchain' || fail "the output does not show the message of sbctl one time"
has "$out" '• the firmware stays in Setup Mode. The firmware loads option ROMs. Add "tpm-eventlog" or "microsoft" to boot.loader.systemd-boot.secureBoot.dbAdditions, then run pacnix apply' || fail "the summary does not tell the next step"
has "$out" '• apply done, but 1 step failed.' || fail "the summary does not count the failed step"
grep -q '^• failed step' <<<"$out" && fail "the summary lists the failed step again"
[[ $(secure_boot) == "disabled (setup)" ]] || fail "the firmware is not in Setup Mode after the refusal: $(secure_boot)"
sb='boot.loader.systemd-boot.secureBoot = { enable = true; dbAdditions = [ "tpm-eventlog" ]; extraFiles = [ "/boot/EFI/tools/tool.efi" ]; };'
scenario sbbox "{ $sb $luks }"
out=$(plan sbbox) || { echo "$out"; fail "plan failed"; }
signed 6 "the boot files are not signed"
for l in 'enroll the Secure Boot keys of this host in the firmware, with tpm-eventlog' 'note: the TPM2 key of LUKS device root waits: the firmware does not check Secure Boot signatures with the keys of this host yet'; do
	has "$out" "$l" || { echo "$out"; fail "the plan does not show: $l"; }
done
# As on a real machine, the user runs the apply in a terminal: it asks for the passphrase for the
# recovery key, and continues after the user types written.
out=$(tty_apply pass1 written)
echo "$out" | tail -8
matches "$out" '• the recovery key of LUKS device root: [a-z]{8}-' || fail "the apply does not show the recovery key"
order "$out"
has "$out" 'recovery key enrolled: root' || fail "the last line does not show the recovery key"
has "$out" '• the firmware has the Secure Boot keys of this host. Reboot. If sbctl status then shows that Secure Boot is disabled, turn on Secure Boot in the firmware' || { echo "$out" | tail; fail "the summary does not tell to reboot"; }
signed 6 "the boot files are not signed"
check sbbox 'grep -qx "root UUID=.* none tpm2-device=auto" /etc/crypttab.initramfs' "crypttab.initramfs does not have the TPM2 option"
check sbbox 'sudo systemd-cryptenroll /dev/vda2 | grep -q recovery' "the recovery key is not enrolled"

step "the end of the input before written stops the apply, and the recovery key stays"
on sbbox 'sudo env PASSWORD=pass1 systemd-cryptenroll --wipe-slot=recovery /dev/vda2 >/dev/null 2>&1' || fail "the removal of the recovery key by hand failed"
rc=0
out=$(on sbbox "printf 'pass1\n' | script -qec 'NO_COLOR=1 pacnix apply' /dev/null" 2>&1 | tr -d '\r') || rc=$?
echo "$out" | tail -8
matches "$out" '• the recovery key of LUKS device root: [a-z]{8}-' || fail "the apply does not show the recovery key"
has "$out" '• error: the input ended before you typed written. The recovery key above is enrolled.' || fail "the apply does not stop at the end of the input"
grep -q 'apply done\|^• \(Refreshing package databases\|Upgrading the system\|system upgrade\)' <<<"$out" && fail "the apply continued without the confirmation"
check sbbox 'sudo systemd-cryptenroll /dev/vda2 | grep -q recovery' "the end of the input removed the recovery key"

step "a new plan after the upgrade does not ask again or show a second recovery key"
wipe_recovery='sudo env PASSWORD=pass1 systemd-cryptenroll --wipe-slot=recovery /dev/vda2 >/dev/null 2>&1'
on sbbox "$wipe_recovery" || fail "the removal of the recovery key by hand failed"
# An old sync database makes the upgrade download it again, thus pacnix computes the plan again.
on sbbox 'sudo touch -d 2000-01-01 /var/lib/pacman/sync/core.db'
out=$(tty_apply pass1 written)
order "$out"
check sbbox 'test "$(stat -c %Y /var/lib/pacman/sync/core.db)" -gt 946771200' "the upgrade did not download the sync database again"
check sbbox 'test "$(sudo systemd-cryptenroll /dev/vda2 | grep -c recovery)" = 1' "the device does not have one recovery key"
has "$out" 'apply done.' || { echo "$out" | tail; fail "the apply after the new plan failed"; }

step "without a terminal, the recovery key waits with a note"
on sbbox "$wipe_recovery" || fail "the removal of the recovery key by hand failed"
out=$(apply sbbox) || { echo "$out" | tail; fail "the apply without a terminal failed"; }
has "$out" '• a recovery key for LUKS device root waits. Run pacnix apply in a terminal to get it' || { echo "$out" | tail; fail "the apply does not tell how to get the recovery key"; }
grep -q 'recovery key of LUKS device root:' <<<"$out" && fail "the apply without a terminal shows a recovery key"
refute sbbox 'sudo systemd-cryptenroll /dev/vda2 | grep -q recovery' "the apply without a terminal enrolled a recovery key"
tty_apply pass1 written >/dev/null
check sbbox 'sudo systemd-cryptenroll /dev/vda2 | grep -q recovery' "the recovery key is not enrolled"

step "the firmware checks the signatures, and the apply enrolls the TPM2 key with the PIN"
reboot_with "Please enter" pass1
[[ $(secure_boot) == "enabled (user)" ]] || fail "Secure Boot is not on after the enrollment: $(secure_boot)"
out=$(pacnix sbbox audit) || true
grep -qx '• Secure Boot: on, with the keys of this host' <<<"$out" || { echo "$out"; fail "the audit does not show the Secure Boot state"; }
grep -e sbctl -e 'without a signature' <<<"$out" && fail "the audit shows a file of sbctl or a boot file without a signature"
out=$(plan sbbox)
has "$out" 'enroll the TPM2 key of LUKS device root: the device has none' || { echo "$out"; fail "the plan does not enroll the TPM2 key"; }
grep -q 'recovery' <<<"$out" && { echo "$out"; fail "the plan enrolls a second recovery key"; }
tty_apply pass1 4321 4321 | tail -4
check sbbox 'sudo systemd-cryptenroll /dev/vda2 | grep -q "tpm2" && sudo systemd-cryptenroll /dev/vda2 | grep -q recovery && sudo cryptsetup luksDump --dump-json-metadata /dev/vda2 | grep -q "\"tpm2-pin\":true"' "the TPM2 key with the PIN or the recovery key is not enrolled"
empty_plan sbbox

step "the system unlocks with the PIN, also after a kernel update"
reboot_with "LUKS2 token PIN" 4321
on sbbox 'sudo pacman -S --noconfirm linux >/dev/null 2>&1' || fail "the kernel update failed"
reboot_with "LUKS2 token PIN" 4321
[[ $(secure_boot) == "enabled (user)" ]] || fail "Secure Boot is not on after the kernel update"
signed 6 "a boot file is not signed after the kernel update"

step "a systemd-boot update installs the signed systemd-boot"
# The signed systemd-boot of an older version, in /usr/lib and on the EFI system partition.
on sbbox 'set -e; major=$(pacman -Q systemd | sed "s/.* \([0-9]*\)\..*/\1/")
sed "s/systemd-boot $major\./systemd-boot $((major - 1))./" /usr/lib/systemd/boot/efi/systemd-bootx64.efi >/tmp/old.efi
for f in /usr/lib/systemd/boot/efi/systemd-bootx64.efi.signed /boot/EFI/systemd/systemd-bootx64.efi; do sudo rm $f && sudo sbctl sign -o $f /tmp/old.efi >/dev/null; done
sudo pacman -S --noconfirm systemd >/dev/null 2>&1' || fail "the systemd update failed"
version='LoaderInfo: systemd-boot $(pacman -Q systemd | cut -d" " -f2)-arch'
check sbbox "sudo grep -aq \"$version\" /usr/lib/systemd/boot/efi/systemd-bootx64.efi.signed" "sbctl did not sign the new systemd-boot"
signed 6 "a boot file is not signed after the systemd update"
# systemd-boot-update.service installs the new systemd-boot at the update or at the next boot.
reboot_with "LUKS2 token PIN" 4321
check sbbox "sudo grep -aq \"$version\" /boot/EFI/systemd/systemd-bootx64.efi" "the new systemd-boot is not on the EFI system partition"
signed 6 "a boot file is not signed after the systemd-boot update"
reboot_with "LUKS2 token PIN" 4321
check sbbox 'bootctl status 2>/dev/null | grep -q "Product: systemd-boot $(pacman -Q systemd | cut -d" " -f2)"' "the updated systemd-boot does not run"
empty_plan sbbox

step "new Secure Boot keys change PCR 7, and the apply enrolls the TPM2 key again"
on sbbox 'sudo sbctl rotate-keys >/dev/null' || fail "sbctl rotate-keys failed"
# After a wrong PIN, systemd-cryptsetup asks for the PIN again, then for the passphrase.
reboot_with "PIN" 4321 "passphrase or recovery key" pass1
out=$(plan sbbox)
has "$out" 'enroll the TPM2 key of LUKS device root: the values of the PCRs changed' || { echo "$out"; fail "the plan does not enroll the TPM2 key again"; }
tty_apply pass1 4321 4321 >/dev/null
check sbbox 'test "$(sudo systemd-cryptenroll /dev/vda2 | grep -c tpm2)" = 1' "the device does not have one TPM2 key"
reboot_with "LUKS2 token PIN" 4321
empty_plan sbbox

step "a file that sbctl cannot sign and the removal of Secure Boot are refused before any change"
state='sudo find /boot /var/lib/sbctl -type f -exec md5sum {} + | sort; sudo find /etc -newer /var/tmp/marker | grep -v "^/etc/pacnix" | sort; pacman -Q | md5sum'
on sbbox 'echo text | sudo tee /boot/not-efi.txt >/dev/null && sudo touch /var/tmp/marker'
before=$(on sbbox "$state")
scenario sbbox "{ boot.loader.systemd-boot.secureBoot = { enable = true; extraFiles = [ \"/boot/EFI/tools/tool.efi\" \"/boot/not-efi.txt\" ]; }; $luks }"
out=$(apply sbbox) && fail "the apply signed a file that is not an EFI binary"
has "$out" '• error: /boot/not-efi.txt is not an EFI binary, thus sbctl cannot sign it. Remove it from boot.loader.systemd-boot.secureBoot.extraFiles' || { echo "$out"; fail "the refusal does not name the file"; }
grep -q 'system upgrade' <<<"$out" && fail "the refusal came after the upgrade"
scenario sbbox "{ $luks }"
out=$(apply sbbox) && fail "the apply removed Secure Boot while the firmware checks the signatures"
has "$out" '• error: the firmware checks Secure Boot signatures, and without boot.loader.systemd-boot.secureBoot' || { echo "$out"; fail "the refusal does not tell to turn off Secure Boot"; }
[[ $before == "$(on sbbox "$state")" ]] || fail "a refused apply changed the system"

step "a declared PCR without a value or a measurement keeps the TPM2 key, and the key waits"
tokens='sudo cryptsetup luksDump --dump-json-metadata /dev/vda2 | jq -c .tokens'
pcrs() { scenario sbbox "{ $sb boot.initrd.luks.devices.root = { device = \"UUID=$uuid\"; tpm2.enable = true; tpm2.pcrs = [ $1 ]; recoveryKey = true; }; }"; }
pcrs '7 24'
out=$(plan sbbox)
has "$out" '• note: the TPM2 key of LUKS device root waits: pacnix cannot read PCR 24 of the SHA-256 bank of the TPM2 chip' || { echo "$out"; fail "the plan does not wait for a PCR that cannot be read"; }
check sbbox 'test "$(cat /sys/class/tpm/tpm0/pcr-sha256/23)" = 0000000000000000000000000000000000000000000000000000000000000000' "PCR 23 is not zero"
pcrs '7 23 7'
before=$(on sbbox "$tokens")
out=$(plan sbbox)
has "$out" '• note: the TPM2 key of LUKS device root waits: PCR 23 of the SHA-256 bank has no measurement for this boot' || { echo "$out"; fail "the plan does not wait for a PCR without a measurement"; }
grep -q '^• plan done: no change' <<<"$out" || { echo "$out"; fail "the plan counts the TPM2 key that waits as a change"; }
out=$(tty_apply)
echo "$out" | tail -4
has "$out" '• the TPM2 key of LUKS device root waits: PCR 23 of the SHA-256 bank has no measurement for this boot' || fail "the apply does not show that the TPM2 key waits"
has "$out" '• apply done. No changes. Exit status 0.' || fail "the apply counts the TPM2 key that waits as a change"
grep -q 'Passphrase' <<<"$out" && fail "the apply asks for the passphrase for a TPM2 key that waits"
[[ $(on sbbox "$tokens") == "$before" ]] || fail "the apply changed the TPM2 key while a PCR has no measurement"
on sbbox 'sudo env PASSWORD=pass1 systemd-cryptenroll --wipe-slot=tpm2 /dev/vda2 >/dev/null 2>&1' || fail "the removal of the TPM2 key by hand failed"
tty_apply >/dev/null
check sbbox "$tokens | jq -e 'all(.[]; .type != \"systemd-tpm2\")'" "the apply enrolled a TPM2 key while a PCR has no measurement"
# systemd-pcrextend measures into PCR 23 and writes the event to the log of systemd.
on sbbox 'sudo /usr/lib/systemd/systemd-pcrextend --pcr=23 pacnix-test' || fail "systemd-pcrextend failed"
out=$(plan sbbox)
has "$out" 'enroll the TPM2 key of LUKS device root: the device has none' || { echo "$out"; fail "the plan does not enroll the TPM2 key after the measurement"; }
tty_apply pass1 >/dev/null
check sbbox "$tokens | jq -e '[.[] | select(.type == \"systemd-tpm2\")] | length == 1 and .[0][\"tpm2-pcrs\"] == [7, 23] and .[0][\"tpm2-pcr-bank\"] == \"sha256\"'" "the TPM2 key is not sealed to PCRs 7 and 23 of the SHA-256 bank"
check sbbox 'sudo cryptsetup open --test-passphrase --token-only /dev/vda2' "the new TPM2 key does not unseal"
empty_plan sbbox
# Without the event in the log, the value of PCR 23 has no measurement event, also for a key that is current.
on sbbox 'sudo rm /run/log/systemd/tpm2-measure.log'
out=$(plan sbbox)
has "$out" '• note: the TPM2 key of LUKS device root waits: PCR 23 of the SHA-256 bank has no measurement event in the event log of this boot' || { echo "$out"; fail "the plan does not wait for a PCR without a measurement event"; }

step "a TPM2 key without a PIN on PCRs 0 and 7 unlocks with no prompt"
scenario sbbox "{ $sb boot.initrd.luks.devices.root = { device = \"UUID=$uuid\"; tpm2 = { enable = true; pcrs = [ 0 7 ]; }; recoveryKey = true; }; }"
out=$(plan sbbox)
has "$out" 'enroll the TPM2 key of LUKS device root: the PCRs or the PIN changed' || { echo "$out"; fail "the plan does not enroll the TPM2 key again"; }
tty_apply pass1 >/dev/null
check sbbox 'sudo cryptsetup luksDump --dump-json-metadata /dev/vda2 | jq -e "[.tokens[] | select(.type == \"systemd-tpm2\")] | length == 1 and .[0][\"tpm2-pcrs\"] == [0, 7] and .[0][\"tpm2-pin\"] != true"' "the TPM2 key without a PIN on PCRs 0 and 7 is not enrolled"
"$vm" reboot sbbox || fail "the system does not unlock with no prompt"
empty_plan sbbox

step "without the TPM2 declaration, the TPM2 key goes, and sbctl stops signing the files that are not boot files"
on sbbox 'sudo rm /boot/loader/entries/old.conf /boot/vmlinuz-old'
scenario sbbox "{ boot.loader.systemd-boot.secureBoot.enable = true; boot.initrd.luks.devices.root = { device = \"UUID=$uuid\"; recoveryKey = true; }; }"
out=$(plan sbbox)
for l in 'remove the TPM2 key of LUKS device root' 'stop signing file /boot/vmlinuz-old, which is not a boot file of the configuration' 'stop signing file /boot/EFI/tools/tool.efi, which is not a boot file of the configuration'; do
	has "$out" "$l" || { echo "$out"; fail "the plan does not show: $l"; }
done
out=$(apply sbbox) || { echo "$out" | tail -20; fail "the apply without TPM2 failed"; }
refute sbbox 'sudo systemd-cryptenroll /dev/vda2 | grep -q tpm2' "the TPM2 key stays"
signed 4 "sbctl did not stop signing the files that are not boot files"
check sbbox "sudo sbctl sign-all -g" "the pacman hook of sbctl fails"
reboot_with "Please enter" pass1
empty_plan sbbox

step "a TPM2 key and a signed file that the user added stay"
on sbbox 'sudo env PASSWORD=pass1 systemd-cryptenroll --tpm2-device=auto --tpm2-pcrs=7 /dev/vda2 >/dev/null 2>&1' || fail "the enrollment by hand failed"
on sbbox 'sudo install /usr/lib/systemd/boot/efi/systemd-bootx64.efi /boot/EFI/tools/hand.efi && sudo sbctl sign -s /boot/EFI/tools/hand.efi >/dev/null' || fail "the signature by hand failed"
empty_plan sbbox
apply sbbox >/dev/null || fail "apply failed"
check sbbox 'sudo sbctl list-files --json | jq -e "any(.file == \"/boot/EFI/tools/hand.efi\")"' "the file that the user saved in sbctl is gone"
pass
