#!/usr/bin/env bash
# Scenario 3: add and remove a repo package, a group and AUR packages, remove a whole
# feature, and restore the original files after an override is dropped. A new AUR
# dependency of a built AUR package is a notice at the end of the apply and in
# the audit; apply asks no question for it or for package removals, also
# without a terminal, and the plan lists the removals. The output of aur sync
# does not show when the build succeeds. After the install, the AUR checkouts
# keep only their git history, PKGBUILD and .SRCINFO, and no copy of the build
# chroot stays. A package base in packages.keepBuildCache keeps its sources until
# it leaves the list. The checkout of an AUR package goes when no declared AUR
# package needs it. The apply removes a link in the build directory, not its
# target.
# After an apply without a failure, the built packages leave the local
# repository: an installed package does not build again, and a removed one
# builds again. A local split package with a version from pkgver() does not
# build again while its installed version is the built one, and a local
# package that needs a local package that left the repository builds. A host
# that serves packages keeps them; the first apply after the end of serving
# removes the package files of earlier applies and builds nothing, as the
# first apply after the upgrade from pacnix 0.16.1.
source "$(dirname "$0")/../lib.sh"
fresh $h
cache=$(cache_list $h)
apply $h >/dev/null || fail "first apply failed"
repo='pacman -Slq --config /run/pacnix/pacman.conf pacnix-local'

step "add a repo package and AUR packages, one with an AUR dependency, on a host that serves one"
scenario $h '{ packages.repo = [ "htop" ]; packages.aur = [ "downgrade" "perl-email-outlook-message" ]; packages.serve.aur = [ "downgrade" ]; }'
out=$(apply $h) || { echo "$out"; fail "apply failed"; }
check $h "$repo | grep -qx perl-ole-storage-lite && ls /var/lib/pacnix/repo/downgrade-*.pkg.tar.zst" "a host that serves packages does not keep them"
refute $h "sudo test -e /var/lib/pacnix/pruned.json" "a host that serves packages has a record of removed packages"
grep -q 'packages ignored' <<<"$out" && fail "apply shows the list of aur sync"
check $h "pacman -Q htop downgrade perl-email-outlook-message" "declared packages are missing"
matches "$out" 'Downloading source Email-Outlook-Message-.* for perl-email-outlook-message\. .*Downloaded source Email-Outlook-Message-.* \([0-9.]+ (B|KiB|MiB)\)\. .*Packaging perl-email-outlook-message\.' || { echo "$out"; fail "apply does not show the AUR source download, its size and packaging in order"; }

step "the first apply after the end of serving removes the package files and builds nothing"
scenario $h '{ packages.repo = [ "htop" ]; packages.aur = [ "downgrade" "perl-email-outlook-message" ]; }'
out=$(apply $h) || { echo "$out"; fail "apply failed"; }
grep -qE '^• (Preparing .*package|Building|Packaging)' <<<"$out" && { echo "$out"; fail "the apply builds"; }
check $h "pacman -Qqd perl-ole-storage-lite" "the AUR dependency is not installed as a dependency"
refute $h "ls /var/lib/pacnix/repo/*.pkg.tar*" "package files stay in the local repository after the install"
refute $h "$repo | grep -q ." "the database of the local repository names packages that left it"
check $h "sudo jq -e '.downgrade and .\"perl-email-outlook-message\" and .\"perl-ole-storage-lite\"' /var/lib/pacnix/pruned.json" "the record of the packages that left the repository is incomplete"
ckout="sudo ls -A /var/lib/pacnix/build/.cache/aurutils/sync"
# A checkout without files that git does not track: the sources and logs went.
clean() { check $h "test -z \"\$(sudo -u pacnix git -C /var/lib/pacnix/build/.cache/aurutils/sync/$1 status --porcelain --ignored)\" && $ckout/$1 | grep -qx PKGBUILD" "$2"; }
for b in downgrade perl-email-outlook-message perl-ole-storage-lite; do
	clean $b "the checkout of $b keeps its sources or logs"
done
check $h 'test "$(sudo ls /var/lib/aurbuild/x86_64 | tr "\n" " ")" = "root root.lock "' "a copy of the build chroot stays"
packages_match $h

step "a new AUR dependency of a built AUR package is a notice"
scenario $h '{ packages.repo = [ "htop" ]; packages.aur = [ "downgrade" "perl-email-outlook-message" ]; packages.keepBuildCache = [ "perl-email-outlook-message" ]; }'
on $h "sudo jq '.\"perl-email-outlook-message\".depends = []' /var/lib/pacnix/aur.json | sudo tee /var/lib/pacnix/aur.json.new >/dev/null && sudo mv /var/lib/pacnix/aur.json.new /var/lib/pacnix/aur.json && sudo jq 'del(.\"perl-email-outlook-message\")' /var/lib/pacnix/pruned.json | sudo tee /var/lib/pacnix/pruned.json.new >/dev/null && sudo mv /var/lib/pacnix/pruned.json.new /var/lib/pacnix/pruned.json"
out=$(apply $h) || { echo "$out"; fail "apply failed"; }
has "$out" '• The AUR package perl-email-outlook-message has new AUR dependencies: perl-ole-storage-lite.' || { echo "$out" | tail -5; fail "apply does not show the new AUR dependency"; }
has "$(pacnix $h audit)" 'The AUR package perl-email-outlook-message has new AUR dependencies: perl-ole-storage-lite.' || fail "the audit does not show the notice"
grep -q 'packages ignored' <<<"$out" && fail "apply shows the list of aur sync"
check $h "$ckout/perl-email-outlook-message | grep -q '^Email-Outlook-Message-.*tar.gz$'" "the sources of a package in keepBuildCache do not stay"

