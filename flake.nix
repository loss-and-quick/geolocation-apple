{
  description = "geolocation-apple — retrieve geographic coordinates for BSSIDs via Apple's location service";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable-small";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = {
    self,
    nixpkgs,
    rust-overlay,
  }: let
    systems = ["x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin"];

    # Packages and overlays are built from the consumer's own nixpkgs instance
    # (no overlay application here), so `inputs.nixpkgs.follows = "nixpkgs"`
    # keeps them on the same nixpkgs and substitutable from its cache.
    forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

    # The dev shell needs rust-overlay on top of nixpkgs, which means its own
    # nixpkgs instance; packages deliberately avoid this one.
    forAllSystemsRustOverlay = f:
      nixpkgs.lib.genAttrs systems (system:
        f (import nixpkgs {
          inherit system;
          overlays = [rust-overlay.overlays.default];
        }));

    version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
  in {
    packages = forAllSystems (pkgs: let
      geolocation-apple = pkgs.rustPlatform.buildRustPackage {
        pname = "geolocation-apple";
        inherit version;

        src = self;

        cargoLock.lockFile = ./Cargo.lock;

        # reqwest pulls in rustls with aws-lc-rs, and nixpkgs' cargo-setup-hook
        # exports AWS_LC_SYS_USE_SYSTEM=1: aws-lc-sys then refuses to compile its
        # vendored C sources and panics unless pkg-config can find a system
        # AWS-LC. Link nixpkgs' aws-lc instead of building the vendored copy.
        nativeBuildInputs = [pkgs.pkg-config];
        buildInputs = [pkgs.aws-lc];
        env.AWS_LC_SYS_USE_SYSTEM = true;

        # tests/live_apple.rs queries Apple's live endpoint, which the build
        # sandbox has no network access to.
        doCheck = false;

        meta = {
          description = "Resolve coordinates for Wi-Fi BSSIDs via Apple's Wi-Fi Positioning Service";
          homepage = "https://github.com/loss-and-quick/geolocation-apple";
          mainProgram = "geolocation-apple";
          platforms = pkgs.lib.platforms.linux ++ pkgs.lib.platforms.darwin;
        };
      };
    in {
      default = geolocation-apple;
      inherit geolocation-apple;
    });

    overlays.default = final: _prev: {
      geolocation-apple = self.packages.${final.stdenv.hostPlatform.system}.default;
    };

    devShells = forAllSystemsRustOverlay (pkgs: {
      default = pkgs.mkShell {
        nativeBuildInputs = [
          (pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml)
          pkgs.pkg-config
        ];
        buildInputs = [pkgs.aws-lc pkgs.openssl];
        # Match the package build: link the system AWS-LC rather than compiling
        # the vendored one on every `cargo build`.
        env.AWS_LC_SYS_USE_SYSTEM = "1";
      };
    });

    formatter = forAllSystems (pkgs: pkgs.alejandra);
  };
}
