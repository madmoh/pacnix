#!/usr/bin/env bash
# Scenario 11: a feature creates state, then the feature goes. Apply keeps the
# data of the feature and tells that the audit shows it. The audit lists it
# last, numbered, with the feature that claimed it, and asks which leftovers to
# delete: at the end of the input nothing goes, a number deletes one leftover,
# and the others show again in the next audit. The rm of the interactive shell
# of the owner deletes, thus an alias of rm applies. The answer a deletes all,
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
# explained, for example the fontconfig links in /etc/fonts/conf.d. A link to a
# file of no package and a link to a directory stay listed with their target,
# and a .pacnew link too. A dangling link is a leftover. The mask of a unit
# that units.mask declares is explained, and another mask has a note.
# The audit shows the paths that nothing owns in one list, each with a clue
# when it finds one: the name is the same as an installed package, a program in /usr/bin or a
# desktop ID, also a hidden entry of the home directory without its dot, or a
# package tmpfiles rule has the path, or the name agrees with an installed
# package without its packaging parts, a GNOME Shell extension, a uv tool, a
# home claim or the last part of a reverse-DNS ID, or the path is in a
# directory of one or two packages, or a directory holds such a name. Then the
# other differences, and last the leftovers: the paths that a feature claimed
# before, also when the feature still exists and no longer claims the path. The
# names of the directories above a path do not count. A claim of the
# configuration and an audit.ignore pattern that match no path show, the claims
# of the pacnix modules do not.
# When apply removes a package, the audit offers what it left as leftovers:
# the paths of its tmpfiles rules and unit directories, also in a home
# directory, the files of its system user, its .pacsave files and the paths
# with the name of the package, a program or a desktop ID. A rule for a path
# that an installed package owns makes no leftovers below it. When the leftovers
# go, the record of the package goes too. An application that was removed
# before it made a path has no match.
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
step "remove the feature: apply keeps its data, and the audit deletes the selected leftovers"
scenario $h ''
out=$(pacnix $h audit <<<a) || { echo "$out" | tail; fail "audit before the apply failed"; }
has "$out" '• The configuration differs from the active configuration. Run pacnix apply, then pacnix audit can delete the leftovers.' || { echo "$out" | tail; fail "the audit offers to delete data of the active configuration"; }
check $h 'test -e /srv/demo/index.html && test -e ~/.cache/demo/page' "the audit deleted data of the active configuration"
out=$(apply $h) || { echo "$out" | tail; fail "apply failed"; }
has "$out" 'Keep the data of removed feature demo for pacnix audit: /srv/demo, /home/arch/.cache/demo' || { echo "$out" | tail; fail "the plan does not show the data of the feature"; }
grep -q '^• The data of the removed packages and features stays. pacnix audit shows it and deletes it when you ask.' <<<"$out" || fail "the summary does not tell that the data stays"
check $h 'test -e /srv/demo/index.html && test -e ~/.cache/demo/page' "apply deleted the data"
out=$(pacnix $h audit) || fail "audit failed"
echo "$out"
has "$out" '• Leftovers of removed packages and features, and broken links (2): 1 /home/arch/.cache/demo (claimed by feature demo in generation' || fail "the audit does not list ~/.cache/demo first with its origin"
matches "$out" '2 /srv/demo \(claimed by feature demo in generation [0-9]+; the feature is no longer enabled\)' || fail "the audit does not list /srv/demo with its origin"
has "$out" 'Delete leftovers? [a]ll, [N]one, or numbers such as 1 3-5:' || fail "the audit does not ask which leftovers to delete"
check $h 'test -e /srv/demo/index.html && test -e ~/.cache/demo/page' "the audit deleted data at the end of the input"
out=$(pacnix $h audit <<<'1 3') && fail "the audit accepted a number that is not in the list"
has "$out" '• Error: 3 is not a number or a range of the list, thus nothing is deleted. Run pacnix audit again' || { echo "$out" | tail; fail "wrong error for a number that is not in the list"; }
check $h 'test -e /srv/demo/index.html && test -e ~/.cache/demo/page' "the audit deleted data at an answer with an error"
out=$(pacnix $h audit <<<2) || { echo "$out" | tail; fail "audit with the answer 2 failed"; }
has "$out" ', 1 deleted.' || { echo "$out" | tail; fail "the last line does not count the deleted leftover"; }
refute $h 'test -e /srv/demo' "the answer 2 did not delete /srv/demo"
check $h 'test -e ~/.cache/demo/page' "the answer 2 deleted ~/.cache/demo"
has "$(pacnix $h audit)" '• Leftovers of removed packages and features, and broken links (1): 1 /home/arch/.cache/demo' || fail "the next audit does not show the leftover that stays"
on $h "printf '%s\n' 'trash() { shift 2; mkdir -p /tmp/trash && mv -t /tmp/trash -- \"\$@\"; } # probe' 'alias rm=trash # probe' >>~/.\${SHELL##*/}rc"
pacnix $h audit <<<a >/dev/null || fail "audit with the answer a failed"
refute $h 'test -e ~/.cache/demo' "the answer a did not delete ~/.cache/demo"
check $h 'test -e /tmp/trash/demo/page' "the audit did not use the rm alias of the user"
on $h 'sed -i "/# probe$/d" ~/.${SHELL##*/}rc'
step "remove the feature again and delete its data"
scenario $h '{ features.demo.enable = true; claims.disposable.home = [ ".bash_history" ]; }'
on $h 'echo ls >~/.bash_history'
apply $h >/dev/null || fail "apply failed"
on $h 'mkdir -p ~/.cache/demo/keep && curl -fs -o ~/.cache/demo/page localhost:8081' || fail "the demo service does not answer"
scenario $h '{ files."/srv/demo/keep.txt".text = "kept\n"; claims.other.home = [ ".cache/demo/keep" ]; claims.arch.home = [ ".bash_logout" ]; }'
apply $h || fail "apply failed"
pacnix $h audit <<<a >/dev/null || fail "audit with the answer a failed"
refute $h 'test -e /srv/demo/index.html || test -e ~/.cache/demo/page' "the data of the removed feature remains"
check $h 'test "$(cat /srv/demo/keep.txt)" = kept && test -d ~/.cache/demo/keep' "a declared file or a claimed path is deleted"
check $h 'test -s ~/.bash_history' "the deletion of the data of a removed feature deleted ~/.bash_history"
step "the audit of a home directory"
scenario $h '{ features.home.enable = true; units.mask = [ "fstrim.service" ]; packages.repo = [ "fontconfig" "linux-headers" "dkms" "v4l2loopback-dkms" ]; packages.local.desk = ./packages/desk; claims.other.home = [ ".var/app/org.demo/cache" ".local/share/uv" ".config/gone" ]; claims.other.system = [ "/srv/gone" ]; audit.ignore = [ "/home/arch/.cache/ignored*" "/home/arch/.local/lib/**/keep" "/srv/gone*" ]; }'
apply $h >/dev/null || fail "apply failed"
on $h 'cd ~ && touch notes.txt .bash_history && mkdir -p .local/state/foo .local/bin .local/lib/x/deep .config/app/a .config/app/b Documents Music Work/Templates Work/Pictures Work/other .var/app/org.demo/cache .var/app/org.demo/data .var/app/other .var/app/org.pacnix.Desk .config/fontconfig .config/darkhttpd .desk-tool .cache/ignored-x &&
	touch .local/bin/tool .local/lib/x/deep/keep .local/lib/x/deep/other .config/demo/extra.conf && ln -s /etc .config/linkdir && cp /etc/skel/.bashrc /etc/skel/.bash_profile . && echo "# mine" >>.bash_profile && rm -f .bash_logout && mkfifo .bash_logout &&
	printf "XDG_DOCUMENTS_DIR=\"\$HOME/Documents\"\nXDG_DESKTOP_DIR=\"\$HOME\"\nXDG_MUSIC_DIR=\"\$HOME/\`echo Music\`\"\nXDG_TEMPLATES_DIR=\"\$HOME/Work/Templates\"\nXDG_PICTURES_DIR=\"/home/arch/Work/Pictures\"\n" >.config/user-dirs.dirs'
