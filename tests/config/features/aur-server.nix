# Serves the local pacnix repository over HTTP and builds the AUR and local
# packages of the hosts that use this host as their server.
{ config, lib, hosts, ... }:
let
  cfg = config.features.aur-server;
  clients = lib.filter (h: h.config.packages.server == cfg.url) (lib.attrValues (removeAttrs hosts [ config.hostName ]));
in
{
  options.features.aur-server = {
    enable = lib.mkEnableOption "the HTTP server of the local repository";
    url = lib.mkOption {
      type = lib.types.str;
      description = "URL that clients set in packages.server.";
    };
  };
  config = lib.mkIf cfg.enable {
    packages.repo = [ "darkhttpd" ];
    packages.serve.aur = lib.concatMap (h: h.config.packages.aur) clients;
    packages.serve.local = lib.foldl' (a: h: a // h.config.packages.local) { } clients;
    files."/etc/systemd/system/pacnix-repo-http.service".text = ''
      [Unit]
      Description=HTTP server of the pacnix repository
      After=network.target
      [Service]
      ExecStart=/usr/bin/darkhttpd /var/lib/pacnix/repo --port 8080 --uid nobody --gid nobody
      [Install]
      WantedBy=multi-user.target
    '';
    units.enable = [ "pacnix-repo-http.service" ];
  };
}
