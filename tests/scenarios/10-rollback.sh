#!/usr/bin/env bash
# Scenario 10: rollback to the previous generation.
source "$(dirname "$0")/../lib.sh"
fresh $h
apply $h >/dev/null || fail "first apply failed"
scenario $h '{ features.demo.enable = true; features.overrides.enable = true; }'
apply $h >/dev/null || fail "second apply failed"
first=$(on $h 'readlink /nix/var/nix/profiles/pacnix')
step "roll back"
pacnix $h rollback --yes || fail "rollback failed"
second=$(on $h 'readlink /nix/var/nix/profiles/pacnix')
[[ $first != "$second" ]] || fail "the profile did not change"
refute $h 'pacman -Q darkhttpd' "darkhttpd is still installed"
refute $h 'test -e /etc/systemd/system/demo-web.service' "the unit file of the feature remains"
refute $h 'grep -qx Color /etc/pacman.conf' "the pacman.conf override remains"
packages_match $h
step "the plan returns to the configuration"
plan $h | grep -qx 'install package darkhttpd' || fail "the plan does not restore the configuration"
apply $h >/dev/null || fail "apply after rollback failed"
check $h 'curl -fs localhost:8081' "the demo service does not answer"
empty_plan $h
pass
