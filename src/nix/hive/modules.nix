with builtins; {
    assertionModule = { config, lib, ... }: {
      assertions = lib.mapAttrsToList (key: opts: let
        nonNulls = l: filter (x: x != null) l;
      in {
        assertion = length (nonNulls [opts.text opts.keyCommand opts.keyFile]) == 1;
        message =
          let prefix = "${name}.deployment.keys.${key}";
          in "Exactly one of `${prefix}.text`, `${prefix}.keyCommand` and `${prefix}.keyFile` must be set.";
        }) config.deployment.keys;
    };

    # Change the ownership of all keys uploaded pre-activation
    #
    # This is built as part of the system profile.
    # We must be careful not to access `text` / `keyCommand` / `keyFile` here
    keyChownModule = { lib, config, ... }: let
      preActivationKeys = lib.filterAttrs (name: key: key.uploadAt == "pre-activation") config.deployment.keys;
      scriptDeps = if config.system.activationScripts ? groups then [ "groups" ] else [ "users" ];

      commands = lib.mapAttrsToList (name: key: let
        keyPath = "${key.destDir}/${key.name}";
      in ''
        if [ -f "${keyPath}" ]; then
          if ! chown ${key.user}:${key.group} "${keyPath}"; then
            # Error should be visible in stderr
            failed=1
          fi
        else
          >&2 echo "Key ${keyPath} does not exist. Skipping chown."
        fi
      '') preActivationKeys;

      script = lib.stringAfter scriptDeps ''
        # This script is injected by Colmena to change the ownerships
        # of keys (`deployment.keys`) deployed before system activation.

        >&2 echo "setting up key ownerships..."

        # We set the ownership of as many keys as possible before failing
        failed=

        ${concatStringsSep "\n" commands}

        if [ -n "$failed" ]; then
          >&2 echo "Failed to set the ownership of some keys."

          # The activation script has a trap to handle failed
          # commands and print out various debug information.
          # Let's trigger that instead of `exit 1`.
          false
        fi
      '';
    in {
      system.activationScripts.colmena-chown-keys = lib.mkIf (length commands != 0) script;
    };

    # Create "${name}-key" services for NixOps compatibility
    #
    # This is built as part of the system profile.
    # We must be careful not to access `text` / `keyCommand` / `keyFile` here
    #
    # Sadly, path units don't automatically deactivate the bound units when
    # the key files are deleted, so we use inotifywait in the services' scripts.
    #
    # <https://github.com/systemd/systemd/issues/3642>
    keyServiceModule = { pkgs, lib, config, ... }: {
      systemd.paths = lib.mapAttrs' (name: val: {
        name = config.serviceName;
        value = {
          wantedBy = [ "paths.target" ];
          pathConfig = {
            PathExists = val.path;
          };
        };
      }) config.deployment.keys;

      systemd.services = lib.mapAttrs' (name: val: {
        name = config.serviceName;
        value = {
          enable = true;
          serviceConfig = {
            TimeoutStartSec = "infinity";
            Restart = "always";
            RestartSec = "100ms";
          };
          path = [ pkgs.inotify-tools ];
          preStart = ''
            (while read f; do if [ "$f" = "${val.name}" ]; then break; fi; done \
              < <(inotifywait -qm --format '%f' -e create,move ${val.destDir}) ) &
            if [[ -e "${val.path}" ]]; then
              echo 'flapped down'
              kill %1
              exit 0
            fi
            wait %1
          '';
          script = ''
            inotifywait -qq -e delete_self "${val.path}" &
            if [[ ! -e "${val.path}" ]]; then
              echo 'flapped up'
              exit 0
            fi
            wait %1
          '';
        };
      }) config.deployment.keys;
    };
}
