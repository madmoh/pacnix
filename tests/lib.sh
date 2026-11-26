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
# The log of pacnix in each VM, with times, goes to the end of the log of the scenario.
vmlog() { for v in "${started[@]}"; do echo "== log of pacnix in $v"; "$vm" ssh "$v" 'sudo cat /var/log/pacnix-test.log' 2>/dev/null || true; done; }
trap 'vmlog; [[ -n ${PACNIX_VM_KEEP:-} ]] || for v in "${started[@]}"; do "$vm" down "$v"; done' EXIT
trap 'exit 143' TERM

# The profile of a scenario: each step shows the seconds since the start, and
# each command in a VM that takes 5 s or more shows its time.
step() { printf '\n== %s: %s (at %d s, %s)\n' "$name" "$*" "$SECONDS" "$(date +%T)"; }
on() {
	local h=$1 s=$SECONDS rc=0 c
	shift
	"$vm" ssh "$h" "$@" || rc=$?
	c="$*"
	((SECONDS - s < 5)) || printf 'time %d s: %s: %s\n' $((SECONDS - s)) "$h" "${c:0:120}" >&2
	return $rc
}
# Writes /etc/pacnix/scenario.nix on a host. An empty argument removes it.
scenario() {
	if [[ -z $2 ]]; then on "$1" 'sudo rm -f /etc/pacnix/scenario.nix'; else on "$1" "sudo tee /etc/pacnix/scenario.nix >/dev/null" <<<"$2"; fi
}
noise="^copying path"
# Runs a pacnix verb on a host. Its exit status is the status of pacnix.
# The login shell of a host can be bash or zsh: zsh keeps the statuses in pipestatus.
# The log in the VM has the time of each line.
pacnix() { local h=$1; shift; on "$h" "pacnix $* 2>&1 | awk '{ print strftime(\"%T\"), \$0; fflush() }' | sudo tee -a /var/log/pacnix-test.log | cut -d' ' -f2- | grep -v \"$noise\"; exit \${PIPESTATUS[0]:-\${pipestatus[1]}}"; }
# The plan of a host: apply without the upgrade and the configuration changes nothing.
plan() { local h=$1; shift; pacnix "$h" apply --no-upgrade --no-config "$@"; }
apply() { local h=$1; shift; pacnix "$h" apply "$@"; }
# Declared packages of the active generation, sorted.
declared() { on "$1" "jq -r '.packages.repo[], .packages.aur[], (.packages.local|keys[])' /nix/var/nix/profiles/pacnix | sort -u"; }
# Fails unless each declared package is explicitly installed and no orphan exists.
packages_match() {
	local d e
	d=$(declared "$1") e=$(on "$1" 'pacman -Qqe | sort')
	if [[ -n $(comm -23 <(echo "$d") <(echo "$e")) ]]; then
		comm -23 <(echo "$d") <(echo "$e")
		fail "declared packages of $1 are not explicitly installed"
	fi
	if [[ -n $(on "$1" 'pacman -Qqdtt' || true) ]]; then fail "$1 has orphan packages"; fi
}
# The output as one line, with each run of spaces and line breaks as one space,
# so that a text matches across the line breaks of pacnix.
flat() { tr -d '\r' <<<"$1" | tr -s ' \n' '  '; }
has() { flat "$1" | grep -qF -- "$2"; }
# Whether the output has a text exactly one time.
once() { (($(flat "$1" | grep -oF -- "$2" | wc -l) == 1)); }
matches() { flat "$1" | grep -qE -- "$2"; }
# The change lines of a plan: the lines indented by two spaces under "• Planned changes".
changes() { awk '/^• Planned changes/ { c = 1; next } /^• / { c = 0 } c && /^  [^ ]/ { print substr($0, 3) }' <<<"$1"; }
# Whether the audit output lists an item under a kind: audit_has <output> <kind> <item>.
# The item is the whole line or the path without the note in parentheses after it.
audit_has() {
	awk -v k="$2" -v i="$3" '
		function check() { path = item; sub(/ \(.*\)$/, "", path); if (kind == k && (item == i || path == i)) found = 1 }
		/^  [^ ]/ { check(); item = ""; kind = substr($0, 3); sub(/ \([0-9]+\):$/, "", kind); next }
		/^    [^ ]/ { check(); item = substr($0, 5); next }
		{ check(); item = "" }
		END { check(); exit !found }' <<<"$1"
}
# The names, sizes and times of the files in the pacman cache of the system:
# pacnix downloads into /var/lib/pacnix/cache, and an apply without a failure
# leaves it empty. cache_kept <host> <list> fails when the cache changed.
cache_list() { on "$1" 'sudo find /var/cache/pacman/pkg -printf "%P %s %T@\n" | sort | md5sum'; }
cache_kept() {
	[[ $(cache_list "$1") == "$2" ]] || fail "pacnix changed the files in /var/cache/pacman/pkg"
	check "$1" 'test -z "$(sudo ls -A /var/lib/pacnix/cache)"' "files stay in /var/lib/pacnix/cache after an apply without a failure"
}
# Fails unless the plan of a host is empty.
empty_plan() {
	local out
	out=$(plan "$1")
	if grep -q '^• Planned changes' <<<"$out"; then
		printf '%s\n' "$out"
		fail "the plan of $1 is not empty"
	fi
}
# Adds packages to the record of the packages that pacnix made, as on a host
# where pacnix installed them: made <host> <package>...
made() { local h=$1; shift; on "$h" "sudo jq -c '. + \$ARGS.positional | unique' --args $* </var/lib/pacnix/packages.json | sudo tee /var/lib/pacnix/packages.json.new >/dev/null && sudo mv /var/lib/pacnix/packages.json.new /var/lib/pacnix/packages.json"; }
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
# Starts fresh VMs from one base disk at the same time: fresh_all <base> <name>...
fresh_all() {
	local base=$1 pids=() p
	shift
	started+=("$@")
	for v in "$@"; do { "$vm" up "$v" "$base" >/dev/null && "$vm" push "$v" >/dev/null; } & pids+=($!); done
	for p in "${pids[@]}"; do wait "$p" || fail "a VM did not start"; done
}
