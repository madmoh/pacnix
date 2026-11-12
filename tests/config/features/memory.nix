# zram, a swap file and sysctl. The swap size is per host.
{ config, lib, ... }:
let
  cfg = config.features.memory;
in
{
  options.features.memory = {
    enable = lib.mkEnableOption "zram, a swap file and memory sysctl settings";
    swapSizeMiB = lib.mkOption {
      type = lib.types.ints.positive;
      default = 8192;
    };
  };
  config = lib.mkIf cfg.enable {
    packages.repo = [ "zram-generator" ];
    files."/etc/systemd/zram-generator.conf".source = ../files/memory/zram-generator.conf;
    files."/etc/systemd/system/dev-zram0.swap.d/memory.conf".source = ../files/memory/zram-swap.conf;
    files."/etc/systemd/system/memory-zswap-disable.service".source = ../files/memory/memory-zswap-disable.service;
    files."/etc/systemd/system/swapfile.swap".source = ../files/memory/swapfile.swap;
    files."/etc/sysctl.d/99-memory.conf".source = ../files/memory/99-memory.conf;
    # The swap file is state. This unit creates it once, before the swap unit.
    files."/etc/systemd/system/swapfile.swap.d/create.conf".text = ''
      [Unit]
      Requires=swapfile-create.service
      After=swapfile-create.service
    '';
    files."/etc/systemd/system/swapfile-create.service".text = ''
      [Unit]
      Description=Create the swap file
      DefaultDependencies=no
      ConditionPathExists=!/swapfile
      [Service]
      Type=oneshot
      ExecStart=/usr/bin/sh -ec 'if [ "$(findmnt -no FSTYPE /)" = btrfs ]; then btrfs filesystem mkswapfile --size ${toString cfg.swapSizeMiB}m /swapfile.new; else dd if=/dev/zero of=/swapfile.new bs=1M count=${toString cfg.swapSizeMiB} && chmod 600 /swapfile.new && mkswap /swapfile.new; fi; mv /swapfile.new /swapfile'
    '';
    units.enable = [ "swapfile.swap" ];
    triggers.sysctl = { paths = [ "/etc/sysctl.d/" ]; command = "sysctl --system >/dev/null"; };
    triggers.zram = {
      paths = [ "/etc/systemd/zram-generator.conf" "/etc/systemd/system/dev-zram0.swap.d/" ];
      command = "systemctl daemon-reload && systemctl restart systemd-zram-setup@zram0.service && systemctl start dev-zram0.swap";
    };
    claims.memory.system = [ "/swapfile" ];
  };
}
