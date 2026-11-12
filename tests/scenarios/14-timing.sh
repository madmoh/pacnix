#!/usr/bin/env bash
# Time cost of pacnix over plain pacman: a no-op apply, and one package added and
# removed through pacnix and through pacman. Prints seconds; the packages are in
# the pacman cache for both runs, so downloads do not count.
size=(4 4096 1)
source "$(dirname "$0")/../lib.sh"
fresh $h
apply $h >/dev/null || fail "first apply failed"
t() { local s; s=$(date +%s.%N); "$@" >/dev/null || fail "$*"; echo "$(date +%s.%N) - $s" | bc; }
on $h 'sudo pacman -Sw --noconfirm gimp >/dev/null'
step "measure"
noop=$(t apply $h)
plan_s=$(t plan $h)
pac_add=$(t on $h 'sudo pacman -S --noconfirm gimp')
pac_rm=$(t on $h 'sudo pacman -Rns --noconfirm gimp')
scenario $h '{ packages.repo = [ "gimp" ]; }'
nix_add=$(t apply $h)
scenario $h ''
nix_rm=$(t apply $h)
printf 'no-op apply %.1f s, plan %.1f s\n' "$noop" "$plan_s"
printf 'add gimp: pacman %.1f s, pacnix apply %.1f s\n' "$pac_add" "$nix_add"
printf 'remove gimp: pacman %.1f s, pacnix apply %.1f s\n' "$pac_rm" "$nix_rm"
pass
