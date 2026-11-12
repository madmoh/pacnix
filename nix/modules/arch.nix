# State that pacman hooks, systemd, boot loaders and pacnix itself create on
# every Arch host. The audit does not report it.
{
  claims.arch.system = [
    "/etc/machine-id" "/etc/hostname" "/etc/localtime" "/etc/locale.conf" "/etc/vconsole.conf" "/etc/adjtime"
    "/etc/ld.so.cache" "/etc/.pwd.lock" "/etc/{passwd,group,shadow,gshadow,subuid,subgid}-" "/etc/{subuid,subgid}"
    "/etc/resolv.conf" "/etc/.updated" "/var/.updated" "/etc/udev/hwdb.bin" "/etc/mkinitcpio.d/*.preset"
    "/etc/pacman.d/gnupg" "/etc/ssl/certs" "/etc/ca-certificates/extracted" "/etc/ssh/ssh_host_*"
    "/etc/systemd/system/*.wants" "/etc/systemd/system/*.requires" "/etc/systemd/system/*.upholds"
    "/etc/systemd/system/dbus-*.service" "/etc/systemd/system/display-manager.service" "/etc/systemd/system/autovt@*.service"
    "/etc/systemd/system-preset/00-pacnix.preset" "/etc/systemd/user/*.wants" "/etc/credstore*"
    "/boot" "/efi"
    "/etc/os-release" "/etc/pacnix" "/etc/systemd/system-preset" "/usr/lib/udev/hwdb.bin" "/usr/lib/gconv/gconv-modules.cache"
    "/usr/lib/modules/*/modules.*" "/usr/share/info/dir" "/usr/lib/locale/locale-archive"
    "/usr/share/mime/**" "/usr/share/glib-2.0/schemas/gschemas.compiled" "/usr/share/icons/*/icon-theme.cache"
    "/usr/lib/gdk-pixbuf-2.0/*/loaders.cache" "/usr/lib/gio/modules/giomodule.cache" "/usr/lib/gtk-*/*/immodules.cache"
    "/usr/share/applications/mimeinfo.cache" "/usr/lib/**/__pycache__" "/usr/share/**/__pycache__"
    "/var/cache" "/var/log" "/var/lib/systemd" "/var/lib/dbus" "/var/lib/private" "/var/lib/machines" "/var/lib/portables"
    "/var/lib/lastlog" "/var/lib/libuuid" "/var/db/sudo" "/var/spool" "/var/lib/aurbuild" "/var/lib/nix" "/var/lib/tpm2-tss" "/var/lib/polkit-1"
  ];
  claims.arch.home = [ ".bash_logout" ".bash_profile" ".bashrc" ".bash_history" ];
}
