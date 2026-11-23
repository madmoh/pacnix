#!/usr/bin/env bash
# Scenario 11: a feature creates state, then the feature goes. The one question
# of the run offers to delete the data of the feature. The answer k keeps it,
# and the audit lists it with the feature that claimed it. --yes deletes it,
# except a declared file, a path that a feature of the configuration claims and
# a bash file.
# The audit of a home directory lists visible and hidden entries, the entries
# of ~/.config, ~/.cache and ~/.local with its children, and the siblings of a
# Home Manager file and of a claimed path. An unexplained directory is one
# entry, and a link is not followed. An XDG user directory, also a nested one
# or one with an absolute path, a path that audit.ignore matches, also below
# **, and an unchanged bash skeleton file are explained; a FIFO does not stop
# the audit. A claim explains a bash file.
# A system link to a file of a package, also through a second link, is
# explained, for example the fontconfig links in /etc/fonts/conf.d. A dangling
# link, a link to a file of no package and a link to a directory stay listed,
# and a .pacnew link too.
# The audit shows the paths that a feature claimed before first, also when the
# feature still exists and no longer claims the path, then the paths with no
# known origin, then the paths whose name is the same as an installed package,
# a program in /usr/bin or a desktop ID, also a hidden entry of the home
# directory without its dot, or that a package tmpfiles rule has. The names of the
# directories above a path do not count, and a removed application has no
# match. A bash file that feature arch claimed before is no former claim.
# A DKMS build of a package source for an installed kernel has a note: its
# registration, its build directory and the installed module with the same
# bytes. A version without source, a registration for a kernel that is not
# installed, and a module of no build or with other bytes have no note.
source "$(dirname "$0")/../lib.sh"
fresh $h
demo='{ features.demo.enable = true; }'
scenario $h "$demo"
apply $h >/dev/null || fail "apply failed"
on $h 'mkdir -p ~/.cache/demo/keep && curl -fs -o ~/.cache/demo/page localhost:8081'
step "the audit of a configured system"
out=$(pacnix $h audit) || fail "audit failed"
echo "$out"
grep -q '/srv/demo\|\.cache/demo' <<<"$out" && fail "the audit lists claimed paths"
step "remove the feature and keep its data"
scenario $h ''
out=$(on $h "printf 'k\n' | script -qec 'NO_COLOR=1 pacnix apply' /dev/null" 2>&1) || { echo "$out" | tail; fail "apply with the answer k failed"; }
has "$out" 'Delete the data of removed feature demo: /srv/demo, /home/arch/.cache/demo.' || { echo "$out" | tail; fail "the question does not show the data of the feature"; }
grep -qF 'Continue? [y/N] (N stops pacnix, k continues and keeps the data)' <<<"$out" || fail "the question does not offer to keep the data"
grep -q '^• The data of the removed features stays. pacnix audit shows it.' <<<"$out" || fail "the summary does not tell that the data stays"
check $h 'test -e /srv/demo/index.html && test -e ~/.cache/demo/page' "the answer k deleted the data"
out=$(pacnix $h audit) || fail "audit failed"
echo "$out"
matches "$out" 'Unowned directory \([0-9]+\):.* /srv/demo \(claimed by feature demo in generation [0-9]+; the feature is no longer enabled\)' || fail "the audit does not list /srv/demo with its origin"
matches "$out" 'Unclaimed home path \([0-9]+\):.* /home/arch/.cache/demo \(claimed by feature demo in generation [0-9]+; the feature is no longer enabled\)' || fail "the audit does not list ~/.cache/demo with its origin"
step "remove the feature again and delete its data"
scenario $h '{ features.demo.enable = true; claims.disposable.home = [ ".bash_history" ]; }'
on $h 'echo ls >~/.bash_history'
apply $h >/dev/null || fail "apply failed"
scenario $h '{ files."/srv/demo/keep.txt".text = "kept\n"; claims.other.home = [ ".cache/demo/keep" ]; claims.arch.home = [ ".bash_logout" ]; }'
apply $h || fail "apply --yes failed"
refute $h 'test -e /srv/demo/index.html || test -e ~/.cache/demo/page' "the data of the removed feature remains"
check $h 'test "$(cat /srv/demo/keep.txt)" = kept && test -d ~/.cache/demo/keep' "a declared file or a claimed path is deleted"
check $h 'test -s ~/.bash_history' "the deletion of the data of a removed feature deleted ~/.bash_history"
step "the audit of a home directory"
scenario $h '{ features.home.enable = true; packages.repo = [ "fontconfig" "linux-headers" "dkms" "v4l2loopback-dkms" ]; packages.local.desk = ./packages/desk; claims.other.home = [ ".var/app/org.demo/cache" ]; audit.ignore = [ "/home/arch/.cache/ignored*" "/home/arch/.local/lib/**/keep" ]; }'
apply $h >/dev/null || fail "apply failed"
on $h 'cd ~ && touch notes.txt .bash_history && mkdir -p .local/state/foo .local/bin .local/lib/x/deep .config/app/a .config/app/b Documents Music Work/Templates Work/Pictures Work/other .var/app/org.demo/cache .var/app/org.demo/data .var/app/other .var/app/org.pacnix.Desk .config/fontconfig .config/darkhttpd .desk-tool .cache/ignored-x &&
	touch .local/bin/tool .local/lib/x/deep/keep .local/lib/x/deep/other .config/demo/extra.conf && ln -s /etc .config/linkdir && cp /etc/skel/.bashrc /etc/skel/.bash_profile . && echo "# mine" >>.bash_profile && rm -f .bash_logout && mkfifo .bash_logout &&
	printf "XDG_DOCUMENTS_DIR=\"\$HOME/Documents\"\nXDG_DESKTOP_DIR=\"\$HOME\"\nXDG_MUSIC_DIR=\"\$HOME/\`echo Music\`\"\nXDG_TEMPLATES_DIR=\"\$HOME/Work/Templates\"\nXDG_PICTURES_DIR=\"/home/arch/Work/Pictures\"\n" >.config/user-dirs.dirs'
