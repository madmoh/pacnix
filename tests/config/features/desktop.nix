# A desktop with its display manager. Scenario 17 switches it. Small window
# managers stand for the desktops: the switch of the packages and of the
# display-manager.service alias is the same.
{ config, lib, ... }:
let
  desktops = {
    fluxbox = { packages = [ "fluxbox" "greetd" ]; unit = "greetd.service"; };
    i3 = { packages = [ "i3-wm" "lightdm" ]; unit = "lightdm.service"; };
  };
  d = config.features.desktop;
in
{
  options.features.desktop = lib.mkOption {
    type = lib.types.nullOr (lib.types.enum (lib.attrNames desktops));
    default = null;
  };
  config = lib.mkIf (d != null) {
    packages.repo = desktops.${d}.packages;
    units.enable = [ desktops.${d}.unit ];
  };
}
