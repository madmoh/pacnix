{ config, lib, ... }:
let
  inherit (lib) mkOption types;
  both = lib.intersectLists config.units.enable (config.units.mask ++ config.units.disable);
in
{
  options.units = {
    enable = mkOption {
      type = types.listOf types.str;
      default = [ ];
      description = "systemd system units that are enabled and started. The vendor presets of the packages decide the units that are not in enable or disable.";
    };
    disable = mkOption {
      type = types.listOf types.str;
      default = [ ];
      example = [ "systemd-networkd.service" ];
      description = "systemd system units that are disabled and stopped, also when a vendor preset enables them.";
    };
    mask = mkOption {
      type = types.listOf types.str;
      default = [ ];
      description = "systemd system units that are masked.";
    };
  };
  config.assertions = [{
    assertion = both == [ ];
    message = "units: these units are enabled and also masked or disabled: ${toString both}";
  }];
}
