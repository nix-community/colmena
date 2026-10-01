# Setup injected here

poison = " ".join(["this", "must", "not", "appear", "in", "the", "nix", "store"])
deployer.succeed(f"echo '{poison}' > /tmp/bundle/key-file")
deployer.succeed(f"sed -i 's|@poison@|{poison}|g' /tmp/bundle/hive.nix")

targets = [alpha, beta, gamma]

logs = deployer.succeed("cd /tmp/bundle &&" \
    f"run-copy-stderr {colmena} apply --evaluator {evaluator} --eval-node-limit 4 --on @target --keep-result")

with subtest("Check that evaluation messages were logged correctly"):
    assert "must appear during evaluation" in logs

with subtest("Check that build messages were logged correctly"):
    assert "must appear during build" in logs

with subtest("Check that push messages were logged correctly"):
    assert "copying path" in logs

with subtest("Check that activation messages were logged correctly"):
    assert "must appear during activation" in logs

with subtest("Check that GC roots are created correctly"):
    for node in targets:
        profile = node.succeed("readlink /run/current-system")
        gcroot = deployer.succeed(f"readlink /tmp/bundle/.gcroots/node-{node.name}")
        assert gcroot == profile

with subtest("Check that we can still connect to the target nodes"):
    for node in targets:
        deployer.succeed(f"ssh {node.name} true")

with subtest("Check that the new configurations are indeed applied"):
    for node in targets:
        node.succeed("grep FIRST /etc/deployment")

with subtest("Check that key files have correct contents"):
    contents = {
        "/run/keys/custom-name":            poison,
        "/run/keys/key-text":               poison,
        "/tmp/another-key-dir/key-command": "deployer",
        "/tmp/another-key-dir/key-file":    poison,
        "/tmp/another-key-dir/key-file-2":  poison,
        "/run/keys/pre-activation":         "pre-activation key",
        "/run/keys/post-activation":        "post-activation key",
    }

    for node in targets:
        for path, content in contents.items():
            node.succeed(f"grep '{content}' '{path}'")

with subtest("Check that key files have correct permissions"):
    permissions = {
        "/run/keys/custom-name":            "600 root root",
        "/run/keys/key-text":               "600 root root",
        "/tmp/another-key-dir/key-command": "600 root root",
        "/tmp/another-key-dir/key-file":    "600 root root",
        "/tmp/another-key-dir/key-file-2":  "600 root root",
        "/run/keys/pre-activation":         "640 testuser testgroup",
        "/run/keys/post-activation":        "600 testuser testgroup",
    }

    for node in targets:
        node.succeed("getent passwd testuser")
        node.succeed("getent group testgroup")

        for path, permission in permissions.items():
            node.succeed(f"if [[ \"{permission}\" != \"$(stat -c '%a %U %G' '{path}')\" ]]; then ls -lah '{path}'; exit 1; fi")

with subtest("Check that the key service is started for post-activation keys"):
    alpha.wait_for_unit("post-activation-key.service")

with subtest("Check that key services respond to key file changes"):
    alpha.wait_for_unit("key-text-key.service")

    alpha.succeed("rm /run/keys/key-text")
    alpha.require_unit_state("key-text-key.service", "activating")

    alpha.succeed("touch /run/keys/key-text")
    alpha.wait_for_unit("key-text-key.service")

with subtest("Check that we can correctly deploy to remaining nodes despite failures"):
    beta.systemctl("stop sshd")

    deployer.succeed("sed -i s/FIRST/SECOND/g /tmp/bundle/hive.nix")
    deployer.fail("cd /tmp/bundle &&" \
        f"{colmena} apply --evaluator {evaluator} --eval-node-limit 4 --on @target")

    alpha.succeed("grep SECOND /etc/deployment")
    beta.succeed("grep FIRST /etc/deployment")
    gamma.succeed("grep SECOND /etc/deployment")

with subtest("Check that key contents are not in the Nix store"):
    new_store_paths = " ".join(get_new_store_paths())

    ret, stdout = deployer.execute(f"grep -r '{poison}' {new_store_paths}")

    if ret != 1:
        deployer.log("Forbidden text found in: " + stdout)

    assert ret == 1

with subtest("Check that our Nix store test is actually working"):
    deployer.succeed(f"nix-build -E 'with import <nixpkgs> {{}}; writeText \"forbidden-text.txt\" \"{poison}\"'")
    new_store_paths = " ".join(get_new_store_paths())
    deployer.succeed(f"grep -r '{poison}' {new_store_paths}")

with subtest("Check that we can build nodes with dots in their names"):
    deployer.succeed("cd /tmp/bundle &&" \
        f"{colmena} build --evaluator {evaluator} --eval-node-limit 4 --on gamma.tld")

# eval, instantiate and the repl ignore --evaluator
if evaluator == "chunked":
    with subtest("Check that eval, instantiate and the repl work"):
        deployer.succeed("echo '{ nodes, ... }: builtins.attrNames nodes' > /tmp/names.nix")
        names = deployer.succeed(f"cd /tmp/bundle && {colmena} eval /tmp/names.nix")
        assert "alpha" in names

        drv = deployer.succeed("cd /tmp/bundle &&" \
            f"{colmena} eval --instantiate -E '{{ nodes, ... }}: nodes.alpha.config.system.build.toplevel'")
        assert drv.strip().endswith(".drv")

        repl = deployer.succeed(f"cd /tmp/bundle && echo 'builtins.attrNames nodes' | {colmena} repl")
        assert "alpha" in repl
