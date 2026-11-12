# The steps of the Arch installation guide after mount, as declarations: fstab,
# time zone, locale, host name, network, boot loader, a user and SSH access.
# The disk layout comes from the host; partitions and file systems are not declared.
{ config, lib, ... }:
let
  cfg = config.features.machine;
in
{
  options.features.machine = {
    enable = lib.mkEnableOption "an installed machine without cloud-init";
    timeZone = lib.mkOption { type = lib.types.str; default = "Europe/Berlin"; };
    sshKey = lib.mkOption { type = lib.types.str; default = ""; };
    shell = lib.mkOption { type = lib.types.str; default = "/usr/bin/zsh"; };
  };
  config = lib.mkIf cfg.enable {
    packages.repo = [ "base" "linux" "grub" "openssh" "sudo" "zsh" "rsync" "jq" ];
    files."/etc/fstab".text = ''
      LABEL=root / ext4 rw,relatime 0 1
    '';
    files."/etc/hostname".text = "${config.hostName}\n";
    files."/etc/localtime".target = "/usr/share/zoneinfo/${cfg.timeZone}";
    files."/etc/locale.gen".text = "en_US.UTF-8 UTF-8\n";
    files."/etc/locale.conf".text = "LANG=en_US.UTF-8\n";
    files."/etc/vconsole.conf".text = "KEYMAP=us\n";
    files."/etc/resolv.conf".target = "/run/systemd/resolve/stub-resolv.conf";
    files."/etc/systemd/network/20-wired.network".text = ''
      [Match]
      Name=en*
      [Network]
      DHCP=yes
    '';
    files."/etc/default/grub.d/10-machine.cfg".text = ''
      GRUB_TIMEOUT=1
    '';
    files."/etc/sysusers.d/50-${config.user}.conf".text = ''
      u ${config.user} 1000 "${config.user}" /home/${config.user} ${cfg.shell}
      m ${config.user} wheel
    '';
    files."/etc/tmpfiles.d/50-${config.user}.conf".text = ''
      d /home/${config.user} 0700 ${config.user} ${config.user}
    '';
    files."/etc/sudoers.d/50-wheel".text = "%wheel ALL=(ALL:ALL) NOPASSWD: ALL\n";
    files."/etc/sudoers.d/50-wheel".mode = "0440";
    files."/etc/ssh/sshd_config.d/50-keys.conf".text = "AuthorizedKeysFile .ssh/authorized_keys /etc/ssh/authorized_keys/%u\n";
    files."/etc/ssh/authorized_keys/${config.user}".text = cfg.sshKey;
    triggers.locale = { paths = [ "/etc/locale.gen" ]; command = "locale-gen >/dev/null"; };
    triggers.users = { paths = [ "/etc/sysusers.d/" "/etc/tmpfiles.d/" ]; command = "systemd-sysusers && systemd-tmpfiles --create"; };
    triggers.grub = {
      paths = [ "/etc/default/grub" "/etc/default/grub.d/" ];
      command = ''grub-install --target=i386-pc "/dev/$(lsblk -no PKNAME "$(findmnt -no SOURCE /)")" && grub-mkconfig -o /boot/grub/grub.cfg'';
    };
    units.enable = [ "sshd.service" "systemd-networkd.service" "systemd-resolved.service" "systemd-timesyncd.service" "getty@tty1.service" ];
    claims.machine.system = [ "/etc/ssh/ssh_host_*" ];
  };
}
