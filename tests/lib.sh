# Helpers for the scenario scripts. Source this file.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
vm=$root/tests/vm
name=$(basename "$0" .sh)
# The host of a scenario is s and its number, with the configuration hosts/generic.nix.
h=s${name%%-*}
# A scenario sets size=(vCPUs MiB VMs) before it sources this file; tests/run
# reads the same line to fit scenarios on the host.
[[ -v size ]] || size=(2 3072 1)
export PACNIX_VM_CPUS=${size[0]} PACNIX_VM_MEM=${size[1]}
# The VMs of a scenario are deleted when it exits, unless PACNIX_VM_KEEP is set.
started=()
trap '[[ -n ${PACNIX_VM_KEEP:-} ]] || for v in "${started[@]}"; do "$vm" down "$v"; done' EXIT

step() { printf '\n== %s: %s\n' "$name" "$*"; }
on() { local h=$1; shift; "$vm" ssh "$h" "$@"; }
# Writes /etc/pacnix/scenario.nix on a host. An empty argument removes it.
scenario() {
	if [[ -z $2 ]]; then on "$1" 'sudo rm -f /etc/pacnix/scenario.nix'; else on "$1" "sudo tee /etc/pacnix/scenario.nix >/dev/null" <<<"$2"; fi
}
noise="^warning: not writing modified lock\\|^• \\|^    '\\(path\\|github\\):\\|^    follows \\|^copying path"
# Runs a pacnix verb on a host. Its exit status is the status of pacnix.
pacnix() { local h=$1; shift; on "$h" "sudo pacnix $* 2>&1 | sudo tee -a /var/log/pacnix-test.log | grep -v \"$noise\"; exit \${PIPESTATUS[0]}"; }
plan() { pacnix "$1" plan; }
apply() { local h=$1; shift; pacnix "$h" apply --yes "$@"; }
# Declared packages of the active generation, sorted.
declared() { on "$1" "jq -r '.packages.repo[], .packages.aur[], (.packages.local|keys[])' /nix/var/nix/profiles/pacnix | sort -u"; }
# Fails unless the explicit packages equal the declared packages and no orphan exists.
packages_match() {
	local d e
	d=$(declared "$1") e=$(on "$1" 'pacman -Qqe | sort')
	if [[ $d != "$e" ]]; then
		diff <(echo "$d") <(echo "$e") || true
		fail "explicit packages of $1 differ from the declaration"
	fi
	if [[ -n $(on "$1" 'pacman -Qqdtt' || true) ]]; then fail "$1 has orphan packages"; fi
}
# Fails unless the plan of a host is empty.
empty_plan() {
	local out
	out=$(plan "$1")
	if ! grep -qx 'No changes.' <<<"$out"; then
		printf '%s\n' "$out"
		fail "the plan of $1 is not empty"
	fi
}
check() { if ! on "$1" "$2" >/dev/null 2>&1; then fail "$3"; fi; }
refute() { if on "$1" "$2" >/dev/null 2>&1; then fail "$3"; fi; }
fail() { printf 'FAIL %s: %s\n' "$name" "$*"; exit 1; }
pass() { printf 'PASS %s\n' "$name"; }
cache=${PACNIX_VM_CACHE:-$HOME/.cache/pacnix-vm}
# Starts a fresh VM with the current package and config. The default disk is
# the snapshot that scenario 1 leaves after a bootstrap.
fresh() {
	local base=${2:-}
	if [[ -z $base && -f $cache/base-boot.qcow2 ]]; then base=$cache/base-boot.qcow2; fi
	started+=("$1")
	"$vm" up "$1" "$base" >/dev/null
	"$vm" push "$1" >/dev/null
}