on $h 'mkdir -p ~/.local/share/uv/tools/ku-banner/bin ~/.local/share/icons ~/.cache/desktray ~/.cache/ku-banner && touch ~/.local/share/uv/tools/ku-banner/bin/ku-banner ~/.local/share/icons/org.pacnix.Desk.desktop.png ~/.desk-toolrc ~/.cache/desk-tool-recovery &&
	ln -s ../share/uv/tools/ku-banner/bin/ku-banner ~/.local/bin/ku-banner && sudo systemctl mask --quiet debug-shell.service' || fail "the home paths with clues cannot be made"
on $h 'cd /etc && sudo ln -s /usr/bin/pacman demo-pkg && sudo ln -s demo-pkg demo-chain && sudo ln -s /nonexistent demo-dangling && sudo touch demo-file && sudo ln -s demo-file demo-other && sudo ln -s /usr/share demo-dir && sudo ln -s /usr/bin/pacman demo.conf.pacnew && sudo touch /usr/share/fontconfig/demo-child && test -d /var/lib/desk-rule && test -L "$(ls -d /etc/fonts/conf.d/* | head -1)"' || fail "the fontconfig links are missing"
k=$(on $h 'ls /var/lib/dkms/v4l2loopback | sed -n "s/^kernel-\(.*\)-x86_64$/\1/p"')
v=$(on $h 'pacman -Q v4l2loopback-dkms | sed "s/.* \(.*\)-.*/\1/"')
d=/usr/lib/modules/$k/updates/dkms
on $h "test -f $d/v4l2loopback.ko.zst && test -d /var/lib/dkms/v4l2loopback/$v" || fail "DKMS did not install v4l2loopback $v for kernel $k"
on $h "cd /var/lib/dkms/v4l2loopback && sudo mkdir 0.0.1 && sudo ln -s /usr/src/v4l2loopback-0.0.1 0.0.1/source && sudo ln -s 0.0.1/9.9.9-arch1-1/x86_64 kernel-9.9.9-arch1-1-x86_64 &&
	sudo touch $d/stray.ko.zst && echo a | sudo tee $v/$k/x86_64/module/other.ko.zst >/dev/null && echo b | sudo tee $d/other.ko.zst >/dev/null" || fail "the stale DKMS paths cannot be made"
