# Installation

DBCrust offers multiple installation methods to fit your workflow. Choose the one that works best for you.

## 📦 Package Manager Installation

### uv (Recommended)

[uv](https://github.com/astral-sh/uv) is the fastest Python package manager and our recommended installation method:

=== "Global Tool Installation"

    ```bash
    # Install as a global tool (recommended)
    uv tool install dbcrust
    
    # Verify installation
    dbcrust --version
    
    # Update to latest version
    uv tool upgrade dbcrust
    ```

=== "Run Without Installing"

    ```bash
    # Try DBCrust immediately without installation
    uvx dbcrust postgresql://user:pass@localhost/mydb
    
    # Works with any database URL
    uvx dbcrust mysql://user:pass@localhost/mydb
    uvx dbcrust sqlite:///path/to/database.db
    ```

=== "Project Dependency"

    ```bash
    # Add to a Python project
    uv add dbcrust
    
    # In pyproject.toml
    [project]
    dependencies = ["dbcrust>=0.4.0"]
    ```

### pip

If you prefer pip, DBCrust is available on PyPI:

```bash
# Install globally
pip install dbcrust

# Install for current user only
pip install --user dbcrust

# Install in virtual environment
python -m venv venv
source venv/bin/activate  # On Windows: venv\Scripts\activate
pip install dbcrust

# Upgrade to latest version
pip install --upgrade dbcrust
```

### pipx

For isolated installations:

```bash
# Install with pipx
pipx install dbcrust

# Upgrade
pipx upgrade dbcrust

# Uninstall
pipx uninstall dbcrust
```

## 🐧 System Package Managers

### Homebrew (macOS/Linux)

```bash
# Install from Homebrew (coming soon)
brew install dbcrust
```

### Conda

```bash
# Install from conda-forge (coming soon)
conda install -c conda-forge dbcrust
```

## 🦀 Build from Source

### Prerequisites

- **Rust**: Install from [rustup.rs](https://rustup.rs/)
- **Python 3.10+**: For Python bindings (optional)

```bash
# Install Rust if you haven't already
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
```

### Clone and Build

```bash
# Clone the repository
git clone https://github.com/ctourriere/pgcrust.git
cd pgcrust

# Build release version
cargo build --release

# Install to ~/.cargo/bin
cargo install --path .

# Verify installation
dbcrust --version
```

### Development Build

For contributing or testing latest features:

```bash
# Clone and build
git clone https://github.com/ctourriere/pgcrust.git
cd pgcrust

# Development build (faster compilation)
cargo build

# Run directly
cargo run -- postgresql://user:pass@localhost/mydb

# Run tests
cargo test
```

### Python Bindings

To build with Python integration:

```bash
# Install maturin for Python bindings
pip install maturin

# Build Python wheel
maturin build --release

# Install the wheel
pip install target/wheels/dbcrust-*.whl
```

## 🔧 Post-Installation Setup

### Shell Completions

Enable tab completion for your shell:

=== "Bash"

    ```bash
    # Add to ~/.bashrc
    eval "$(dbcrust --generate-completion bash)"
    ```

=== "Zsh"

    ```bash
    # Add to ~/.zshrc
    eval "$(dbcrust --generate-completion zsh)"
    ```

=== "Fish"

    ```bash
    # Add to ~/.config/fish/config.fish
    dbcrust --generate-completion fish | source
    ```

### Configuration Directory

DBCrust creates its configuration directory automatically:

```bash
# Configuration location
~/.config/dbcrust/
├── config.toml          # Main configuration
├── history.txt          # Command history
└── sessions/            # Saved sessions
```

### Environment Variables

Optional environment variables for convenience:

```bash
# Default database URL
export DBCRUST_DATABASE_URL="postgresql://user:pass@localhost/mydb"

# Default SSH tunnel
export DBCRUST_SSH_TUNNEL="jumphost.example.com"

# Vault configuration
export VAULT_ADDR="https://vault.company.com"
export VAULT_TOKEN="your-token"
```

## 🐳 Docker

Run DBCrust in a container:

```bash
# Pull the image (coming soon)
docker pull ghcr.io/ctourriere/dbcrust:latest

# Run with database connection
docker run -it --rm \
  -e DATABASE_URL="postgresql://user:pass@host.docker.internal/db" \
  ghcr.io/ctourriere/dbcrust:latest

# Mount config directory
docker run -it --rm \
  -v ~/.config/dbcrust:/root/.config/dbcrust \
  ghcr.io/ctourriere/dbcrust:latest
```

## ✅ Verify Installation

Test your installation with these commands:

```bash
# Check version
dbcrust --version

# Show help
dbcrust --help

# Test with SQLite (no external database needed)
dbcrust sqlite://:memory: --query "SELECT 'Hello DBCrust!' as message"
```

Expected output:
```
╭─────────────────╮
│ message         │
├─────────────────┤
│ Hello DBCrust!  │
╰─────────────────╯
```

## 🔄 Updates

### Automatic Updates

DBCrust will notify you when newer versions are available:

```
📦 DBCrust v0.5.0 is available! (currently using v0.4.0)
   Run 'uv tool upgrade dbcrust' to update
```

### Manual Updates

=== "uv"

    ```bash
    uv tool upgrade dbcrust
    ```

=== "pip"

    ```bash
    pip install --upgrade dbcrust
    ```

=== "From Source"

    ```bash
    cd pgcrust
    git pull origin main
    cargo install --path .
    ```

## 🗑️ Uninstallation

=== "uv"

    ```bash
    uv tool uninstall dbcrust
    ```

=== "pip"

    ```bash
    pip uninstall dbcrust
    ```

=== "From Source"

    ```bash
    cargo uninstall dbcrust
    ```

Remove configuration (optional):

```bash
rm -rf ~/.config/dbcrust
```

## 🆘 Troubleshooting

### Common Issues

!!! error "Command not found: dbcrust"

    **Solution**: Make sure the installation directory is in your PATH:
    
    ```bash
    # For uv tool installs
    export PATH="$HOME/.local/bin:$PATH"
    
    # For pip --user installs
    export PATH="$HOME/.local/bin:$PATH"
    
    # For cargo installs
    export PATH="$HOME/.cargo/bin:$PATH"
    ```

!!! error "SSL certificate verify failed"

    **Solution**: Update certificates or use system packages:
    
    ```bash
    # macOS
    /Applications/Python\ 3.x/Install\ Certificates.command
    
    # Linux
    sudo apt-get update && sudo apt-get install ca-certificates
    ```

!!! error "Permission denied"

    **Solution**: Use virtual environments or user installations:
    
    ```bash
    # Use --user flag
    pip install --user dbcrust
    
    # Or use virtual environment
    python -m venv venv
    source venv/bin/activate
    pip install dbcrust
    ```

### Platform-Specific Notes

=== "macOS"

    - **M1/M2 Macs**: All installation methods work natively
    - **Homebrew**: Recommended for system-wide installation
    - **Security**: You may need to allow the binary in System Preferences

=== "Linux"

    - **Distribution packages**: Coming soon for major distros
    - **AppImage**: Portable version coming soon
    - **Dependencies**: Most distros include all required libraries

=== "Windows"

    - **WSL2**: Recommended for best experience
    - **Native**: Supported but may require Visual C++ redistributables
    - **PowerShell**: Full support for modern terminals

### Getting Help

If you encounter issues:

1. Check the troubleshooting section below
2. Search [existing issues](https://github.com/ctourriere/pgcrust/issues)
3. Create a [new issue](https://github.com/ctourriere/pgcrust/issues/new) with:
   - Operating system and version
   - Installation method used
   - Full error message
   - Output of `dbcrust --version`

---

<div align="center">
    <strong>Installation complete? Let's get started!</strong><br>
    <a href="quick-start.md" class="md-button md-button--primary">Quick Start Guide</a>
    <a href="user-guide/basic-usage.md" class="md-button">User Guide</a>
</div>