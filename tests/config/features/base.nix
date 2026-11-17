# The Arch cloud image: kernel, boot loader, network, SSH and cloud-init.
{ config, lib, ... }:
let
  cfg = config.features.base;
in
{
  options.features.base = {
    enable = lib.mkEnableOption "the base system of a cloud VM";
    kernel = lib.mkOption { type = lib.types.str; default = "linux"; };
    network = lib.mkOption { type = lib.types.enum [ "networkd" "NetworkManager" ]; default = "networkd"; };
  };
  options.user = lib.mkOption {
    type = lib.types.str;
    default = "arch";
  };
  config = lib.mkIf cfg.enable {
    packages.repo = [
      "base" "efibootmgr" "dosfstools" "btrfs-progs" "gptfdisk"
      "openssh" "sudo" "cloud-init" "cloud-guest-utils" "qemu-guest-agent" "rsync" "git" "jq"
    ] ++ lib.optional (cfg.network == "NetworkManager") "networkmanager";
    units.enable = [
      "sshd.service" "cloud-init-local.service" "cloud-init-main.service" "cloud-init-network.service"
      "cloud-config.service" "cloud-final.service" "pacman-init.service" "getty@tty1.service"
      "systemd-network-generator.service" "systemd-resolved.service" "systemd-timesyncd.service"
      "systemd-homed.service" "systemd-userdbd.socket" "remote-fs.target"
    ] ++ (if cfg.network == "networkd"
      then [ "systemd-networkd.service" "systemd-networkd.socket" "systemd-networkd-wait-online.service" ]
      else [ "NetworkManager.service" "NetworkManager-wait-online.service" ]);
    # The systemd vendor preset enables systemd-networkd.
    units.disable = lib.optionals (cfg.network != "networkd") [ "systemd-networkd.service" "systemd-networkd.socket" "systemd-networkd-wait-online.service" ];
    boot.kernels = [ cfg.kernel ];
    boot.loader.grub = { enable = true; device = "/dev/vda"; settings.GRUB_TIMEOUT = "1"; };
    claims.base.system = [
      "/efi" "/swap" "/var/lib/cloud" "/etc/systemd/network/10-cloud-init-*.network" "/etc/sudoers.d/90-cloud-init-users"
      "/etc/systemd/system/pacman-init.service" "/etc/systemd/system/ctrl-alt-del.target" "/etc/audit/*.d" "/etc/audisp"
    ];
    claims.base.home = [ ".ssh" ];
  };
}
