# Shared image base environment, modelled on zeko/nix/docker.nix `mkBaseEnv`:
# one buildEnv placed in the `contents` of every settlement image so that
# /bin, /etc and /share carry the same interactive shell, CA bundle, timezone
# data, locales and core tools. Images add their application packages next to
# it and set TZ, TZDIR and SSL_CERT_FILE (nix/docker.nix, nix/bootstrap.nix).
{
  buildEnv,
  bashInteractive,
  cacert,
  openssl,
  tzdata,
  glibcLocales,
  coreutils,
  findutils,
  procps,
  shadow,
  which,
  curl,
  jq,
  gnutar,
  gzip,
}:
buildEnv {
  name = "zeko-settlement-docker-base-env";
  # zeko additionally lists dockerTools.shadowSetup; that attribute is a shell
  # snippet (a string, meant for buildImage runAsRoot), not a store path, and
  # buildEnv's builder skips entries that do not exist on disk. Only the shadow
  # package itself is linked here.
  paths = [
    bashInteractive
    cacert
    openssl
    tzdata
    glibcLocales
    coreutils
    findutils
    procps
    shadow
    which
    curl
    jq
    gnutar
    gzip
  ];
  pathsToLink = [
    "/bin"
    "/etc"
    "/share"
  ];
}
