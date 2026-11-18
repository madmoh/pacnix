#!/usr/bin/env bash
# Scenario 27: pacnix touches only what the configuration declares. On a used
# system (the cloud image, with a package that the user installed, an orphan
# and the units of the image), a configuration with one package gives a plan
# with only that package, and the apply changes nothing else. When the
# declaration goes, the package that pacnix installed goes, and the other
# packages and the orphan of before stay. A declared package that the user
# installed before stays when its declaration goes. A declared unit goes back
# to its vendor preset when its declaration goes, and the other units stay.
source "$(dirname "$0")/../lib.sh"
fresh $h "$cache/base-20261101.620452.qcow2"
on $h 'sudo pacman -S --noconfirm tree >/dev/null 2>&1 && sudo pacman -S --noconfirm --asdeps lsof >/dev/null 2>&1 && sudo touch /var/tmp/marker' || fail "the changes by hand failed"
# The state that the configuration does not declare: packages and their install reason, enabled units, and changed files in /etc and /boot.
state='pacman -Qqe | grep -vx cowsay; echo ==; pacman -Qqdtt; echo ==; systemctl list-unit-files --state=enabled --no-legend | sort; echo ==; sudo find /etc /boot -newer /var/tmp/marker \( -type f -o -type l \) -not -path "/etc/pacnix*" -not -path /etc/ld.so.cache | sort'
before=$(on $h "$state")
# The test harness declares the host name.
host='{ lib, ... }: { networking.hostName = lib.mkForce null; }'
with() { scenario $h "{ lib, ... }: { networking.hostName = lib.mkForce null; $1 }"; }

step "a configuration with one package gives a plan with only that package"
with 'packages.repo = [ "cowsay" ];'
out=$(plan $h) || { echo "$out"; fail "plan failed"; }
[[ $(grep -v '^Generation ' <<<"$out") == "install package cowsay" ]] || { echo "$out"; fail "the plan has more than the declared package"; }
out=$(apply $h --offline) || { echo "$out" | tail -20; fail "apply failed"; }
check $h 'pacman -Qqe cowsay' "the declared package is not installed"
[[ $before == "$(on $h "$state")" ]] || { diff <(echo "$before") <(on $h "$state"); fail "the apply changed what the configuration does not declare"; }
check $h 'test ! -e /etc/systemd/system-preset/00-pacnix.preset' "the apply wrote a preset file without declared units"
empty_plan $h

step "when the declaration goes, only what pacnix installed goes"
scenario $h "$host"
out=$(plan $h)
[[ $(grep -v '^Generation ' <<<"$out") == $'mark package as dependency cowsay\nremove package cowsay' ]] || { echo "$out"; fail "the plan does not remove only the package that pacnix installed"; }
apply $h --offline >/dev/null || fail "apply failed"
refute $h 'pacman -Q cowsay' "the package that pacnix installed stays"
[[ $before == "$(on $h "$state")" ]] || { diff <(echo "$before") <(on $h "$state"); fail "the removal changed what the configuration did not declare"; }
empty_plan $h

step "a package that the user installed stays when its declaration goes"
with 'packages.repo = [ "tree" ];'
empty_plan $h
apply $h --offline >/dev/null || fail "apply failed"
scenario $h "$host"
empty_plan $h
apply $h --offline >/dev/null || fail "apply failed"
check $h 'pacman -Qqe tree' "the package that the user installed is not explicitly installed"

step "a declared unit goes back to its vendor preset when its declaration goes"
with 'units.enable = [ "fstrim.timer" ];'
out=$(plan $h)
[[ $(grep -v '^Generation ' <<<"$out") == "enable and start unit fstrim.timer" ]] || { echo "$out"; fail "the plan has more than the declared unit"; }
apply $h --offline >/dev/null || fail "apply failed"
check $h 'systemctl is-enabled fstrim.timer' "the declared unit is not enabled"
scenario $h "$host"
out=$(plan $h)
[[ $(grep -v '^Generation ' <<<"$out") == "disable and stop unit fstrim.timer" ]] || { echo "$out"; fail "the plan does not give the unit back to its vendor preset"; }
apply $h --offline >/dev/null || fail "apply failed"
[[ $before == "$(on $h "$state")" ]] || { diff <(echo "$before") <(on $h "$state"); fail "the unit changes changed what the configuration did not declare"; }
empty_plan $h
pass
