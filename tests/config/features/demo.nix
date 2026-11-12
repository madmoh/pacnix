# A small web service: a package, a unit file, a drop-in for a unit of a
# different package, a sysctl file and state that the service creates.
{ config, lib, ... }:
{
  options.features.demo.enable = lib.mkEnableOption "the demo web service";
  config = lib.mkIf config.features.demo.enable {
    packages.repo = [ "darkhttpd" ];
    files."/etc/systemd/system/demo-web.service".text = ''
      [Unit]
      Description=Demo web server
      [Service]
      ExecStartPre=/usr/bin/mkdir -p /srv/demo
      ExecStartPre=/usr/bin/sh -c 'date > /srv/demo/index.html'
      ExecStart=/usr/bin/darkhttpd /srv/demo --port 8081
      [Install]
      WantedBy=multi-user.target
    '';
    files."/etc/systemd/system/sshd.service.d/demo.conf".text = ''
      [Service]
      Environment=DEMO=1
    '';
    files."/etc/sysctl.d/90-demo.conf".text = "net.core.somaxconn = 8192\n";
    triggers.sysctl = { paths = [ "/etc/sysctl.d/" ]; command = "sysctl --system >/dev/null"; };
    units.enable = [ "demo-web.service" ];
    claims.demo.system = [ "/srv/demo" ];
    claims.demo.home = [ ".cache/demo" ];
  };
}
