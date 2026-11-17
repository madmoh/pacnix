# Mounts and swap compile to /etc/fstab, which systemd-fstab-generator reads,
# and zram swap to the configuration of zram-generator.
{ config, lib, ... }:
let
  inherit (lib) mkOption types;
  cfg = config.fileSystems;
  sized = lib.filter (s: s.size != null) config.swapDevices;
  pass = fs: if lib.elem fs.fsType [ "ext2" "ext3" "ext4" "vfat" ] then (if fs.mountPoint == "/" then "1" else "2") else "0";
  value = v: if lib.isBool v then lib.boolToString v else toString v;
in
{
  options.fileSystems = mkOption {
    default = { };
    example = { "/" = { device = "UUID=0a3407de-014b-458b-b5c1-848e92a327a3"; fsType = "ext4"; }; };
    description = "File systems to mount, by mount point. With a declared file system or swap device, pacnix writes all of /etc/fstab. A new mount is active after the next boot. pacnix does not make partitions or file systems.";
    type = types.attrsOf (types.submodule ({ name, ... }: {
      options = {
        mountPoint = mkOption { type = types.str; default = name; };
        device = mkOption { type = types.str; example = "LABEL=root"; };
        fsType = mkOption { type = types.str; example = "ext4"; };
        options = mkOption { type = types.listOf types.str; default = [ "defaults" ]; };
      };
    }));
  };
  options.swapDevices = mkOption {
    default = [ ];
    description = "Swap partitions and swap files, in /etc/fstab. A swap file with a size that does not exist, pacnix makes with mkswap. When its declaration goes, the swap file is data of a removed feature.";
    type = types.listOf (types.submodule {
      options = {
        device = mkOption { type = types.str; example = "/swap/swapfile"; };
        size = mkOption {
          type = types.nullOr types.int;
          default = null;
          description = "Size of a swap file in MiB. null: the device or file exists.";
        };
      };
    });
  };
  options.zramSwap = {
    enable = mkOption { type = types.bool; default = false; description = "Swap on compressed memory with zram-generator."; };
    settings = mkOption {
      type = types.attrsOf (types.oneOf [ types.str types.int types.bool ]);
      default = { zram-size = "min(ram / 2, 4096)"; compression-algorithm = "zstd"; };
      description = "Settings of the device zram0, see zram-generator.conf(5).";
    };
  };
  config = lib.mkMerge [
    (lib.mkIf (cfg != { } || config.swapDevices != [ ]) {
      files."/etc/fstab".text = lib.concatStrings (
        map (fs: "${fs.device} ${fs.mountPoint} ${fs.fsType} ${lib.concatStringsSep "," fs.options} 0 ${pass fs}\n") (lib.attrValues cfg)
        ++ map (s: "${s.device} none swap defaults 0 0\n") config.swapDevices);
      triggers.fstab = {
        paths = [ "/etc/fstab" ];
        command = lib.concatMapStrings (s: "test -e ${s.device} || mkswap --file ${s.device} --size ${toString s.size}M >/dev/null; ") sized
          + "systemctl daemon-reload";
      };
      claims = lib.listToAttrs (map (s: lib.nameValuePair "swap file ${s.device}" { system = [ s.device ]; }) sized);
    })
    (lib.mkIf config.zramSwap.enable {
      packages.repo = [ "zram-generator" ];
      files."/etc/systemd/zram-generator.conf".text = "[zram0]\n" + lib.concatStrings (lib.mapAttrsToList (k: v: "${k} = ${value v}\n") config.zramSwap.settings);
      triggers.zram = { paths = [ "/etc/systemd/zram-generator.conf" ]; command = "systemctl daemon-reload && systemctl start dev-zram0.swap"; };
    })
  ];
}
