#!/usr/bin/env bash
# Scenario 25: a step that fails does not stop the apply. The local package
# kg-a does not build: the package kg-b, which needs it, does not build, the
# independent package kg-c installs, and cowsay, which the configuration no
# longer declares, stays until each declared package builds. The trigger
# kgfail fails, thus the later trigger kgnext waits; the unit kg-fail.service
# does not start; the home activation fails. The files, the other units and
# the users of the configuration apply. The output shows each failure and a
# summary, the exit status is 4, and the plan shows the failed steps. After the
# corrections, the next apply does each failed step again; the unit, which the
# configuration no longer enables, does not start again; the plan is empty. A
# local package whose source host does not resolve fails after the last of four
# attempts, and the other packages install. When its declaration goes, its
# sources go.
source "$(dirname "$0")/../lib.sh"
fresh $h
scenario $h '{ packages.repo = [ "cowsay" ]; }'
apply $h >/dev/null || fail "first apply failed"

step "an apply with a failed build, trigger, unit and home activation"
on $h 'echo fail | sudo tee /etc/pacnix/packages/kg-a/mode >/dev/null && touch ~/.fail-activation'
scenario $h '{
  features.home.enable = true;
  packages.local = { kg-a = ./packages/kg-a; kg-b = ./packages/kg-b; kg-c = ./packages/kg-c; };
  files."/etc/kg.conf".text = "1\n";
  files."/etc/systemd/system/kg-fail.service".text = "[Service]\nType=oneshot\nRemainAfterExit=yes\nExecStart=/usr/bin/test -e /etc/kg-unit-ok\n[Install]\nWantedBy=multi-user.target\n";
  units.enable = [ "kg-fail.service" ];
  triggers.kgfail = { paths = [ "/etc/kg.conf" ]; command = "test -e /etc/kg-ok"; };
  triggers.kgnext = { paths = [ "/etc/kg.conf" ]; command = "touch /var/tmp/kgnext"; };
}'
rc=0
out=$(apply $h) || rc=$?
echo "$out" | tail -30
((rc == 4)) || fail "the exit status is $rc, not 4"
grep -q '^pacnix: error: build local package kg-a: aur chroot .* failed' <<<"$out" || fail "the output does not show the failed build"
grep -q '^ *error: the test build fails while the file mode has the word fail' <<<"$out" || fail "the output does not show the reason of the failed build"
grep -qx 'pacnix: error: build local package kg-b: it needs kg-a, which did not build' <<<"$out" || fail "the output does not show that kg-b needs kg-a"
grep -qx 'pacnix: error: trigger kgnext: it runs after trigger kgfail, which failed' <<<"$out" || fail "the trigger after the failed trigger did not wait"
grep -qx 'pacnix: the apply is done, but 7 steps failed. The next apply does them again:' <<<"$out" || fail "the summary does not count the failed steps"
for s in 'build local package kg-a' 'build local package kg-b' 'remove the packages that the configuration does not declare' 'trigger kgfail' 'trigger kgnext' 'systemctl start kg-fail.service' 'activate the home configuration of arch'; do
	grep -qxF "  $s" <<<"$out" || fail "the summary does not show: $s"
done
check $h 'pacman -Q kg-c dconf' "the independent packages are not installed"
refute $h 'pacman -Q kg-a || pacman -Q kg-b' "a package that did not build is installed"
check $h 'pacman -Q cowsay' "the removal did not wait for the package that did not build"
check $h 'test "$(cat /etc/kg.conf)" = 1' "the file is not written"
check $h 'systemctl is-enabled kg-fail.service' "the unit is not enabled"
refute $h 'test -e /var/tmp/kgnext' "the trigger after the failed trigger ran"
out=$(plan $h)
for l in 'build local package kg-a' 'build local package kg-b' 'install package kg-a' 'remove package cowsay' 'run trigger kgfail: test -e /etc/kg-ok' 'run trigger kgnext: touch /var/tmp/kgnext' \
	'run systemctl start kg-fail.service again (it failed at the last apply)' 'activate home configuration of arch: '; do
	grep -qF "$l" <<<"$out" || { echo "$out"; fail "the plan does not show: $l"; }
done

step "the next apply does the failed steps again"
on $h 'echo ok | sudo tee /etc/pacnix/packages/kg-a/mode >/dev/null && sudo touch /etc/kg-ok && rm ~/.fail-activation'
scenario $h '{
  features.home.enable = true;
  packages.local = { kg-a = ./packages/kg-a; kg-b = ./packages/kg-b; kg-c = ./packages/kg-c; };
  files."/etc/kg.conf".text = "1\n";
  files."/etc/systemd/system/kg-fail.service".text = "[Service]\nType=oneshot\nRemainAfterExit=yes\nExecStart=/usr/bin/test -e /etc/kg-unit-ok\n[Install]\nWantedBy=multi-user.target\n";
  triggers.kgfail = { paths = [ "/etc/kg.conf" ]; command = "test -e /etc/kg-ok"; };
  triggers.kgnext = { paths = [ "/etc/kg.conf" ]; command = "touch /var/tmp/kgnext"; };
}'
out=$(plan $h)
grep -q 'systemctl start kg-fail.service' <<<"$out" && { echo "$out"; fail "the plan starts a unit that the configuration does not enable"; }
out=$(apply $h) || { echo "$out" | tail -30; fail "the apply after the corrections failed"; }
grep -q 'steps failed' <<<"$out" && fail "the summary shows failed steps"
check $h 'pacman -Q kg-a kg-b kg-c' "the local packages are not installed"
refute $h 'pacman -Q cowsay' "cowsay stays"
check $h 'test -e /var/tmp/kgnext' "the trigger did not run again"
refute $h 'systemctl is-active kg-fail.service || systemctl is-enabled kg-fail.service' "the unit that the configuration does not enable is active or enabled"
check $h 'test "$(dconf read /org/gnome/desktop/interface/color-scheme)" = "'"'prefer-dark'"'"' "the home configuration is not active"
check $h 'test "$(sudo cat /var/lib/pacnix/retry.json)" = "[]"' "the record of failed steps is not empty"
packages_match $h
empty_plan $h

step "a source host that does not resolve fails the build after the last attempt"
scenario $h '{ packages.repo = [ "cowsay" ]; packages.local.kg-d = ./packages/kg-d; }'
rc=0
out=$(apply $h) || rc=$?
echo "$out" | tail -12
((rc == 4)) || fail "the exit status is $rc, not 4"
for n in 1 2 3; do
	grep -q "^pacnix: attempt $n of 4 failed in the network: curl: (6) Could not resolve host: kg.invalid" <<<"$out" || fail "the output does not show attempt $n"
done
grep -qx '  build local package kg-d' <<<"$out" || fail "the summary does not show the build that failed in the network"
check $h 'pacman -Q cowsay' "the other package is not installed"
check $h 'sudo test -d /var/lib/pacnix/build/sources/kg-d' "the sources of the failed build do not stay"
step "the sources go with the declaration"
scenario $h '{ packages.repo = [ "cowsay" ]; }'
apply $h >/dev/null || fail "the apply without kg-d failed"
refute $h 'sudo test -e /var/lib/pacnix/build/sources/kg-d' "the sources of a package that is not declared stay"
empty_plan $h
pass
