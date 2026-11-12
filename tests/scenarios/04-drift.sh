#!/usr/bin/env bash
# Scenario 4: manual changes. Plan and audit show each one. Apply corrects what
# pacnix owns.
source "$(dirname "$0")/../lib.sh"
fresh $h
scenario $h '{ features.demo.enable = true; }'
apply $h >/dev/null || fail "first apply failed"

step "change the system by hand"
on $h 'sudo pacman -S --noconfirm tree >/dev/null && echo "net.core.somaxconn = 1" | sudo tee -a /etc/sysctl.d/90-demo.conf >/dev/null &&
	sudo systemctl enable --now systemd-time-wait-sync.service 2>/dev/null; sudo touch /etc/stray.conf'
out=$(plan $h)
echo "$out"
for l in 'mark package as dependency tree' 'remove package tree' 'write file /etc/sysctl.d/90-demo.conf (correct drift)' 'disable and stop unit systemd-time-wait-sync.service' 'run trigger sysctl: sysctl --system >/dev/null'; do
	grep -qxF "$l" <<<"$out" || fail "the plan does not show: $l"
done
out=$(pacnix $h audit)
for l in 'explicit package not declared: tree' 'managed file differs from the generation: /etc/sysctl.d/90-demo.conf' 'enabled unit not declared and not enabled by a vendor preset: systemd-time-wait-sync.service' 'unowned file: /etc/stray.conf'; do
	grep -qxF "$l" <<<"$out" || { echo "$out"; fail "the audit does not show: $l"; }
done

step "apply corrects what pacnix owns"
apply $h || fail "apply failed"
refute $h "pacman -Q tree" "tree is still installed"
refute $h "grep -q 'somaxconn = 1$' /etc/sysctl.d/90-demo.conf" "the managed file is not restored"
refute $h "systemctl is-enabled systemd-time-wait-sync.service" "the undeclared unit is still enabled"
check $h "test \$(sysctl -n net.core.somaxconn) = 8192" "the trigger did not reapply sysctl"
empty_plan $h
pacnix $h audit | grep -qxF 'unowned file: /etc/stray.conf' || fail "the audit does not show the stray file after apply"
pass
