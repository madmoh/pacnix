# Remote access: SSH, LAN discovery and Tailscale. The role is per host and
# sets the lid switch action. Tailscale login is out of scope.
{ config, lib, ... }:
let
  cfg = config.features.remote;
in
{
  options.features.remote = {
    enable = lib.mkEnableOption "SSH, LAN discovery and Tailscale";
    role = lib.mkOption {
      type = lib.types.enum [ "stationary" "mobile" ];
      default = "mobile";
    };
  };
  config = lib.mkIf cfg.enable {
    packages.repo = [ "avahi" "nss-mdns" "openssh" "tailscale" ];
    files."/etc/ssh/sshd_config.d/10-remote.conf".text = ''
      PasswordAuthentication no
      KbdInteractiveAuthentication no
    '';
    files."/etc/avahi/avahi-daemon.conf".text = ''
      [server]
      use-ipv6=yes
      deny-interfaces=tailscale0
      [publish]
      publish-workstation=yes
    '';
    files."/etc/systemd/logind.conf.d/50-lid.conf".text = ''
      [Login]
      HandleLidSwitch=${if cfg.role == "stationary" then "ignore" else "suspend"}
    '';
    units.enable = [ "sshd.service" "avahi-daemon.service" "tailscaled.service" ];
    triggers.sshd = { paths = [ "/etc/ssh/" ]; command = "sshd -t && systemctl reload-or-restart sshd.service"; };
    triggers.avahi = { paths = [ "/etc/avahi/" ]; command = "systemctl try-restart avahi-daemon.service"; };
    claims.remote.system = [ "/var/lib/tailscale" "/var/cache/tailscale" "/etc/ssh/ssh_host_*" ];
  };
}