on $h 'cd /etc && sudo ln -s /usr/bin/pacman demo-pkg && sudo ln -s demo-pkg demo-chain && sudo ln -s /nonexistent demo-dangling && sudo touch demo-file && sudo ln -s demo-file demo-other && sudo ln -s /usr/share demo-dir && sudo ln -s /usr/bin/pacman demo.conf.pacnew && sudo touch /usr/share/fontconfig/demo-child && test -d /var/lib/desk-rule && test -L "$(ls -d /etc/fonts/conf.d/* | head -1)"' || fail "the fontconfig links are missing"
k=$(on $h 'ls /var/lib/dkms/v4l2loopback | sed -n "s/^kernel-\(.*\)-x86_64$/\1/p"')
v=$(on $h 'pacman -Q v4l2loopback-dkms | sed "s/.* \(.*\)-.*/\1/"')
d=/usr/lib/modules/$k/updates/dkms
on $h "test -f $d/v4l2loopback.ko.zst && test -d /var/lib/dkms/v4l2loopback/$v" || fail "DKMS did not install v4l2loopback $v for kernel $k"
on $h "cd /var/lib/dkms/v4l2loopback && sudo mkdir 0.0.1 && sudo ln -s /usr/src/v4l2loopback-0.0.1 0.0.1/source && sudo ln -s 0.0.1/9.9.9-arch1-1/x86_64 kernel-9.9.9-arch1-1-x86_64 &&
	sudo touch $d/stray.ko.zst && echo a | sudo tee $v/$k/x86_64/module/other.ko.zst >/dev/null && echo b | sudo tee $d/other.ko.zst >/dev/null" || fail "the stale DKMS paths cannot be made"
dkms="DKMS module v4l2loopback $v for kernel $k; the source is in package v4l2loopback-dkms"
# The number of items of a kind, in all blocks: count <output> <kind>.
count() { grep -o "^  $2 ([0-9]*)" <<<"$1" | tr -dc '0-9\n' | awk '{ s += $1 } END { print s + 0 }'; }
start=$(date +%s.%N)
out=$(pacnix $h audit) || { echo "$out"; fail "audit failed"; }
echo "COUNT home: $(count "$out" 'Unclaimed home path'), unowned files: $(count "$out" 'Unowned file'), unowned directories: $(count "$out" 'Unowned directory'), audit $(awk -v s="$start" -v e="$(date +%s.%N)" 'BEGIN { printf "%.2f", e - s }') s"
echo "$out"
for l in '/home/arch/.config/fontconfig (the name is the same as installed package fontconfig)' '/home/arch/.var/app/org.pacnix.Desk (the name is the same as desktop ID org.pacnix.Desk)' '/var/lib/desk-rule (a tmpfiles rule in /usr/lib/tmpfiles.d/desk.conf has this path)' '/etc/xdg/nix (the name is the same as installed package nix)' '/home/arch/.desk-tool (the name is the same as program /usr/bin/desk-tool of installed package desk)'; do
	grep -qxF "    $l" <<<"$out" || fail "the audit does not show: $l"
