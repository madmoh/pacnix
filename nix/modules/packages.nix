{ config, lib, ... }:
let
  inherit (lib) mkOption types;
  cfg = config.packages;
  names = cfg.repo ++ cfg.aur ++ lib.attrNames cfg.local ++ lib.attrNames cfg.providers ++ lib.attrValues cfg.providers;
  duplicates = lib.unique (lib.filter (n: lib.count (m: m == n) (lib.unique cfg.repo ++ lib.unique cfg.aur ++ lib.attrNames cfg.local) > 1) names);
in
{
  options.packages = {
    repo = mkOption {
      type = types.listOf types.str;
      default = [ ];
      description = "Packages from the sync repositories that are explicitly installed.";
    };
    aur = mkOption {
      type = types.listOf types.str;
      default = [ ];
      description = "AUR packages. pacnix builds them in a clean chroot into the local repository pacnix-local.";
    };
    local = mkOption {
      type = types.attrsOf types.path;
      default = { };
      description = "Local PKGBUILD directories, by package name. pacnix builds them like AUR packages.";
    };
    providers = mkOption {
      type = types.attrsOf types.str;
      default = { };
      example = { libpdfium = "pdfium-binaries-v8-bin"; };
      description = "The package that satisfies a dependency, by dependency name, as with the provider choice of pacman. pacnix installs it as a dependency. It comes from the sync repositories, from local, or else from the AUR.";
    };
    serve = {
      aur = mkOption {
        type = types.listOf types.str;
        default = [ ];
        description = "AUR packages that this host builds into its repository for other hosts. This host does not install them.";
      };
      local = mkOption {
        type = types.attrsOf types.path;
        default = { };
        description = "Local PKGBUILD directories that this host builds into its repository for other hosts.";
      };
    };
    server = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "http://desktop:8080";
      description = "URL of the pacnix-local repository of a different host. When set, this host installs AUR and local packages from that URL and builds nothing.";
    };
  };
  config.assertions = [
    {
      assertion = duplicates == [ ];
      message = "packages: these names are in more than one of repo, aur and local: ${toString duplicates}";
    }
    {
      assertion = cfg.server != null || (cfg.aur == [ ] && cfg.local == { } && cfg.serve.aur == [ ] && cfg.serve.local == { }) || lib.elem "aurutils" cfg.aur;
      message = "packages: a host that builds AUR or local packages declares aurutils in packages.aur. pacnix builds it first on a new host.";
    }
    {
      assertion = lib.all (n: builtins.match "[a-z0-9@._+-]+" n != null) names;
      message = "packages: invalid package names: ${toString (lib.filter (n: builtins.match "[a-z0-9@._+-]+" n == null) names)}";
    }
  ];
}
