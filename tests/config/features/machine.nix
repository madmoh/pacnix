# The steps of the Arch installation guide after mount, as declarations: fstab,
# swap, time zone, locale, host name, network, boot loader, a user and SSH
# access. The disk layout comes from the host; partitions and file systems are
# not declared.
{ config, lib, ... }:
let
  cfg = config.features.machine;
in
{
  options.features.machine = {
    enable = lib.mkEnableOption "an installed machine without cloud-init";
    sshKey = lib.mkOption { type = lib.types.str; default = ""; };
  };
  config = lib.mkIf cfg.enable {
    packages.repo = [ "base" "openssh" "sudo" "zsh" "git" "rsync" "jq" ];
    fileSystems."/" = { device = "LABEL=root"; fsType = "ext4"; options = [ "rw" "relatime" ]; };
    fileSystems."/boot" = { device = "LABEL=ESP"; fsType = "vfat"; options = [ "rw" "fmask=0077" "dmask=0077" ]; };
    swapDevices = [ { device = "/swapfile"; size = 256; } ];
    zramSwap.enable = true;
    boot.kernels = [ "linux" ];
    boot.loader.systemd-boot = { enable = true; settings.timeout = 1; };
    time.timeZone = "Europe/Berlin";
    i18n.defaultLocale = "en_US.UTF-8";
    console.keyMap = "us";
    users.users.${config.user} = { uid = 1000; extraGroups = [ "wheel" ]; shell = "/usr/bin/zsh"; };
    files."/etc/resolv.conf".target = "/run/systemd/resolve/stub-resolv.conf";
    files."/etc/systemd/network/20-wired.network".text = ''
      [Match]
      Name=en*
      [Network]
      DHCP=yes
    '';
    files."/etc/sudoers.d/50-wheel".text = "%wheel ALL=(ALL:ALL) NOPASSWD: ALL\n";
    files."/etc/sudoers.d/50-wheel".mode = "0440";
    files."/etc/ssh/sshd_config.d/50-keys.conf".text = "AuthorizedKeysFile .ssh/authorized_keys /etc/ssh/authorized_keys/%u\n";
    files."/etc/ssh/authorized_keys/${config.user}".text = cfg.sshKey;
    units.enable = [ "sshd.service" "systemd-networkd.service" "systemd-resolved.service" "systemd-timesyncd.service" "getty@tty1.service" ];
    claims.machine.system = [ "/etc/ssh/ssh_host_*" ];
  };
}
