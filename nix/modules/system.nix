# Host name, time zone, locales and console: the files of the Arch
# installation guide, and locale-gen after a change of /etc/locale.gen.
{ config, lib, ... }:
let
  inherit (lib) mkOption types;
  i18n = config.i18n;
in
{
  options.networking.hostName = mkOption { type = types.nullOr types.str; default = null; description = "Host name, in /etc/hostname."; };
  options.time.timeZone = mkOption { type = types.nullOr types.str; default = null; example = "Europe/Berlin"; description = "Time zone, the link /etc/localtime."; };
  options.i18n = {
    defaultLocale = mkOption { type = types.nullOr types.str; default = null; example = "en_US.UTF-8"; description = "LANG in /etc/locale.conf."; };
    extraLocaleSettings = mkOption { type = types.attrsOf types.str; default = { }; example = { LC_TIME = "en_GB.UTF-8"; }; description = "Other variables of /etc/locale.conf."; };
    supportedLocales = mkOption {
      type = types.listOf types.str;
      default = lib.optional (i18n.defaultLocale != null) "${i18n.defaultLocale} ${lib.last (lib.splitString "." i18n.defaultLocale)}";
      example = [ "en_US.UTF-8 UTF-8" "de_DE.UTF-8 UTF-8" ];
      description = "Lines of /etc/locale.gen. locale-gen runs after a change. The default has the default locale.";
    };
  };
  options.console = {
    keyMap = mkOption { type = types.nullOr types.str; default = null; example = "de-latin1"; description = "KEYMAP in /etc/vconsole.conf."; };
    font = mkOption { type = types.nullOr types.str; default = null; example = "ter-132b"; description = "FONT in /etc/vconsole.conf."; };
  };
  config = lib.mkMerge [
    (lib.mkIf (config.networking.hostName != null) {
      files."/etc/hostname".text = "${config.networking.hostName}\n";
      triggers.hostname = { paths = [ "/etc/hostname" ]; command = "cat /etc/hostname >/proc/sys/kernel/hostname"; };
    })
    (lib.mkIf (config.time.timeZone != null) {
      files."/etc/localtime".target = "/usr/share/zoneinfo/${config.time.timeZone}";
    })
    (lib.mkIf (i18n.supportedLocales != [ ]) {
      files."/etc/locale.gen".text = lib.concatMapStrings (l: "${l}\n") i18n.supportedLocales;
      triggers.locale-gen = { paths = [ "/etc/locale.gen" ]; command = "locale-gen >/dev/null"; };
    })
    (lib.mkIf (i18n.defaultLocale != null || i18n.extraLocaleSettings != { }) {
      files."/etc/locale.conf".text = lib.concatStrings (lib.mapAttrsToList (k: v: "${k}=${v}\n")
        (lib.optionalAttrs (i18n.defaultLocale != null) { LANG = i18n.defaultLocale; } // i18n.extraLocaleSettings));
    })
    (lib.mkIf (config.console.keyMap != null || config.console.font != null) {
      files."/etc/vconsole.conf".text = lib.optionalString (config.console.keyMap != null) "KEYMAP=${config.console.keyMap}\n"
        + lib.optionalString (config.console.font != null) "FONT=${config.console.font}\n";
      triggers.vconsole = { paths = [ "/etc/vconsole.conf" ]; command = "systemctl restart systemd-vconsole-setup.service"; };
    })
  ];
}
