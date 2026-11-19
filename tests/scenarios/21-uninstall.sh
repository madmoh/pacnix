#!/usr/bin/env bash
# Scenario 21: pacnix uninstall. A plain Arch system gets pacnix, the feature
# demo with claimed data, a home configuration and an AUR package, and an
# upgrade. pacnix uninstall --yes, as the user, keeps the declared files, the
# enabled units, the declared packages and the content of the home files. No
# path, package, user, group or key of pacnix, aurutils and Nix remains, and
# pacman -Qkk shows no new change to the files of the packages.
size=(2 3072 1)
source "$(dirname "$0")/../lib.sh"
export LC_ALL=C
started+=($h)
"$vm" up $h "$cache/cloud-20261101.620452.qcow2" >/dev/null
step "a plain Arch system"
on $h 'sudo pacman -Syu --noconfirm rsync >/dev/null' || fail "the upgrade of the plain system failed"
"$vm" reboot $h
paths() { on $h 'sudo find / -xdev ! -type s | LC_ALL=C sort'; }
accounts() { on $h 'cut -d: -f1 /etc/passwd; sed "s/:.*//; s/^/group /" /etc/group' | LC_ALL=C sort; }
# The user trusts the key of [pacnix] with the steps of the README, and it stays.
repo_key=$(gpg --with-colons --show-keys "$cache/pkg/test-key.asc" 2>/dev/null | awk -F: '/^fpr/ { print $10; exit }')
keys() { on $h 'sudo gpg --homedir /etc/pacman.d/gnupg --list-keys --with-colons 2>/dev/null | grep ^fpr' | grep -v "$repo_key" | LC_ALL=C sort; }
# pacman writes to standard output and standard error; one pipe for both can mix their lines.
# The test harness adds the repository [pacnix] to /etc/pacman.conf.
changed() { on $h 'LC_ALL=C pacman -Qkk >/tmp/qkk.out 2>/tmp/qkk.err; cat /tmp/qkk.*' | grep -E "checksum mismatch|Size mismatch|type mismatch|No such file" | grep -v ' /etc/pacman.conf (' | LC_ALL=C sort; }
plain=$(paths) plain_accounts=$(accounts) plain_keys=$(keys) plain_changed=$(changed)

step "install pacnix, apply a feature with data, a home configuration and an AUR package, and upgrade"
"$vm" push $h >/dev/null
scenario $h '{ features.demo.enable = true; features.home.enable = true; packages.aur = [ "downgrade" ]; }'
apply $h >/dev/null || fail "apply failed"
on $h 'mkdir -p ~/.cache/demo && curl -fs -o ~/.cache/demo/page localhost:8081' || fail "the demo service does not answer"
pacnix $h apply --yes >/dev/null || fail "upgrade failed"
list=$(on $h 'jq -r ".files|keys[]" /nix/var/nix/profiles/pacnix')
contents() { on $h "for f in $(echo $list); do if [ -L \$f ]; then echo \"\$f -> \$(readlink \$f)\"; else sudo sha256sum \$f; fi; done"; }
home() { on $h 'cd ~ && sha256sum .config/demo/settings.conf .config/systemd/user/hello.service .config/environment.d/10-home-manager.conf'; }
units() { on $h 'systemctl list-unit-files --state=enabled --no-legend | cut -d" " -f1 | sort'; }
files=$(contents) home_files=$(home) enabled=$(units) packages=$(declared $h | grep -vx pacnix)
hm=$(on $h 'cd "$(readlink -f ~/.local/state/home-manager/gcroots/current-home)/home-files" && find . -mindepth 1 | sed "s|^\.|$HOME|"')

step "uninstall"
out=$(pacnix $h uninstall --yes) || { echo "$out" | tail -20; fail "uninstall failed"; }
grep '^  Remove\|^  Delete' <<<"$out"
for p in pacnix aurutils nix devtools; do
	matches "$out" "Remove [0-9]+ packages: .*\b$p\b" || fail "the question does not list the removal of $p"
done
has "$out" 'Delete the data of removed feature pacnix: /nix, /var/lib/pacnix, /var/lib/aurbuild, ' || fail "the question does not list the data of pacnix"
matches "$out" 'Delete the users .*\bpacnix\b.* and the groups pacnix nixbld\.' || fail "the question does not list the users and groups"

step "the system stays as configured"
[[ $(contents) == "$files" ]] || fail "a declared file changed"
[[ $(units) == "$(grep -vx nix-daemon.socket <<<"$enabled")" ]] || { diff <(echo "$enabled") <(units); fail "the enabled units changed"; }
[[ $(on $h 'pacman -Qqe | sort') == "$packages" ]] || fail "the explicit packages are not the declared packages"
refute $h 'pacman -Qqdtt | grep -q .' "orphan packages remain"
[[ $(home) == "$home_files" ]] || fail "the content of a home file changed"
refute $h 'find ~ -lname "/nix/*" | grep -q .' "a link into the Nix store remains in the home"
refute $h 'sudo find /home/arch -user root | grep -q .' "a file in the home belongs to root"
check $h 'curl -fs localhost:8081' "the demo service does not answer"

step "nothing of pacnix, aurutils and Nix remains"
refute $h 'pacman -Qq pacnix aurutils nix 2>/dev/null | grep -q .' "pacnix, aurutils or nix is installed"
[[ $(keys) == "$plain_keys" ]] || fail "the keys of the pacman keyring changed"
created=$(on $h "awk '\$1 == \"u\" || \$1 == \"g\" { print \$2; print \"group \" \$2 }' /usr/lib/sysusers.d/*.conf" | sort -u)
new=$(comm -13 <(echo "$plain_accounts") <(accounts) | comm -23 - <(echo "$created"))
[[ -z $new ]] || fail "users or groups remain that no installed package creates: $new"
new=$(comm -13 <(echo "$plain_changed") <(changed) | grep -vF "$(printf ' %s (\n' $list)" || true)
[[ -z $new ]] || fail "pacman -Qkk shows new changes: $new"
owned=$(on $h 'pacman -Qlq | sed "s|/\$||" | LC_ALL=C sort -u')
explained=$( (for f in $list; do while [[ $f != / ]]; do echo "$f"; f=$(dirname "$f"); done; done; echo "$hm") | LC_ALL=C sort -u)
noise='^(/etc/pacnix|/srv/pacnix|/home/arch/(config|repo)|/var/lib/pacman/(local|sync/pacnix\.db(\.sig)?)|/var/log|/var/cache|/var/lib/systemd|/var/tmp/systemd-private-[^/]*|/etc/systemd/system/[^/]*\.wants|/etc/systemd/system/autovt@tty1\.service|/usr/share/info/dir|/usr/lib/gio/modules/giomodule\.cache|/srv/demo|/home/arch/\.(cache/demo|config/(dconf|Code)))(/|$)'
new=$(comm -13 <(echo "$plain") <(paths) | comm -23 - <(echo "$owned") | comm -23 - <(echo "$explained") | grep -Ev "$noise" || true)
[[ -z $new ]] || fail "paths remain that no package, declaration or home file explains: $new"
pass