dkms="DKMS module v4l2loopback $v for kernel $k; the source is in package v4l2loopback-dkms"
start=$(date +%s.%N)
out=$(pacnix $h audit) || { echo "$out"; fail "audit failed"; }
echo "COUNT paths: $(grep -o '^• Paths that no package, declaration or claim owns ([0-9]*)' <<<"$out" | tr -dc '0-9'), audit $(awk -v s="$start" -v e="$(date +%s.%N)" 'BEGIN { printf "%.2f", e - s }') s"
echo "$out"
for l in '/home/arch/.config/fontconfig (the name is the same as installed package fontconfig)' '/home/arch/.var/app/org.pacnix.Desk (the name is the same as desktop ID org.pacnix.Desk)' '/var/lib/desk-rule (a tmpfiles rule in /usr/lib/tmpfiles.d/desk.conf has this path)' '/etc/xdg/nix (the name is the same as installed package nix)' '/home/arch/.desk-tool (the name is the same as program /usr/bin/desk-tool of installed package desk)'; do
	grep -qxF "  $l" <<<"$out" || fail "the audit does not show: $l"
done
for l in "$d/v4l2loopback.ko.zst (the same bytes as the build of $dkms)" "/var/lib/dkms/v4l2loopback/kernel-$k-x86_64 (active registration of $dkms)" "/var/lib/dkms/v4l2loopback/$v (DKMS module v4l2loopback $v: the source is in package v4l2loopback-dkms, and each kernel directory has an active registration)" \
	'/home/arch/.local/bin/ku-banner (a link to ../share/uv/tools/ku-banner/bin/ku-banner, which feature other claims)' '/home/arch/.cache/ku-banner (the name agrees with uv tool ku-banner)' \
	'/home/arch/.cache/desktray (the name agrees with installed package desk)' '/home/arch/.local/share/icons (it holds org.pacnix.Desk.desktop.png, whose name agrees with installed package desk)' \
	'/home/arch/.desk-toolrc (the name agrees with installed package desk)' '/home/arch/.cache/desk-tool-recovery (the name agrees with installed package desk)' \
	'/etc/systemd/system/debug-shell.service (a mask of unit debug-shell.service, which units.mask does not declare)' '/etc/demo-other (a link to demo-file)'; do
	grep -qxF "  $l" <<<"$out" || fail "the audit does not show: $l"