done
for l in "$d/v4l2loopback.ko.zst (the same bytes as the build of $dkms)" "/var/lib/dkms/v4l2loopback/kernel-$k-x86_64 (active registration of $dkms)" "/var/lib/dkms/v4l2loopback/$v (DKMS module v4l2loopback $v: the source is in package v4l2loopback-dkms, and each kernel directory has an active registration)" /var/lib/dkms/v4l2loopback/0.0.1 /var/lib/dkms/v4l2loopback/kernel-9.9.9-arch1-1-x86_64 $d/stray.ko.zst $d/other.ko.zst; do
	grep -qxF "    $l" <<<"$out" || fail "the audit does not show: $l"
done
for p in /home/arch/.config/darkhttpd /usr/share/fontconfig/demo-child /home/arch/.bash_logout; do
	grep -qxF "    $p" <<<"$out" || fail "the audit shows $p with a note"
done
awk '/^• Paths that a feature claimed/ { a = NR } /^• Paths with no known origin/ { b = NR } /^• Paths with a name, rule or DKMS match/ { c = NR } /^• Other items/ { d = NR } END { exit !(a && a < b && b < c && c < d) }' <<<"$out" || fail "the blocks of the audit are not in the order: former claims, no known origin, name matches, other items"
for p in demo-dangling demo-file demo-other demo-dir; do
	audit_has "$out" 'Unowned file' "/etc/$p" || fail "the audit does not list /etc/$p"
done
audit_has "$out" 'File .pacnew' /etc/demo.conf.pacnew || fail "the audit does not list /etc/demo.conf.pacnew"
grep -q '/etc/demo-pkg\|/etc/demo-chain\|/etc/fonts' <<<"$out" && fail "the audit lists a link to a file of a package"
for p in notes.txt .local/state/foo .local/bin/tool .local/lib/x/deep/other Work/other .config/demo/extra.conf .config/app .config/linkdir Music .bash_profile .bash_logout .var/app/org.demo/data .var/app/other; do
	audit_has "$out" 'Unclaimed home path' "/home/arch/$p" || fail "the audit does not list ~/$p"
done
audit_has "$out" 'Unclaimed home path' "/home/arch/.bash_history (claimed by feature disposable in generation 4; the feature is no longer enabled)" || fail "the audit does not list ~/.bash_history"
for p in .config/app/a .config/demo/settings.conf Documents Work Work/Templates Work/Pictures .local/lib .local/lib/x .local/lib/x/deep/keep .bashrc .cache/ignored-x .var/app/org.demo/cache .var .var/app .config/linkdir/passwd .config/Code; do
	audit_has "$out" 'Unclaimed home path' "/home/arch/$p" && fail "the audit lists ~/$p"
done
on $h "sudo rm -rf /var/lib/dkms/v4l2loopback/0.0.1 /var/lib/dkms/v4l2loopback/kernel-9.9.9-arch1-1-x86_64 $d/stray.ko.zst $d/other.ko.zst /var/lib/dkms/v4l2loopback/$v/$k/x86_64/module/other.ko.zst"
scenario $h '{ features.home.enable = true; claims.other.home = [ ".bash_history" ]; }'
apply $h >/dev/null || fail "apply failed"
out=$(pacnix $h audit)
echo "$out"
audit_has "$out" 'Unclaimed home path' /home/arch/.bash_history && fail "the audit lists a claimed bash file"
matches "$out" '/home/arch/.var/app/org.demo/cache \(claimed by feature other in generation [0-9]+; the feature no longer claims it\)' || fail "the audit does not show the path that feature other no longer claims"
audit_has "$out" 'Unclaimed home path' /home/arch/.var && fail "the audit shows ~/.var as one entry"
pass
