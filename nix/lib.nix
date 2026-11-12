# pacnixSystem { modules, specialArgs ? { } } evaluates a host the way nixosSystem does.
# The result has config, options and generation: the store path of the manifest that
# pacnix applies. The manifest references every managed file in the store.
{ nixpkgs, home-manager }:
{ modules, specialArgs ? { }, system ? "x86_64-linux" }:
let
  inherit (nixpkgs) lib;
  eval = lib.evalModules {
    modules = [ ./modules ] ++ modules;
    inherit specialArgs;
  };
  cfg = eval.config;
  pkgs = nixpkgs.legacyPackages.${system};
  homes = lib.mapAttrs (user: module: home-manager.lib.homeManagerConfiguration {
    inherit pkgs;
    modules = [
      module
      {
        home.username = lib.mkDefault user;
        home.homeDirectory = lib.mkDefault "/home/${user}";
        home.stateVersion = lib.mkDefault lib.trivial.release;
        programs.man.enable = lib.mkDefault false;
        manual.manpages.enable = lib.mkDefault false;
        xdg.mime.enable = lib.mkDefault false;
      }
    ];
  }) cfg.home.users;
  # Home Manager always adds hm-session-vars.sh, a generated shell file.
  homePackages = hm: lib.filter (p: lib.getName p != "hm-session-vars.sh") hm.config.home.packages;
  failed = map (a: a.message) (lib.filter (a: !a.assertion) (cfg.assertions ++ lib.concatLists (lib.mapAttrsToList (user: hm: [{
    assertion = homePackages hm == [ ];
    message = "home.users.${user}: home.packages must stay empty, install software with packages.repo or packages.aur. Set the package option of each enabled program to null. Found: ${toString (map lib.getName (homePackages hm))}";
  }]) homes)));
  storeName = path: lib.strings.sanitizeDerivationName (baseNameOf path);
  manifest = {
    version = 1;
    packages = {
      repo = lib.unique cfg.packages.repo;
      aur = lib.unique cfg.packages.aur;
      local = lib.mapAttrs (name: dir: "${builtins.path { path = dir; name = storeName name; }}") cfg.packages.local;
      server = cfg.packages.server;
      providers = cfg.packages.providers;
      serve = {
        aur = lib.unique cfg.packages.serve.aur;
        local = lib.mapAttrs (name: dir: "${builtins.path { path = dir; name = storeName name; }}") cfg.packages.serve.local;
      };
    };
    files = lib.mapAttrs (path: f: {
      source =
        if f.text != null then builtins.toFile (storeName path) f.text
        else if f.source != null then "${builtins.path { path = f.source; name = storeName path; }}"
        else "";
      inherit (f) target mode user group;
    }) cfg.files;
    units = { enable = lib.unique cfg.units.enable; disable = lib.unique cfg.units.disable; mask = lib.unique cfg.units.mask; };
    triggers = cfg.triggers;
    claims = cfg.claims;
    audit.ignore = lib.unique cfg.audit.ignore;
    home = lib.mapAttrs (user: hm: {
      directory = hm.config.home.homeDirectory;
      drv = builtins.unsafeDiscardStringContext hm.activationPackage.drvPath;
    }) homes;
  };
in
eval // {
  inherit homes;
  generation =
    if failed != [ ] then throw "pacnix: failed assertions:\n- ${lib.concatStringsSep "\n- " failed}"
    else builtins.toFile "pacnix-generation.json" (builtins.toJSON manifest);
}
