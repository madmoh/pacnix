# State that pacman hooks, systemd and boot loaders create on every Arch host,
# and the footprint of pacnix, Nix and Home Manager, which pacnix uninstall
# deletes. The audit does not report it.
{
  claims.arch.system = [
    "/etc/machine-id" "/etc/hostname" "/etc/localtime" "/etc/locale.conf" "/etc/vconsole.conf" "/etc/adjtime"
    "/etc/ld.so.cache" "/etc/.pwd.lock" "/etc/{passwd,group,shadow,gshadow,subuid,subgid}-" "/etc/{subuid,subgid}"
    "/etc/resolv.conf" "/etc/.updated" "/var/.updated" "/etc/udev/hwdb.bin" "/etc/mkinitcpio.d/*.preset"
    "/etc/pacman.d/gnupg" "/etc/ssl/certs" "/etc/ca-certificates/extracted" "/etc/ssh/ssh_host_*"
    "/etc/systemd/system/*.wants" "/etc/systemd/system/*.requires" "/etc/systemd/system/*.upholds"
    "/etc/systemd/system/dbus-*.service" "/etc/systemd/system/display-manager.service" "/etc/systemd/system/autovt@*.service"
    "/etc/systemd/user/*.wants" "/etc/credstore*"
    "/boot" "/efi"
    "/etc/os-release" "/etc/pacnix" "/usr/lib/udev/hwdb.bin" "/usr/lib/gconv/gconv-modules.cache"
    "/usr/lib/modules/*/modules.*" "/usr/share/info/dir" "/usr/lib/locale/locale-archive"
    "/usr/share/mime/**" "/usr/share/glib-2.0/schemas/gschemas.compiled" "/usr/share/icons/*/icon-theme.cache"
    "/usr/lib/gdk-pixbuf-2.0/*/loaders.cache" "/usr/lib/gio/modules/giomodule.cache" "/usr/lib/gtk-*/*/immodules.cache"
    "/usr/share/applications/mimeinfo.cache" "/usr/lib/**/__pycache__" "/usr/share/**/__pycache__"
    "/var/cache" "/var/log" "/var/lib/systemd" "/var/lib/dbus" "/var/lib/private" "/var/lib/machines" "/var/lib/portables"
    "/var/lib/lastlog" "/var/lib/libuuid" "/var/db/sudo" "/var/spool" "/var/lib/tpm2-tss" "/var/lib/polkit-1"
  ];
  claims.pacnix.system = [
    "/nix" "/var/lib/pacnix" "/var/lib/aurbuild" "/var/lib/pacman/sync/pacnix-local.*"
    "/etc/systemd/system-preset" "/etc/profiles/per-user" "/root/.cache/nix" "/root/.nix-defexpr"
  ];
  claims.pacnix.home = [ ".cache/nix" ".local/state/home-manager" ".local/state/nix" ".nix-defexpr" ".nix-profile" ];
}
