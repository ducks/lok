{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  name = "lok-dev";

  buildInputs = with pkgs; [
    rustc
    cargo
    rust-analyzer
    clippy
    rustfmt

    # For reqwest/openssl
    openssl
    pkg-config

    # For LLM CLI backends
    nodejs_22
    nodePackages.npm
  ];

  shellHook = ''
    export RUST_BACKTRACE=1
    export NPM_CONFIG_PREFIX=$HOME/.npm-global
    export PATH=$HOME/.local/bin:$NPM_CONFIG_PREFIX/bin:$PATH

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
