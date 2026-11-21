#!/usr/bin/env bash
# Scenario 4: manual changes. The plan and apply correct only what pacnix owns,
# a declared file; a package and a unit that the configuration does not declare
# stay as the user made them, and the audit shows them. A file system that cannot keep the declared mode stops the apply
# before any change, instead of a drift that each apply corrects again.
source "$(dirname "$0")/../lib.sh"
fresh $h
scenario $h '{ features.demo.enable = true; }'
apply $h >/dev/null || fail "first apply failed"

step "change the system by hand"
on $h 'sudo pacman -S --noconfirm tree >/dev/null && echo "net.core.somaxconn = 1" | sudo tee -a /etc/sysctl.d/90-demo.conf >/dev/null &&
	sudo systemctl enable --now systemd-time-wait-sync.service 2>/dev/null; sudo touch /etc/stray.conf'
out=$(plan $h)
echo "$out"
for l in 'Changing file /etc/sysctl.d/90-demo.conf (it differs from the declaration)' 'Run trigger sysctl: sysctl --system >/dev/null'; do
	changes "$out" | grep -qxF "$l" || fail "the plan does not show: $l"
done
grep -e tree -e systemd-time-wait-sync <<<"$out" && fail "the plan changes a package or a unit that the configuration does not declare"
out=$(pacnix $h audit)
for l in 'Explicit package not declared|tree' 'Managed file differs from the generation|/etc/sysctl.d/90-demo.conf' 'Enabled unit not declared and not enabled by a vendor preset|systemd-time-wait-sync.service' 'Unowned file|/etc/stray.conf'; do
	audit_has "$out" "${l%%|*}" "${l#*|}" || { echo "$out"; fail "the audit does not show: $l"; }
done

step "apply corrects what pacnix owns"
apply $h || fail "apply failed"
check $h "pacman -Qe tree" "the package that the user installed is not explicitly installed"
refute $h "grep -q 'somaxconn = 1$' /etc/sysctl.d/90-demo.conf" "the managed file is not restored"
check $h "systemctl is-enabled systemd-time-wait-sync.service" "the unit that the user enabled is not enabled"
check $h "test \$(sysctl -n net.core.somaxconn) = 8192" "the trigger did not reapply sysctl"
empty_plan $h
audit_has "$(pacnix $h audit)" 'Unowned file' /etc/stray.conf || fail "the audit does not show the stray file after apply"

step "a vfat file system with a umask cannot keep the mode 0600"
scenario $h '{ features.demo.enable = true; packages.repo = [ "dosfstools" ]; }'
apply $h >/dev/null || fail "apply of dosfstools failed"
on $h 'sudo truncate -s 32M /var/tmp/esp.img && sudo mkfs.fat -F 32 /var/tmp/esp.img >/dev/null && sudo mkdir -p /mnt/esp && sudo mount -o umask=0077 /var/tmp/esp.img /mnt/esp'
esp() { scenario $h "{ features.demo.enable = true; packages.repo = [ \"dosfstools\" ]; files.\"/mnt/esp/loader/loader.conf\" = { text = \"timeout 3\\n\"; mode = \"$1\"; }; }"; }
esp 0600
out=$(apply $h) && { echo "$out"; fail "apply wrote a file that the file system cannot keep"; }
echo "$out" | tail -1
has "$out" '• Error: the vfat file system on /mnt/esp cannot keep the mode 0600 and the owner root:root of /mnt/esp/loader/loader.conf: a new file gets the mode 0700' || fail "the error does not name the file system and the mode"
grep -q 'fmask=0077' <<<"$out" || fail "the error does not show the mount options"
refute $h 'test -e /mnt/esp/loader' "apply changed the file system before the refusal"
esp 0700
apply $h >/dev/null || fail "apply of the mode that the file system gives failed"
empty_plan $h
pass
