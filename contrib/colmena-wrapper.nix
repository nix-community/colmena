# SPDX-License-Identifier: CC0-1.0
# SPDX-FileCopyrightText: 2024 Jade Lovelace
#
# A wrapper for colmena that prevents accidentally deploying changes without
# having pulled.
{ colmena, runCommandNoCC }:
runCommandNoCC "colmena-wrapper"
{
  env = {
    colmena = "${colmena}/bin/colmena";
    remote_name = "origin";
    upstream_branch = "main";
  };
} ''
  mkdir -p $out
  ln -s ${colmena}/share $out/share
  mkdir $out/bin

  substituteAll ${./colmena-wrapper.sh.in} $out/bin/colmena
  chmod +x $out/bin/colmena
''