step "the sources go when the package base leaves keepBuildCache, and a link goes without its target"
on $h 'sudo -u pacnix mkdir -p /var/lib/pacnix/build/sources && sudo mkdir -p /srv/target && sudo touch /srv/target/file && sudo ln -s /srv/target /var/lib/pacnix/build/sources/perl-email-outlook-message'
scenario $h '{ packages.repo = [ "htop" ]; packages.aur = [ "downgrade" "perl-email-outlook-message" ]; }'
apply $h >/dev/null || fail "apply failed"
for b in downgrade perl-email-outlook-message; do
	clean $b "the sources of $b stay"
done
check $h 'test -z "$(sudo ls -A /var/lib/pacnix/build/local /var/lib/pacnix/build/sources | grep -v "^/\|^$")"' "sources or a link stay"
check $h 'test -f /srv/target/file' "the apply removed the target of a link"
before=$(on $h 'stat -c %Y /var/lib/pacnix/build-cache.json')
empty_plan $h
apply $h >/dev/null || fail "the no-op apply failed"
[[ $before == $(on $h 'stat -c %Y /var/lib/pacnix/build-cache.json') ]] || fail "the no-op apply wrote the record of the sources"

step "a removed package that left the repository builds again"
on $h 'sudo pacman -R --noconfirm downgrade >/dev/null'
out=$(apply $h) || { echo "$out"; fail "apply failed"; }
has "$out" 'Packaging downgrade.' || { echo "$out" | tail; fail "downgrade did not build again"; }
check $h "pacman -Q downgrade" "downgrade is not installed again"
refute $h "ls /var/lib/pacnix/repo/*.pkg.tar*" "package files stay in the local repository"

step "local packages: a split package with a version from pkgver(), and a package that needs a package that left the repository"
scenario $h '{ packages.repo = [ "htop" ]; packages.aur = [ "downgrade" "perl-email-outlook-message" ]; packages.local = { split-a = ./packages/split; kg-a = ./packages/kg-a; }; }'
apply $h >/dev/null || fail "apply failed"
v=$(on $h 'pacman -Q split-a')
refute $h "ls /var/lib/pacnix/repo/*.pkg.tar*" "package files stay in the local repository"
out=$(apply $h) || { echo "$out"; fail "the no-op apply failed"; }
grep -qE '^• (Preparing .*package|Building|Packaging)' <<<"$out" && { echo "$out"; fail "the no-op apply builds"; }
[[ $(on $h 'pacman -Q split-a') == "$v" ]] || fail "the no-op apply changed split-a"
empty_plan $h
scenario $h '{ packages.repo = [ "htop" ]; packages.aur = [ "downgrade" "perl-email-outlook-message" ]; packages.local = { split-a = ./packages/split; split-b = ./packages/split; kg-a = ./packages/kg-a; kg-b = ./packages/kg-b; }; }'
out=$(apply $h) || { echo "$out"; fail "apply of split-b and of kg-b, which needs kg-a, failed"; }
check $h "pacman -Q split-b kg-b" "split-b or kg-b is not installed"
refute $h "ls /var/lib/pacnix/repo/*.pkg.tar*" "package files stay in the local repository"
empty_plan $h

step "remove them"
scenario $h ''
out=$(apply $h) || { echo "$out" | tail -20; fail "apply failed"; }
changes "$out" | grep -qx 'Remove package htop' || { echo "$out"; fail "the plan of the apply does not list the removal"; }
for p in htop downgrade perl-email-outlook-message perl-ole-storage-lite; do
	refute $h "pacman -Q $p" "$p is still installed"
	refute $h "$repo | grep -qx $p" "$p is still in the local repository"
	refute $h "ls /var/lib/pacnix/repo/$p-[0-9]*" "the file of $p stays in the local repository"
done
refute $h "$ckout | grep -q ." "the AUR checkouts of the removed packages stay"
packages_match $h

step "adopt an older AUR package installed outside pacnix"
on $h 'set -e
	work=$(mktemp -d)
	trap "rm -rf \"$work\"" EXIT
	cd "$work"
	printf "%s\n" "pkgname=downgrade" "pkgver=0.1" "pkgrel=1" "arch=(any)" \
		"package() { install -dm755 \"\$pkgdir/usr/share/downgrade\"; }" >PKGBUILD
	makepkg --noconfirm >/dev/null
	printf "[options]\nSigLevel = Never\n" >pacman.conf
	sudo pacman -U --config "$work/pacman.conf" --noconfirm downgrade-*.pkg.tar.zst >/dev/null'
