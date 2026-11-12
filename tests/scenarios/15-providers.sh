#!/usr/bin/env bash
# Scenario 15: chosen providers of a dependency, from the repositories and from
# the AUR. pacman would take jack2 for jack, and aurutils would build libpdfium.
source "$(dirname "$0")/../lib.sh"
fresh $h
apply $h >/dev/null || fail "first apply failed"
step "choose providers"
scenario $h '{
  packages.repo = [ "a2jmidid" ];
  packages.aur = [ "pdf-inspector-ocr" ];
  packages.providers = { jack = "pipewire-jack"; libpdfium = "pdfium-binaries-v8-bin"; };
}'
apply $h >/dev/null || fail "apply failed"
check $h 'pacman -Qi pipewire-jack | grep -q "^Install Reason *: Installed as a dependency"' "pipewire-jack is not a dependency"
check $h 'pacman -Qi pdfium-binaries-v8-bin | grep -q "^Install Reason *: Installed as a dependency"' "pdfium-binaries-v8-bin is not a dependency"
refute $h 'pacman -Q jack2' "pacman installed jack2"
refute $h 'ls /var/lib/pacnix/repo/libpdfium-[0-9]*' "aurutils built libpdfium"
packages_match $h
empty_plan $h
step "refuse a provider that does not provide the dependency"
scenario $h '{ packages.repo = [ "a2jmidid" ]; packages.providers.jack = "htop"; }'
out=$(apply $h) && fail "apply accepted a wrong provider"
grep -q 'htop (it does not provide jack)' <<<"$out" || { echo "$out"; fail "wrong error for a wrong provider"; }
step "remove the packages"
scenario $h ''
apply $h >/dev/null || fail "apply failed"
refute $h 'pacman -Q pipewire-jack' "the provider stays after its dependent goes"
refute $h 'pacman -Q pdfium-binaries-v8-bin' "the AUR provider stays after its dependent goes"
packages_match $h
pass
