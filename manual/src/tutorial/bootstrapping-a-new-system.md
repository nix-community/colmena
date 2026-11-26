# Bootstrapping a new install

Colmena doesn't support a `nixos-install` like interface out-of-the-box, but you can easily pass the closure that
Colmena builds to `nixos-install` directly!

This tutorial assumes you have SSH access to your live ISO

First, create your partitions and make your filesystems as you normally would, then mount them and run

```sh
nixos-generate-config --root /mnt # replace /mnt to wherever your root filesystem is mounted at
```

Or if you already have a configuration ready, simply output your hardware configuration to the console.
```
nixos-generate-config --root /mnt --show-hardware-config
```

For those using third-party secret management tools like `agenix`, it is necessary to generate your host
keys and make your configuration aware of it before copying the closure to the ISO

```sh
$ mkdir -p /mnt/etc/ssh
$ ssh-keygen -A -f /mnt # replace /mnt with your root filesystem mount
```

After you've adjusted your configuration to fit the needs of the new node, you can go ahead
and build the closure for the new system!

```sh
colmena build --on node --no-build-on-target
```

Copy the closure over to the running live NixOS ISO!
```sh
nix --extra-experimental-features 'nix-command' copy /nix/store/myclosure --to ssh://live@iso
```

Finally, you can pass the closure to `nixos-install` to be installed on your new node!
```sh
nixos-install --system /nix/store/myclosure
````

You can proceed with installation as you would normally without using Colmena!