done
grep -qF /etc/systemd/system/fstrim.service <<<"$out" && fail "the audit lists a declared mask"
grep -qE '^ *[0-9]+  /etc/demo-dangling \(a link to /nonexistent, which does not exist\)$' <<<"$out" || fail "the audit does not offer the dangling link as a leftover"
grep -qE '^ *[0-9]+  /var/lib/dkms/v4l2loopback/kernel-9.9.9-arch1-1-x86_64 \(a link to .*, which does not exist\)$' <<<"$out" || fail "the audit does not offer the stale DKMS registration as a leftover"
# A stale DKMS path has no DKMS note, and a path in a directory of the kernel packages has no name note.
for p in /var/lib/dkms/v4l2loopback/0.0.1 $d/stray.ko.zst $d/other.ko.zst /usr/share/fontconfig/demo-child; do
	grep -qxE "  $p( \(in directory [^()]*, which only installed packages? [^()]*\))?" <<<"$out" || fail "the audit shows $p with a wrong note"
done
for p in /home/arch/.config/darkhttpd; do
	grep -qxF "  $p" <<<"$out" || fail "the audit shows $p with a note"
done
awk '/^• Leftovers of removed packages and features, and broken links/ { a = NR } /^• Paths that no package, declaration or claim owns/ { b = NR } /^• Explicit package not declared/ { c = NR } END { exit !(b && b < c && c < a) }' <<<"$out" || fail "the sections of the audit are not in the order: paths, other differences, leftovers"
LC_ALL=C sort -c <<<"$(awk '/^• Paths that no package, declaration or claim owns/ { p = 1; next } /^• / { p = 0 } p { print $1 }' <<<"$out")" || fail "the paths of the audit are not in alphabetical order"
for p in demo-file demo-other demo-dir; do
	audit_has "$out" paths "/etc/$p" || fail "the audit does not list /etc/$p"
done
for l in '.config/gone (home claim of feature other)' '/srv/gone (feature other)'; do
	audit_has "$out" 'Claim that matches no path' "$l" || fail "the audit does not show the claim that matches no path: $l"
done
audit_has "$out" 'Pattern of audit.ignore that matches no path' '/srv/gone*' || fail "the audit does not show the audit.ignore pattern that matches no path"
grep -qE '^  .*\((home claim of )?feature (arch|pacnix)\)$' <<<"$out" && fail "the audit shows a claim of the pacnix modules"
audit_has "$out" 'Claim that matches no path' '.var/app/org.demo/cache (home claim of feature other)' && fail "the audit shows a claim that matches a path"
audit_has "$out" 'File .pacnew' /etc/demo.conf.pacnew || fail "the audit does not list /etc/demo.conf.pacnew"
grep -q '/etc/demo-pkg\|/etc/demo-chain\|/etc/fonts' <<<"$out" && fail "the audit lists a link to a file of a package"
for p in notes.txt .local/state/foo .local/bin/tool .local/lib/x/deep/other Work/other .config/demo/extra.conf .config/app .config/linkdir Music .bash_profile .bash_logout .var/app/org.demo/data .var/app/other; do
	audit_has "$out" paths "/home/arch/$p" || fail "the audit does not list ~/$p"
