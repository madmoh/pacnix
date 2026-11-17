# Kernel command line, and initramfs settings through a drop-in.
{ config, lib, ... }:
let
  cfg = config.features.boot;
in
{
  options.features.boot = {
    enable = lib.mkEnableOption "kernel command line and initramfs drop-ins";
    cmdline = lib.mkOption {
      type = lib.types.str;
      default = "";
    };
    compression = lib.mkOption {
      type = lib.types.enum [ "zstd" "xz" "lz4" "gzip" ];
      default = "zstd";
    };
  };
  config = lib.mkIf cfg.enable {
    boot.kernelParams = lib.filter (p: p != "") (lib.splitString " " cfg.cmdline);
    files."/etc/mkinitcpio.conf.d/50-boot.conf".text = ''
      COMPRESSION="${cfg.compression}"
    '';
  };
}
