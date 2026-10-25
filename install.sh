#!/usr/bin/env bash
set -euo pipefail

main() {
  ROSTER_HOME="${ROSTER_MACHINE_HOME:-$HOME}/.roster"
  INSTALL_DIR="${ROSTER_INSTALLATION_ROOT:-$ROSTER_HOME/installation}"
  BIN_DIR="$HOME/.local/bin"
  ROSTER_BIN_DIR="$ROSTER_HOME/bin"
  REPO_URL="https://github.com/R4yHarris/github-agent-roster.git"

  echo "=== github-agent-roster installer (Linux / WSL 2 / macOS) ==="

  OS_NAME="$(uname -s)"
  if [ "$OS_NAME" = "Linux" ] && command -v apt-get >/dev/null 2>&1; then
    NEED_APT=0
    command -v pkg-config >/dev/null 2>&1 || NEED_APT=1
    command -v cc >/dev/null 2>&1 || NEED_APT=1
    command -v git >/dev/null 2>&1 || NEED_APT=1
    command -v curl >/dev/null 2>&1 || NEED_APT=1
    if [ ! -f /usr/include/openssl/ssl.h ] && [ ! -f /usr/include/x86_64-linux-gnu/openssl/opensslconf.h ] && [ ! -f /usr/include/aarch64-linux-gnu/openssl/opensslconf.h ]; then
      NEED_APT=1
    fi
    if [ "$NEED_APT" -eq 1 ]; then
      echo "-> Installing Linux build prerequisites (build-essential, pkg-config, libssl-dev, git, curl)..."
      run_root() {
        if [ "$(id -u)" -eq 0 ]; then
          "$@" </dev/null
        elif command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then
          sudo "$@" </dev/null
        elif [ -x /mnt/c/Windows/System32/wsl.exe ]; then
          /mnt/c/Windows/System32/wsl.exe -d "${WSL_DISTRO_NAME:-Ubuntu}" -u root -- "$@" </dev/null
        elif command -v sudo >/dev/null 2>&1 && [ -r /dev/tty ]; then
          sudo "$@" </dev/tty
        else
          sudo "$@"
        fi
      }
      if ! run_root apt-get update -qq 2>/dev/null; then
        if [ -f /etc/apt/sources.list.d/ubuntu.sources ]; then
          run_root apt-get update -o Dir::Etc::sourcelist="sources.list.d/ubuntu.sources" -o Dir::Etc::sourceparts="-" -o APT::Get::List-Cleanup="0" -qq
        fi
      fi
      run_root env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq build-essential pkg-config libssl-dev git curl ca-certificates
    fi
  fi

  if [ -f "$HOME/.cargo/env" ]; then
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
  fi
  if ! command -v cargo >/dev/null 2>&1; then
    echo "-> Installing Rust toolchain via rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y </dev/null
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
  fi

  export NVM_DIR="${NVM_DIR:-$HOME/.nvm}"
  if [ -s "$NVM_DIR/nvm.sh" ]; then
    # shellcheck disable=SC1091
    . "$NVM_DIR/nvm.sh"
  elif ! (command -v node >/dev/null 2>&1 && node --version 2>/dev/null | grep -q '^v20\.'); then
    echo "-> Installing nvm for Node 20 consumer checks and agent-pr.mjs..."
    curl -fsSL https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh | bash </dev/null
    if [ -s "$NVM_DIR/nvm.sh" ]; then
      # shellcheck disable=SC1091
      . "$NVM_DIR/nvm.sh"
    fi
  fi
  if command -v nvm >/dev/null 2>&1; then
    if ! ls "$NVM_DIR/versions/node" 2>/dev/null | grep -q '^v20\.'; then
      echo "-> Installing Node 20 via nvm for consumer JS checks and agent-pr.mjs..."
      nvm install 20 >/dev/null </dev/null
    fi
  fi

  mkdir -p "$ROSTER_HOME" "$ROSTER_HOME/vault" "$ROSTER_BIN_DIR" "$BIN_DIR"
  chmod 700 "$ROSTER_HOME" "$ROSTER_HOME/vault"

  SCRIPT_SOURCE_DIR="${ROSTER_INSTALL_SOURCE:-}"
  if [ -z "$SCRIPT_SOURCE_DIR" ] && [ -n "${BASH_SOURCE[0]:-}" ] && [ -f "${BASH_SOURCE[0]}" ]; then
    CANDIDATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    if [ -f "$CANDIDATE_DIR/Cargo.toml" ] && [ -d "$CANDIDATE_DIR/rust" ]; then
      SCRIPT_SOURCE_DIR="$CANDIDATE_DIR"
    fi
  fi

  if [ -n "$SCRIPT_SOURCE_DIR" ] && [ "$SCRIPT_SOURCE_DIR" = "$INSTALL_DIR" ]; then
    echo "-> Using existing installation at $INSTALL_DIR..."
    git -c core.autocrlf=false -C "$INSTALL_DIR" submodule update --init --recursive </dev/null
  elif [ -d "$INSTALL_DIR/.git" ]; then
    echo "-> Updating installation at $INSTALL_DIR..."
    git -c core.autocrlf=false -C "$INSTALL_DIR" fetch --prune origin </dev/null
    git -c core.autocrlf=false -C "$INSTALL_DIR" checkout -B main origin/main </dev/null
    git -c core.autocrlf=false -C "$INSTALL_DIR" submodule update --init --recursive </dev/null
  elif [ -n "$SCRIPT_SOURCE_DIR" ]; then
    echo "-> Cloning local source checkout $SCRIPT_SOURCE_DIR into $INSTALL_DIR..."
    git -c core.autocrlf=false clone --branch main --recurse-submodules "$SCRIPT_SOURCE_DIR" "$INSTALL_DIR" </dev/null
    git -C "$INSTALL_DIR" remote set-url origin "$REPO_URL" || true
  else
    echo "-> Cloning $REPO_URL into $INSTALL_DIR..."
    git -c core.autocrlf=false clone --branch main --recurse-submodules "$REPO_URL" "$INSTALL_DIR" </dev/null
  fi
  git -C "$INSTALL_DIR" config core.autocrlf false
  if [ -d "$INSTALL_DIR/vendor/github-agent-contracts" ]; then
    git -C "$INSTALL_DIR/vendor/github-agent-contracts" config core.autocrlf false
    git -C "$INSTALL_DIR/vendor/github-agent-contracts" rm --cached -r . >/dev/null 2>&1 || true
    git -C "$INSTALL_DIR/vendor/github-agent-contracts" reset --hard HEAD >/dev/null 2>&1 || true
  fi

  if [ -n "${ROSTER_PREBUILT_BIN:-}" ] && [ -f "$ROSTER_PREBUILT_BIN" ]; then
    echo "-> Installing prebuilt binary from $ROSTER_PREBUILT_BIN..."
    cp "$ROSTER_PREBUILT_BIN" "$ROSTER_BIN_DIR/roster"
    cp "$ROSTER_PREBUILT_BIN" "$BIN_DIR/roster"
  else
    echo "-> Building native Rust release binary (roster)..."
    (cd "$INSTALL_DIR" && cargo build --release --locked </dev/null)
    cp "$INSTALL_DIR/target/release/roster" "$ROSTER_BIN_DIR/roster"
    cp "$INSTALL_DIR/target/release/roster" "$BIN_DIR/roster"
  fi
  chmod 755 "$ROSTER_BIN_DIR/roster" "$BIN_DIR/roster"

  SEED_SOURCE_DIR="${ROSTER_SEED_DIR:-}"
  if [ -z "$SEED_SOURCE_DIR" ] && [ -n "$SCRIPT_SOURCE_DIR" ] && [ -d "$SCRIPT_SOURCE_DIR/.roster" ]; then
    SEED_SOURCE_DIR="$SCRIPT_SOURCE_DIR/.roster"
  fi

  if [ ! -f "$ROSTER_HOME/config.yml" ]; then
    if [ -n "$SEED_SOURCE_DIR" ] && [ -f "$SEED_SOURCE_DIR/config.yml" ]; then
      cp "$SEED_SOURCE_DIR/config.yml" "$ROSTER_HOME/config.yml"
    else
      cp "$INSTALL_DIR/roster.config.example.yml" "$ROSTER_HOME/config.yml"
    fi
  fi
  chmod 600 "$ROSTER_HOME/config.yml"

  if [ ! -f "$ROSTER_HOME/fleet.yml" ]; then
    if [ -n "$SEED_SOURCE_DIR" ] && [ -f "$SEED_SOURCE_DIR/fleet.yml" ]; then
      cp "$SEED_SOURCE_DIR/fleet.yml" "$ROSTER_HOME/fleet.yml"
    else
      printf "profiles: []\n" > "$ROSTER_HOME/fleet.yml"
    fi
  fi
  chmod 600 "$ROSTER_HOME/fleet.yml"

  if [ ! -f "$ROSTER_HOME/capabilities.yml" ]; then
    if [ -n "$SEED_SOURCE_DIR" ] && [ -f "$SEED_SOURCE_DIR/capabilities.yml" ]; then
      cp "$SEED_SOURCE_DIR/capabilities.yml" "$ROSTER_HOME/capabilities.yml"
    else
      printf "capabilities: []\n" > "$ROSTER_HOME/capabilities.yml"
    fi
  fi
  chmod 600 "$ROSTER_HOME/capabilities.yml"

  GIT_REAL="$(readlink -f "$(command -v git)")"
  CARGO_REAL="$(readlink -f "$(command -v cargo)")"
  NODE20_REAL=""
  if [ -d "$NVM_DIR/versions/node" ]; then
    V20_DIR="$(ls "$NVM_DIR/versions/node" 2>/dev/null | grep '^v20\.' | tail -n 1 || true)"
    if [ -n "$V20_DIR" ] && [ -x "$NVM_DIR/versions/node/$V20_DIR/bin/node" ]; then
      NODE20_REAL="$(readlink -f "$NVM_DIR/versions/node/$V20_DIR/bin/node")"
    fi
  fi
  if [ -z "$NODE20_REAL" ] && command -v node >/dev/null 2>&1; then
    if node --version 2>/dev/null | grep -q '^v20\.'; then
      NODE20_REAL="$(readlink -f "$(command -v node)")"
    fi
  fi

  cat > "$ROSTER_HOME/env.sh" <<EOF
export PATH="$BIN_DIR:$ROSTER_BIN_DIR:\$HOME/.cargo/bin:\$PATH"
export ROSTER_MACHINE_HOME="${ROSTER_MACHINE_HOME:-$HOME}"
export ROSTER_INSTALLATION_ROOT="$INSTALL_DIR"
export ROSTER_GIT="$GIT_REAL"
export ROSTER_CARGO="$CARGO_REAL"
EOF
  if [ -n "$NODE20_REAL" ]; then
    printf 'export ROSTER_NODE20="%s"\n' "$NODE20_REAL" >> "$ROSTER_HOME/env.sh"
  fi
  chmod 600 "$ROSTER_HOME/env.sh"

  SOURCE_LINE='[ -f "$HOME/.roster/env.sh" ] && . "$HOME/.roster/env.sh"'
  for rc in "$HOME/.bashrc" "$HOME/.profile" "$HOME/.zshrc"; do
    if [ -f "$rc" ]; then
      if ! grep -Fq '.roster/env.sh' "$rc"; then
        printf '\n%s\n' "$SOURCE_LINE" >> "$rc"
      fi
    fi
  done

  # shellcheck disable=SC1090
  . "$ROSTER_HOME/env.sh"

  echo ""
  echo "=== Machine-scoped configuration initialized in $ROSTER_HOME ==="
  echo "  Binary:        $BIN_DIR/roster"
  echo "  Installation:  $INSTALL_DIR"
  echo "  Config:        $ROSTER_HOME/config.yml"
  echo "  Fleet:         $ROSTER_HOME/fleet.yml"
  echo "  Capabilities:  $ROSTER_HOME/capabilities.yml"
  echo "  Vault:         $ROSTER_HOME/vault"
  echo "  Environment:   $ROSTER_HOME/env.sh"
  echo ""
  "$BIN_DIR/roster" doctor || true
  echo ""
  "$BIN_DIR/roster" fleet list || true
  echo ""
  echo "Next: run 'source ~/.roster/env.sh' (or open a new shell) and run 'roster onboard --discover' to configure your machine fleet."
}

main "$@"