scenario $h '{ packages.aur = [ "downgrade" ]; }'
out=$(apply $h) || { echo "$out"; fail "apply failed"; }
has "$out" 'package upgraded: downgrade' || { echo "$out"; fail "apply does not report the upgrade"; }
check $h 'test "$(vercmp "$(pacman -Q downgrade | cut -d" " -f2)" 0.1-1)" = 1' "the older AUR package is still installed"
out=$(apply $h) || { echo "$out"; fail "the second apply failed"; }
has "$out" 'Apply done. No changes.' || { echo "$out"; fail "the second apply changed packages"; }
grep -qE '^• (Preparing .*package|Building|Packaging|Downloading)' <<<"$out" && { echo "$out"; fail "the second apply reports a build or download"; }
scenario $h ''
apply $h >/dev/null || fail "apply failed after the removal of the declaration"
check $h 'pacman -Qqe downgrade' "apply removed the externally installed package"
on $h 'sudo pacman -Rns --noconfirm downgrade >/dev/null'

step "a group stands for its members, as with pacman -S"
scenario $h '{ packages.repo = [ "fprint" ]; }'
apply $h >/dev/null || fail "apply failed"
check $h "pacman -Qqe fprintd libfprint" "the members of the group are not explicitly installed"
empty_plan $h
pacnix $h audit | grep -q '^  fprint' && fail "the audit does not count the members as declared"
scenario $h ''
apply $h >/dev/null || fail "apply failed"
refute $h "pacman -Q fprintd" "a member of the removed group is still installed"

step "a symbolic link: create, change the target, drop"
scenario $h '{ files."/etc/pacnix-zone".target = "/usr/share/zoneinfo/UTC"; }'
apply $h >/dev/null || fail "apply failed"
check $h 'test "$(readlink /etc/pacnix-zone)" = /usr/share/zoneinfo/UTC' "the link is not created"
scenario $h '{ files."/etc/pacnix-zone".target = "/usr/share/zoneinfo/Europe/Berlin"; }'
changes "$(plan $h)" | grep -qx 'Changing file /etc/pacnix-zone (link to /usr/share/zoneinfo/Europe/Berlin)' || fail "the plan does not update the link"
apply $h >/dev/null || fail "apply failed"
check $h 'test "$(readlink /etc/pacnix-zone)" = /usr/share/zoneinfo/Europe/Berlin' "the link target is not changed"
empty_plan $h
scenario $h ''
apply $h >/dev/null || fail "apply failed"
refute $h 'test -L /etc/pacnix-zone' "the dropped link remains"

step "enable a feature"
scenario $h '{ features.demo.enable = true; }'
apply $h || fail "apply failed"
check $h "curl -fs localhost:8081" "the demo service does not answer"
check $h "systemctl show -p Environment sshd | grep -q DEMO=1" "the sshd drop-in is not active"
check $h "test \$(sysctl -n net.core.somaxconn) = 8192" "the sysctl trigger did not run"

step "remove the feature"
scenario $h ''
apply $h || fail "apply failed"
refute $h "pacman -Q darkhttpd" "darkhttpd is still installed"
for f in /etc/systemd/system/demo-web.service /etc/systemd/system/sshd.service.d /etc/sysctl.d/90-demo.conf; do
	refute $h "test -e $f" "$f still exists"
done
refute $h "systemctl is-enabled demo-web.service" "demo-web.service is still enabled"
refute $h "systemctl is-active demo-web.service" "demo-web.service is still running"
refute $h "find /etc/systemd/system -xtype l | grep ." "dangling unit links remain"
refute $h "systemctl show -p Environment sshd | grep -q DEMO=1" "the sshd drop-in is still active"
empty_plan $h

step "override package files"
originals=$(on $h 'md5sum /etc/pacman.conf /etc/locale.gen /etc/pam.d/su')
scenario $h '{ features.overrides.enable = true; }'
apply $h || fail "apply failed"
check $h "grep -qx Color /etc/pacman.conf" "pacman.conf is not overridden"
check $h "locale -a | grep -q ar_AE" "the locale trigger did not run"
check $h "grep -q '^auth.*pam_wheel.so use_uid' /etc/pam.d/su" "the PAM file is not overridden"
empty_plan $h

step "drop the overrides and restore the original files"
scenario $h ''
apply $h || fail "apply failed"
[[ $(on $h 'md5sum /etc/pacman.conf /etc/locale.gen /etc/pam.d/su') == "$originals" ]] || fail "the original files did not come back"
refute $h "locale -a | grep -q ar_AE" "the locale trigger did not run after the restore"
empty_plan $h
cache_kept $h "$cache"
pass
