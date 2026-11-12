{
  inputs.pacnix.url = "path:/usr/share/pacnix";
  outputs = { self, pacnix, ... }:
    let
      inherit (pacnix.inputs.nixpkgs) lib;
      features = map (f: ./features + "/${f}") (lib.filter (lib.hasSuffix ".nix") (builtins.attrNames (builtins.readDir ./features)));
      # Scenario scripts write scenario.nix to change the configuration of a test VM.
      host = name: pacnix.lib.pacnixSystem {
        modules = features ++ [ (if builtins.pathExists ./hosts/${name}.nix then ./hosts/${name}.nix else ./hosts/generic.nix) { hostName = name; } ] ++ lib.optional (builtins.pathExists ./scenario.nix) ./scenario.nix;
        specialArgs.hosts = self.pacnixConfigurations;
      };
    in
    {
      pacnixConfigurations = lib.genAttrs ([ "desktop" "laptop" "newpc" "workstation" "old" "newbox" ] ++ lib.genList (i: "s${lib.fixedWidthNumber 2 (i + 1)}") 20) host;
    };
}
