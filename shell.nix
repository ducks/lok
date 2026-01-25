{ pkgs ? import <nixpkgs> {} }:

let
  # Use fenix for latest stable Rust
  fenix = import (fetchTarball "https://github.com/nix-community/fenix/archive/main.tar.gz") { inherit pkgs; };
  rustToolchain = fenix.stable.toolchain;
in
pkgs.mkShell {
  name = "lok-dev";

  buildInputs = [
    rustToolchain

    # For reqwest/openssl
    pkgs.openssl
    pkgs.pkg-config

    # For LLM CLI backends
    pkgs.nodejs_22
    pkgs.nodePackages.npm
    pkgs.ollama
  ];

  shellHook = ''
    export RUST_BACKTRACE=1
    export NPM_CONFIG_PREFIX=$HOME/.npm-global
    export PATH=$HOME/.local/bin:$NPM_CONFIG_PREFIX/bin:$PATH
    export LD_LIBRARY_PATH=${pkgs.openssl.out}/lib:$LD_LIBRARY_PATH

    echo ""
    echo "Lok Development Environment"
    echo "==========================="
    echo "Rust: $(rustc --version)"
    echo "Cargo: $(cargo --version)"
    echo ""
    echo "Commands:"
    echo "  cargo build          - Build the project"
    echo "  cargo run -- ask     - Run with ask command"
    echo "  cargo test           - Run tests"
    echo "  cargo clippy         - Lint"
    echo ""
  '';
}
