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
  homes = lib.mapAttrs home cfg.home.users;
  home = user: module: home-manager.lib.homeManagerConfiguration {
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
        # Root links home.path to /etc/profiles/per-user/USER, as NixOS does, and
        # writes the profile and the GC roots: the activation needs no write to
        # the Nix store and no nix-daemon.
        submoduleSupport.enable = true;
        submoduleSupport.externalPackageInstall = true;
        home.activationGenerateGcRoot = false;
      }
      # Home Manager resets the dconf keys that leave a database only when the new
      # generation still has that database. This resets, in the same way, the keys
      # of each database that the old generation has and the new one does not. It
      # runs before dconfSettings, thus a database that stays loads its keys after
      # the reset also when two databases use the same file.
      ({ lib, pkgs, ... }: {
        home.activation.pacnixDconfCleanup = lib.hm.dag.entryBetween [ "dconfSettings" ] [ "installPackages" ] ''
          for old in "''${oldGenPath:-/nonexistent}"/state/dconf-keys*.json; do
            [[ -f $old && ! -e $newGenPath/state/''${old##*/} ]] || continue
            name=''${old##*/dconf-keys}; name=''${name%.json}; name=''${name#-}
            bus=() profile=()
            [[ -v DBUS_SESSION_BUS_ADDRESS ]] || bus=(${pkgs.dbus}/bin/dbus-run-session --dbus-daemon=${pkgs.dbus}/bin/dbus-daemon)
            if [[ -n $name ]]; then
              file=$(mktemp) && printf 'user-db:%s\n' "$name" >"$file" && profile=("DCONF_PROFILE=$file")
            fi
            ${pkgs.jq}/bin/jq -r '.[]' "$old" | while read -r key; do
              verboseEcho "Resetting dconf key \"$key\""
              run "''${bus[@]}" env "''${profile[@]}" ${pkgs.dconf}/bin/dconf reset "$key"
            done
            [[ -z $name ]] || rm -f "$file"
          done
        '';
      })
      # pacman installs the software. An enabled program or service gets no
      # package from Nix: null when its package option permits it, else a
      # package that only links to the program of pacman in /usr/bin.
      ({ config, options, ... }: {
        config = lib.genAttrs [ "programs" "services" ] (kind: lib.mapAttrs (name: o: {
          package = lib.mkIf config.${kind}.${name}.enable (lib.mkDefault (if o.package.type.check null then null else host o.package.default));
        }) (lib.filterAttrs (_: o: visible o && (o.package.type.check null || o.package ? default)) options.${kind}));
      })
    ];
  };
  # The home files that an empty home configuration of the user also has, for example ~/.cache/.keep.
  support = user: hm: let empty = home user { home.homeDirectory = hm.config.home.homeDirectory; }; in
    map (f: f.target) (lib.filter (f: f.enable) (lib.attrValues empty.config.home.file));
  # Home Manager always adds hm-session-vars.sh, a generated shell file.
  homePackages = hm: lib.filter (p: lib.getName p != "hm-session-vars.sh" && !(p.pacnixHost or false)) hm.config.home.packages;
  # A package with only a link to the program of the same name that pacman installs.
  host = p: let main = p.meta.mainProgram or (lib.getName p); in pkgs.runCommandLocal "pacman-${main}" {
    # Modules read the version, for example to choose the shell integration.
    version = p.version or "";
    meta.mainProgram = main;
    passthru.pacnixHost = true;
  } "mkdir -p $out/bin && ln -s /usr/bin/${main} $out/bin/${main}";
  # The enabled programs and services whose package option puts a package into home.packages.
  packageOptions = hm: lib.concatLists (map (kind: lib.mapAttrsToList (name: _: "${kind}.${name}")
    (lib.filterAttrs (name: o: visible o && hm.config.${kind}.${name}.enable && lib.elem hm.config.${kind}.${name}.package (homePackages hm)) hm.options.${kind})) [ "programs" "services" ]);
  # A program or service with an enable and a package option that are not removed or renamed.
  visible = o: lib.isAttrs o && !lib.isOption o && lib.all (n: lib.isOption (o.${n} or null) && o.${n}.visible or true != false) [ "enable" "package" ];
  failed = map (a: a.message) (lib.filter (a: !a.assertion) (cfg.assertions ++ lib.concatLists (lib.mapAttrsToList (user: hm: [{
    assertion = homePackages hm == [ ];
    message = "home.users.${user}: Home Manager installs these packages from Nix: ${toString (map lib.getName (homePackages hm))}. pacnix installs software only with pacman. Remove them from home.packages${lib.optionalString (packageOptions hm != [ ]) " and remove ${lib.concatStringsSep ", " (packageOptions hm)}"}, and declare them in packages.repo or packages.aur.";
  }]) homes)));
  storeName = path: lib.strings.sanitizeDerivationName (baseNameOf path);
  manifest = {
    version = 1;
    packages = {
      repo = lib.unique cfg.packages.repo;
      aur = lib.unique cfg.packages.aur;
      local = lib.mapAttrs (name: dir: "${builtins.path { path = dir; name = storeName name; }}") cfg.packages.local;
      server = cfg.packages.server;
      serverKey = cfg.packages.serverKey;
      keepBuildCache = lib.unique cfg.packages.keepBuildCache;
      providers = cfg.packages.providers;
      versions = cfg.packages.versions;
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
    users = {
      users = lib.mapAttrs (name: u: { inherit (u) uid group extraGroups shell home description; }) cfg.users.users;
      groups = lib.mapAttrs (name: g: { inherit (g) gid; }) cfg.users.groups;
    };
    triggers = cfg.triggers;
    mounts = cfg.mounts;
    boot = {
      secureBoot = { inherit (cfg.boot.loader.systemd-boot.secureBoot) enable dbAdditions files; esp = cfg.boot.loader.efi.efiSysMountPoint; };
      luks = lib.mapAttrs (name: d: { inherit (d) device recoveryKey; tpm2 = { inherit (d.tpm2) enable pcrs pin; }; }) cfg.boot.initrd.luks.devices;
    };
    claims = cfg.claims;
    installNotes = cfg.installNotes;
    audit.ignore = lib.unique cfg.audit.ignore;
    home = lib.mapAttrs (user: hm: {
      directory = hm.config.home.homeDirectory;
      drv = builtins.unsafeDiscardStringContext hm.activationPackage.drvPath;
      path = builtins.unsafeDiscardStringContext hm.config.home.path.drvPath;
      support = support user hm;
    }) homes;
  };
in
eval // {
  inherit homes;
  generation =
    if failed != [ ] then throw (lib.concatStringsSep "\n" failed)
    else builtins.toFile "pacnix-generation.json" (builtins.toJSON manifest);
}
