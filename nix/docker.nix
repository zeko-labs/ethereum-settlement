{
  lib,
  writeText,
  dockerTools,
  tzdata,
  rustPlatform,
  stdenvNoCC,
  clang,
  cmake,
  go,
  llvmPackages,
  pkg-config,
  protobuf,
  nodejs,
  pnpm_10,
  fetchCrate,
  buildGoModule,
  foundry,
  bash,
  source,
  sourceRevision,
  rustToolchain,
  callPackage,
}:
let
  # The gateway binary loads per-rollup public artifacts (settlement-program,
  # bridge-program, settlement-vk.serde.json) from this directory at startup;
  # nothing rollup-specific is baked into the image. Keep in sync with
  # script/src/lib.rs DEFAULT_ARTIFACTS_DIR.
  artifactsDir = "/var/lib/zeko-proof-api/artifacts";
  # Fail closed if operator runtime data was placed inside the checkout. By the
  # time this is evaluated a `path:.` flake reference has already copied the
  # entire checkout (tracked or not) into the world-readable Nix store, so no
  # source filter can protect such files; aborting every output makes the
  # mistake unmissable. The bootstrap workspace must live outside all source
  # checkouts (nix/README.md "Operator workspace"). Only existence is checked.
  repoLocalRuntimePaths = lib.filter (name: builtins.pathExists (source + "/nix/bootstrap/${name}")) [
    ".env"
    "secrets"
    "config"
    "state"
    "tls"
    "out"
  ];
  noRepoLocalRuntimeData =
    if repoLocalRuntimePaths != [ ] then
      throw ''Refusing to evaluate: operator runtime data exists inside the checkout at nix/bootstrap/{${lib.concatStringsSep ", " repoLocalRuntimePaths}}. These paths have already been copied into the Nix store by the path: flake reference. Move the bootstrap workspace outside every source checkout, remove the copies from the checkout and consider the ingested store paths compromised (nix/README.md "Operator workspace").''
    else
      true;

  # Shared /bin, /etc and /share base of every image (shell, CA bundle,
  # timezone data, core tools); see nix/base-env.nix. The matching image
  # environment is `baseImageEnv` below.
  baseEnv = callPackage ./base-env.nix { };
  baseImageEnv = [
    "TZ=UTC"
    "TZDIR=${tzdata}/share/zoneinfo"
    "SSL_CERT_FILE=/etc/ssl/certs/ca-bundle.crt"
  ];

  # Ethereum-side preparation tooling (zeko-bootstrap image: Foundry with the
  # bundled contracts, vkey, in-container SP1 guest build) and the pinned SP1
  # toolchain output; see nix/bootstrap.nix.
  bootstrap = callPackage ./bootstrap.nix {
    inherit
      bash
      source
      sourceRevision
      rustToolchain
      foundry
      gatewayPackage
      artifactsDir
      baseEnv
      baseImageEnv
      ;
  };
  sp1RecursionGnarkFfiSrc = fetchCrate {
    pname = "sp1-recursion-gnark-ffi";
    version = "6.1.0";
    hash = "sha256-kg/xbjFTfVkPGi5Q2/suz9LbV/rrZ3O6+bSVRsg1vT0=";
  };
  sp1RecursionGnarkFfiGo = buildGoModule {
    pname = "sp1-recursion-gnark-ffi-go-modules";
    version = "6.1.0";
    src = sp1RecursionGnarkFfiSrc;
    modRoot = "go";
    vendorHash = "sha256-hPAu1k9v/yP8EaAtt8pIP6sWfKzFa+pJ2BxIlGG2LdM=";
    subPackages = [ ];
  };
  # The SP1 6.1.0 `sp1-core-executor-runner` build script (build.rs PATH 2)
  # runs a nested `cargo metadata` and `cargo build` from its own vendored
  # crate directory to compile `sp1-core-executor-runner-binary` and embed it.
  # Under importCargoLock that nested cargo also reads
  # cargo-vendor-dir/.cargo/config.toml, whose `directory = "cargo-vendor-dir"`
  # is relative to that file, so it resolves to
  # /build/cargo-vendor-dir/cargo-vendor-dir and the gateway build fails. The
  # build script's documented alternative (PATH 1) is the
  # SP1_CORE_RUNNER_OVERRIDE_BINARY environment variable: it skips the nested
  # cargo (the actual fix), embeds nothing and compiles the absolute path into
  # the runner, which spawns it at runtime only under the native-executor
  # configuration. Build the helper crate here as its own package from its
  # published source and locked dependencies (fetchCargoVendor writes an
  # absolute vendor directory, and this crate's build script runs no cargo), with
  # the same toolchain and rustPlatform as the gateway. zkapp-script enables the
  # sp1-sdk `profiling` feature, which selects the portable executor and
  # compiles the override path literal out, so the helper is a build input and
  # is not necessarily referenced from the image closure.
  sp1CoreExecutorRunnerBinary = rustPlatform.buildRustPackage {
    pname = "sp1-core-executor-runner-binary";
    version = "6.1.0";
    src = fetchCrate {
      pname = "sp1-core-executor-runner-binary";
      version = "6.1.0";
      hash = "sha256-rOmUBSLZoewxVRmFAk3l+42StYurb0WDALnhYIOxaFI=";
    };
    cargoHash = "sha256-1OxrJMDc3QgWT2RwLnvh1kC7hTURC5IJ894SqmDtQr4=";
    cargoBuildFlags = [ "--ignore-rust-version" ];
    doCheck = false;
    CARGO_BUILD_IGNORE_RUST_VERSION = "true";
    CARGO_BUILD_JOBS = "1";
    RAYON_NUM_THREADS = "1";
    HOME = "/build";
  };
  gatewayPackage = rustPlatform.buildRustPackage {
    pname = "zeko-proof-api";
    version = "0.1.0";
    src = source;
    cargoLock = {
      lockFile = "${source}/Cargo.lock";
      outputHashes = {
        "crypto-bigint-0.5.5" = "sha256-jtN5kCTIjNxzIAsJ24wxe2qFE60IZNfQ9OOnprko7p8=";
      };
    };
    # One hook-managed `cargo build` produces both runtime binaries: the
    # gateway and the generic vkey tool (both read the mounted artifacts
    # directory at runtime). The `--bin` filters apply across the two selected
    # packages, so no other zkapp-script binary is built. cargoBuildHook adds
    # `--target x86_64-unknown-linux-gnu`, and cargoInstallHook installs every
    # executable found in target/x86_64-unknown-linux-gnu/release, so no
    # postBuild/postInstall step is needed (a manual `install` from
    # target/release fails: that directory does not exist with --target).
    cargoBuildFlags = [
      "--ignore-rust-version"
      "--locked"
      "-p"
      "zeko-proof-api"
      "-p"
      "zkapp-script"
      "--bin"
      "zeko-proof-api"
      "--bin"
      "vkey"
    ];
    doCheck = false;
    nativeBuildInputs = [
      clang
      cmake
      go
      llvmPackages.libclang
      pkg-config
      protobuf
    ];
    LIBCLANG_PATH = "${llvmPackages.libclang.lib}/lib";
    CARGO_BUILD_IGNORE_RUST_VERSION = "true";
    CARGO_BUILD_JOBS = "1";
    RAYON_NUM_THREADS = "1";
    HOME = "/build";
    GOCACHE = "/build/go-cache";
    # Consumed by sp1-core-executor-runner's build script (see
    # sp1CoreExecutorRunnerBinary above). The file must exist at build time; the
    # build script canonicalizes it and embeds the path.
    SP1_CORE_RUNNER_OVERRIDE_BINARY = "${sp1CoreExecutorRunnerBinary}/bin/sp1-core-executor-runner-binary";
    preBuild = ''
      cp -r ${sp1RecursionGnarkFfiGo.goModules} /build/cargo-vendor-dir/sp1-recursion-gnark-ffi-6.1.0/go/vendor
      chmod -R u+w /build/cargo-vendor-dir/sp1-recursion-gnark-ffi-6.1.0/go/vendor
    '';
  };

  staticServer =
    {
      name,
      root,
      headers,
      runtimeConfigEnv,
    }:
    let
      server = writeText "${name}-server.mjs" ''
        import { createServer } from "node:http";
        import { readFile, stat } from "node:fs/promises";
        import { extname, join, resolve, sep } from "node:path";

        const root = ${builtins.toJSON root};
        const runtimeConfigPath = process.env.${runtimeConfigEnv} ?? "/run/zeko/runtime-config.json";
        const host = process.env.HOST ?? "0.0.0.0";
        const port = Number(process.env.PORT ?? "${if name == "zeko-bridge-ui" then "4174" else "4175"}");
        if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error("PORT must be a valid TCP port");
        const contentTypes = {
          ".avif": "image/avif",
          ".css": "text/css; charset=utf-8",
          ".gif": "image/gif",
          ".html": "text/html; charset=utf-8",
          ".ico": "image/x-icon",
          ".jpeg": "image/jpeg",
          ".jpg": "image/jpeg",
          ".js": "text/javascript; charset=utf-8",
          ".json": "application/json; charset=utf-8",
          ".png": "image/png",
          ".svg": "image/svg+xml",
          ".wasm": "application/wasm",
          ".webm": "video/webm",
          ".woff": "font/woff",
          ".woff2": "font/woff2"
        };
        const send = async (request, response) => {
          let requestPath;
          try {
            requestPath = decodeURIComponent(new URL(request.url ?? "/", "http://localhost").pathname);
          } catch {
            response.statusCode = 400;
            response.end("Bad Request\n");
            return;
          }
          const runtimeConfigRequest = requestPath === "/runtime-config.json";
          const candidate = resolve(root, "." + requestPath);
          if (!runtimeConfigRequest && candidate !== root && !candidate.startsWith(root + sep)) {
            response.statusCode = 403;
            response.end("Forbidden\n");
            return;
          }
          let file = candidate;
          if (runtimeConfigRequest) {
            try {
              const info = await stat(runtimeConfigPath);
              if (!info.isFile()) throw new Error("runtime config is not a file");
              file = runtimeConfigPath;
            } catch {
              response.statusCode = 503;
              response.setHeader("Cache-Control", "no-store");
              response.end("Runtime configuration is not mounted\n");
              return;
            }
          } else {
            try {
              const info = await stat(file);
              if (!info.isFile()) throw new Error("not a file");
            } catch {
              file = join(root, "index.html");
            }
          }
          const body = await readFile(file);
          response.statusCode = 200;
          response.setHeader("Content-Type", contentTypes[extname(file)] ?? "application/octet-stream");
          ${headers}
          if (runtimeConfigRequest) response.setHeader("Cache-Control", "no-store");
          response.end(body);
        };
        createServer((request, response) => {
          send(request, response).catch(() => {
            response.statusCode = 500;
            response.end("Internal Server Error\n");
          });
        }).listen(port, host, () => console.log("${name} listening on " + host + ":" + port));
      '';
    in
    {
      inherit server;
      runtime = nodejs;
    };

  bridgeUi = stdenvNoCC.mkDerivation (finalAttrs: {
    pname = "ethereum-bridge-ui";
    version = "0.1.0";
    src = "${source}/bridge-ui";
    pnpmDeps = pnpm_10.fetchDeps {
      pname = finalAttrs.pname;
      version = finalAttrs.version;
      src = finalAttrs.src;
      pnpm = pnpm_10;
      fetcherVersion = 2;
      hash = "sha256-IXV4MA+oWfB62Is9crjHmzLkKd3fEMR1eO/X813sjK8=";
    };
    nativeBuildInputs = [
      nodejs
      pnpm_10
      pnpm_10.configHook
    ];
    postUnpack = ''
      mkdir -p "$sourceRoot/../mina-snap/packages"
      cp -r "${source}/mina-snap/packages/provider" "$sourceRoot/../mina-snap/packages/"
      chmod -R u+w "$sourceRoot/../mina-snap"
    '';
    buildPhase = ''
      runHook preBuild
      export COREPACK_HOME="$TMPDIR/corepack"
      export COREPACK_ENABLE_PROJECT_SPEC=0
      export COREPACK_DEFAULT_TO_LATEST=0
      export VITE_MINA_SNAP_ID="npm:@mondejka/mina-snap"
      mkdir -p "$COREPACK_HOME/v1/pnpm/${pnpm_10.version}"
      cp -r "${pnpm_10}/libexec/pnpm/." "$COREPACK_HOME/v1/pnpm/${pnpm_10.version}/"
      chmod -R u+w "$COREPACK_HOME"
      printf '{"locator":{"name":"pnpm","reference":"%s"},"bin":{"pnpm":"./bin/pnpm.cjs","pnpx":"./bin/pnpx.cjs"},"hash":"sha512-nixpkgs"}\n' "${pnpm_10.version}" > "$COREPACK_HOME/v1/pnpm/${pnpm_10.version}/.corepack"
      printf '{"pnpm":"%s"}\n' "${pnpm_10.version}" > "$COREPACK_HOME/lastKnownGood.json"
      corepack pnpm build
      runHook postBuild
    '';
    installPhase = ''
      runHook preInstall
      mkdir -p $out/dist
      cp -r dist/. $out/dist/
      rm -f $out/dist/runtime-config.json
      runHook postInstall
    '';
  });

  explorerUi = stdenvNoCC.mkDerivation (finalAttrs: {
    pname = "ethereum-explorer-ui";
    version = "0.1.0";
    src = "${source}/explorer-ui";
    pnpmDeps = pnpm_10.fetchDeps {
      pname = finalAttrs.pname;
      version = finalAttrs.version;
      src = finalAttrs.src;
      pnpm = pnpm_10;
      fetcherVersion = 2;
      hash = "sha256-gynRJ9jiOfW1y/l/USPuhYEe4461isi80HZU/wQDjvk=";
    };
    nativeBuildInputs = [
      nodejs
      pnpm_10
      pnpm_10.configHook
    ];
    buildPhase = ''
      runHook preBuild
      pnpm run build
      runHook postBuild
    '';
    installPhase = ''
      runHook preInstall
      mkdir -p $out/dist
      cp -r dist/. $out/dist/
      rm -f $out/dist/runtime-config.json
      runHook postInstall
    '';
  });

  bridgeServer = staticServer {
    name = "zeko-bridge-ui";
    root = "${bridgeUi}/dist";
    runtimeConfigEnv = "BRIDGE_UI_RUNTIME_CONFIG_FILE";
    headers = ''
      response.setHeader("Cross-Origin-Opener-Policy", "same-origin");
      response.setHeader("Cross-Origin-Embedder-Policy", "require-corp");
      response.setHeader("Referrer-Policy", "strict-origin-when-cross-origin");
    '';
  };
  explorerServer = staticServer {
    name = "zeko-explorer-ui";
    root = "${explorerUi}/dist";
    runtimeConfigEnv = "EXPLORER_UI_RUNTIME_CONFIG_FILE";
    headers = ''
      response.setHeader("Referrer-Policy", "strict-origin-when-cross-origin");
    '';
  };
  mkUiImage =
    {
      name,
      app,
      server,
      port,
    }:
    dockerTools.streamLayeredImage {
      inherit name;
      tag = "latest";
      contents = [
        baseEnv
        nodejs
        app
      ];
      config = {
        Entrypoint = [
          "${nodejs}/bin/node"
          "${server.server}"
        ];
        Env = baseImageEnv ++ [
          "HOST=0.0.0.0"
          "PORT=${toString port}"
        ];
        ExposedPorts = {
          "${toString port}/tcp" = { };
        };
        User = "65532:65532";
        WorkingDir = "/";
      };
    };