done
# pacnix never deletes a bash file: the former claim is a note, not a leftover to delete.
audit_has "$out" paths '/home/arch/.bash_history (claimed by feature disposable in generation 4; the feature is no longer enabled)' || fail "the audit does not list ~/.bash_history with its former claim"
grep -qE '^ *[0-9]+  /home/arch/.bash_history' <<<"$out" && fail "the audit offers to delete ~/.bash_history"
for p in .config/app/a .config/demo/settings.conf Documents Work Work/Templates Work/Pictures .local/lib .local/lib/x .local/lib/x/deep/keep .bashrc .cache/ignored-x .var/app/org.demo/cache .var .var/app .config/linkdir/passwd .config/Code; do
	audit_has "$out" paths "/home/arch/$p" && fail "the audit lists ~/$p"
done
on $h "sudo rm -rf /var/lib/dkms/v4l2loopback/0.0.1 /var/lib/dkms/v4l2loopback/kernel-9.9.9-arch1-1-x86_64 $d/stray.ko.zst $d/other.ko.zst /var/lib/dkms/v4l2loopback/$v/$k/x86_64/module/other.ko.zst"
on $h 'sudo systemctl unmask --quiet debug-shell.service && ln -s ../../.config/dconf ~/.local/bin/desk-tool && sudo systemctl start desk.service && systemctl --user start desk.service && sudo install -d -o desk /srv/desk-data && echo setting=2 | sudo tee /etc/desk.conf >/dev/null' || fail "the desk services did not start"
step "remove a package: the audit offers what it left"
scenario $h '{ features.home.enable = true; claims.other.home = [ ".bash_history" ]; }'
apply $h >/dev/null || fail "apply failed"
out=$(pacnix $h audit)
echo "$out"
for l in '/var/lib/desk-rule (a tmpfiles rule in /usr/lib/tmpfiles.d/desk.conf of removed package desk)' \
	'/var/lib/desk-state (the StateDirectory of unit desk.service of removed package desk)' \
	'/srv/desk-data (the owner is user desk of removed package desk)' \
	'/etc/desk.conf.pacsave (the saved copy of /etc/desk.conf of removed package desk)' \
	'/home/arch/.cache/desk-cache (the CacheDirectory of unit desk.service of removed package desk)' \
	'/home/arch/.desk-tool (the name is the same as program or desktop ID desk-tool of removed package desk)' \
	'/home/arch/.var/app/org.pacnix.Desk (the name is the same as program or desktop ID org.pacnix.Desk of removed package desk)'; do
	grep -E '^ *[0-9]+  ' <<<"$out" | grep -qF -- "  $l" || fail "the audit does not offer the leftover: $l"
done
grep -qE '^ *[0-9]+  .*/srv/pacnix' <<<"$out" && fail "the audit offers a path below a directory that an installed package owns"
grep -qxF '  /home/arch/.local/bin/desk-tool (a link to ../../.config/dconf, which feature home claims; the name is also the same as program or desktop ID desk-tool of removed package desk)' <<<"$out" || fail "the audit does not show the clue and the removed package of ~/.local/bin/desk-tool"
grep -qE '^ *[0-9]+  .*/\.local/bin/desk-tool' <<<"$out" && fail "the audit offers a path with a clue as a leftover"
on $h 'rm ~/.local/bin/desk-tool'
pacnix $h audit <<<a >/dev/null || fail "audit with the answer a failed"
refute $h 'test -L /etc/demo-dangling || test -e /var/lib/desk-rule || test -e /var/lib/desk-state || test -e /srv/desk-data || test -e /etc/desk.conf.pacsave || test -e ~/.cache/desk-cache || test -e ~/.desk-tool' "a leftover of the removed package stays"
grep -q 'of removed package desk' <<<"$(pacnix $h audit)" && fail "the audit shows a leftover of desk after the deletion"
check $h '! sudo jq -e "has(\"desk\")" /var/lib/pacnix/removed.json' "the record of desk stays after its leftovers went"
grep -q '/home/arch/.bash_history' <<<"$out" && fail "the audit lists a claimed bash file"
matches "$out" '/home/arch/.var/app/org.demo/cache \(claimed by feature other in generation [0-9]+; the feature no longer claims it\)' || fail "the audit does not show the path that feature other no longer claims"
audit_has "$out" paths /home/arch/.var && fail "the audit shows ~/.var as one entry"
pass
