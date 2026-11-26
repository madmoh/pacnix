#!/usr/bin/env bash
# Scenario 6: pacnix apply of a host from the image of 2026-10-01, with a
# kernel update, DKMS builds from the repositories and the AUR, .pacnew files
# and a reboot. The summary of the upgrade tells to reboot for the new kernel.
# A system upgrade that replaces pacnix hands the apply to the new pacnix,
# which evaluates the configuration again and completes the same apply. An
# apply with --no-config upgrades with the active generation.
size=(4 4096 1)
source "$(dirname "$0")/../lib.sh"
old=$cache/base-20261001.604814.qcow2
[[ -f $old ]] || "$vm" base "$cache/cloud-20261001.604814.qcow2" 2026/10/01 >/dev/null
# A later pacnix package, with modules that declare /etc/pacnix-next: a copy
# of the source with its own build directory, built while the VM upgrades.
next=$(mktemp -d -p "$cache")
(
	cd "$root" && cp -a Cargo.toml Cargo.lock PKGBUILD LICENSE README.md pacnix.asc src nix pkg "$next/" && cd "$next" &&
	sed -i '0,/^version = /s/^version = .*/version = "0.99.0"/' Cargo.toml && sed -i '/^name = "pacnix"$/{n;s/.*/version = "0.99.0"/}' Cargo.lock &&
	sed -i 's/^pkgver=.*/pkgver=0.99.0/' PKGBUILD && sed -i 's|^  options = {|  config.files."/etc/pacnix-next".text = "0.99.0";\n  options = {|' nix/modules/default.nix &&
	BUILDDIR=$next/build PKGDEST=$next PACNIX_KEY=$cache/pkg/test-key.asc makepkg -df --noconfirm >"$next/log" 2>&1 &&
	export GNUPGHOME=$cache/gnupg && key=$(gpg --list-secret-keys --with-colons | awk -F: '/^fpr/ { print $10; exit }') &&
	gpg --batch --detach-sign --local-user "$key" pacnix-0.99.0-1-x86_64.pkg.tar.zst &&
	cp "$cache"/pkg/pacnix.db.tar.gz . && repo-add -q --sign --key "$key" pacnix.db.tar.gz pacnix-0.99.0-1-x86_64.pkg.tar.zst
) &
build=$!
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
# --no-config upgrades with the active generation: a later edit of the flake waits for the next apply.
generation=$(on old 'readlink /nix/var/nix/profiles/pacnix')
scenario old "{ files.\"/etc/pacman.d/mirrorlist\".text = \"$servers\"; features.overrides.enable = true; files.\"/etc/pending\".text = \"x\"; }"
out=$(pacnix old apply --no-config) || { echo "$out" | tail -20; fail "upgrade failed"; }
refute old 'test -e /etc/pending' "apply --no-config applied the edit of the flake"
check old "test \"\$(readlink /nix/var/nix/profiles/pacnix)\" = $generation" "apply --no-config changed the generation"
check old 'test -z "$(ls -A /var/lib/pacnix/cache 2>/dev/null)"' "apply --no-config did not empty the staging cache"
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

step "the system upgrade replaces pacnix: the new pacnix continues the same apply"
wait $build || { tail -20 "$next/log"; fail "the later pacnix package did not build"; }
pkg=pacnix-0.99.0-1-x86_64.pkg.tar.zst
for f in $pkg $pkg.sig pacnix.db.tar.gz pacnix.db.tar.gz.sig; do on old "cat >/tmp/$f" <"$next/$f"; done
rm -rf "$next"
on old "cd /tmp && sudo mv $pkg $pkg.sig pacnix.db.tar.gz pacnix.db.tar.gz.sig /srv/pacnix/"
# A package file of an earlier run in the staging cache, which only an apply without a failure deletes.
on old 'sudo mkdir -p /var/lib/pacnix/cache && sudo touch /var/lib/pacnix/cache/old-1-1-any.pkg.tar.zst'
log=$(on old 'stat -c %s /var/log/pacman.log')
# A second pacnix while the new pacnix continues: the lock refuses it.
on old "sudo systemd-run --quiet sh -c 'until grep -q \"continues the apply\" /var/log/pacnix-test.log; do sleep 0.1; done; pacnix apply --no-upgrade --no-config >/var/tmp/parallel 2>&1'"
out=$(apply old) || { echo "$out" | tail -30; fail "the apply that upgrades pacnix failed"; }
once "$out" "• pacnix was upgraded. The new version continues the apply." || { echo "$out"; fail "the old pacnix did not hand off"; }
once "$out" "• pacnix 0.99.0 continues the apply." || { echo "$out"; fail "the new pacnix did not continue the apply"; }
has "$out" "package upgraded: pacnix" || { echo "$out"; fail "the summary does not show the upgrade of pacnix from before the handoff"; }
check old 'pacman -Q pacnix | grep -q " 0.99.0-1$"' "pacnix 0.99.0 is not installed"
check old 'test "$(cat /etc/pacnix-next)" = 0.99.0' "the new pacnix did not evaluate the configuration with its modules"
check old 'test -z "$(ls -A /var/lib/pacnix/cache)"' "the new pacnix did not empty the staging cache"
syncs=$(on old "tail -c +$((log + 1)) /var/log/pacman.log | grep -c \"Running 'pacman -Sy --config /run/pacnix/pacman.conf'\"" || true)
upgrades=$(on old "tail -c +$((log + 1)) /var/log/pacman.log | grep -c \"Running 'pacman -Su \"" || true)
[[ $syncs == 1 && $upgrades == 1 ]] || fail "the apply ran $syncs database refreshes and $upgrades system upgrades, not one of each"
check old 'grep -q "a different pacnix process is running" /var/tmp/parallel' "a second pacnix ran while the new pacnix continued the apply"
out=$(apply old) || { echo "$out" | tail -20; fail "the no-op apply failed"; }
has "$out" "continues the apply" && fail "the no-op apply handed off"
out=$(apply old --no-upgrade) || { echo "$out" | tail -20; fail "apply --no-upgrade failed"; }
has "$out" "continues the apply" && fail "apply --no-upgrade handed off"
empty_plan old
pass