in
assert noRepoLocalRuntimeData;
bootstrap
// {
  zeko-gateway-image = dockerTools.streamLayeredImage {
    name = "docker.io/zekolabs/zeko-gateway";
    tag = "latest";
    contents = [
      baseEnv
      gatewayPackage
    ];
    fakeRootCommands = ''
      mkdir -p ./var/lib/zeko-proof-api
      chown 65532:65532 ./var/lib/zeko-proof-api
      chmod 0750 ./var/lib/zeko-proof-api
      mkdir -p .${artifactsDir}
      chmod 0755 .${artifactsDir}
    '';
    config = {
      Entrypoint = [ "${gatewayPackage}/bin/zeko-proof-api" ];
      # The operator mounts the per-rollup public artifacts directory read-only
      # at ZEKO_ARTIFACTS_DIR; the gateway refuses to start without it and
      # asserts the guest program vkeys against the contracts. The bind mount
      # keeps the host directory's mode, and this process runs as 65532:65532,
      # so the host directory must be 0755 with 0644 files (nix/README.md step 5).
      Env = baseImageEnv ++ [
        "API_BIND=0.0.0.0:8080"
        "HOME=/var/lib/zeko-proof-api"
        "ZEKO_ARTIFACTS_DIR=${artifactsDir}"
      ];
      ExposedPorts = {
        "8080/tcp" = { };
      };
      User = "65532:65532";
      WorkingDir = "/var/lib/zeko-proof-api";
      Volumes = {
        "/var/lib/zeko-proof-api" = { };
        "${artifactsDir}" = { };
      };
    };
  };

  zeko-bridge-ui-image = mkUiImage {
    name = "docker.io/zekolabs/zeko-bridge-ui";
    app = bridgeUi;
    server = bridgeServer;
    port = 4174;
  };
  zeko-explorer-ui-image = mkUiImage {
    name = "docker.io/zekolabs/zeko-explorer-ui";
    app = explorerUi;
    server = explorerServer;
    port = 4175;
  };
}
