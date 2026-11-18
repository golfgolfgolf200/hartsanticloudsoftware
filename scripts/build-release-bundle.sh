#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="$ROOT_DIR/dist-release"
rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR/bin" "$DIST_DIR/docs" "$DIST_DIR/assets/wasm"

cp "$ROOT_DIR"/{README,THIRD_PARTY_PACKAGES,SECURITY,PRIVACY,TERMS,LICENSE}.md "$DIST_DIR/docs/" 2>/dev/null || true

echo "[1/4] Building stripped LTO Rust supervisor..."
cd "$ROOT_DIR"
cargo build --release -p hartsanticloud-supervisor
install -m 0755 target/release/hartsanticloud-supervisor "$DIST_DIR/bin/hartsanticloud-supervisor"

echo "[2/4] Bundling stripped static runtime binaries..."
for bin in llama-server whisper-cli wasmtime firecracker; do
  if [[ -x "/opt/hartsanticloud/bin/$bin" ]]; then
    install -m 0755 "/opt/hartsanticloud/bin/$bin" "$DIST_DIR/bin/$bin"
    strip --strip-unneeded "$DIST_DIR/bin/$bin" 2>/dev/null || true
  fi
done
[[ -f /tmp/hartsanticloud-dev/wasm/agent-shell-task.wasm ]] && \
  install -m 0644 /tmp/hartsanticloud-dev/wasm/agent-shell-task.wasm "$DIST_DIR/assets/wasm/agent-shell-task.wasm"

echo "[3/4] Building Tauri v2 + Svelte 5 desktop packages..."
cd "$ROOT_DIR/control-plane"
npm install --silent
npm run tauri build
find "$ROOT_DIR/target/release/bundle" -type f \( -name "*.deb" -o -name "*.AppImage" \) -exec cp {} "$DIST_DIR/" \;

echo "[4/4] Generating unified single-command office installer (install.sh)..."
cat << 'INSTALL_EOF' > "$DIST_DIR/install.sh"
#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
echo "=== Installing Hart's Anti-Cloud Edge Platform ==="

sudo mkdir -p /opt/hartsanticloud/bin /opt/hartsanticloud/models \
  /var/lib/hartsanticloud/sqlite /var/lib/hartsanticloud/run \
  /var/lib/hartsanticloud/firecracker /var/lib/hartsanticloud/wasm

for bin in hartsanticloud-supervisor llama-server wasmtime firecracker; do
  if [[ -x "$SCRIPT_DIR/bin/$bin" ]]; then
    sudo install -m 0755 "$SCRIPT_DIR/bin/$bin" "/opt/hartsanticloud/bin/$bin"
  fi
done

if [[ -d "$SCRIPT_DIR/assets/wasm" ]]; then
  sudo cp -rn "$SCRIPT_DIR/assets/wasm/." /var/lib/hartsanticloud/wasm/
fi
sudo chown -R "$USER":"$(id -gn)" /opt/hartsanticloud /var/lib/hartsanticloud

# Unified desktop installation: native .deb on Debian/Ubuntu, AppImage fallback elsewhere
DEB_PKG=$(find "$SCRIPT_DIR" -maxdepth 1 -name "*.deb" | head -n 1)
APPIMAGE_PKG=$(find "$SCRIPT_DIR" -maxdepth 1 -name "*.AppImage" | head -n 1)

if command -v apt-get >/dev/null 2>&1 && [[ -n "$DEB_PKG" ]]; then
  echo "Installing native Debian/Ubuntu desktop package..."
  sudo apt-get install -y "$DEB_PKG"
elif [[ -n "$APPIMAGE_PKG" ]]; then
  echo "Installing portable desktop AppImage..."
  install -m 0755 "$APPIMAGE_PKG" /opt/hartsanticloud/bin/hartsanticloud-control-plane.AppImage
  mkdir -p ~/.local/share/applications
  cat << 'DESKTOP_EOF' > ~/.local/share/applications/hartsanticloud.desktop
[Desktop Entry]
Name=Hart's AntiCloud Control Plane
Exec=/opt/hartsanticloud/bin/hartsanticloud-control-plane.AppImage
Type=Application
Categories=Utility;System;
DESKTOP_EOF
fi

# Optional Firecracker KVM kernel/rootfs fetch only when --with-kvm is passed
if [[ "${1:-}" == "--with-kvm" && -e /dev/kvm && ! -f /var/lib/hartsanticloud/firecracker/vmlinux.bin ]]; then
  echo "Provisioning optional Firecracker microVM kernel & rootfs (~425 MB)..."
  curl -fsSL "https://s3.amazonaws.com/spec.ccfc.min/firecracker-ci/v1.10/x86_64/vmlinux-5.10.223" \
    -o /var/lib/hartsanticloud/firecracker/vmlinux.bin
  curl -fsSL "https://s3.amazonaws.com/spec.ccfc.min/firecracker-ci/v1.10/x86_64/ubuntu-22.04.ext4" \
    -o /var/lib/hartsanticloud/firecracker/rootfs.ext4
fi

mkdir -p ~/.config/systemd/user
cat << 'UNIT_EOF' > ~/.config/systemd/user/hartsanticloud-supervisor.service
[Unit]
Description=Hart's Anti-Cloud Hardened Edge Supervisor
After=network-online.target

[Service]
Type=simple
ExecStart=/opt/hartsanticloud/bin/hartsanticloud-supervisor
Restart=always
RestartSec=2

[Install]
WantedBy=default.target
UNIT_EOF

systemctl --user daemon-reload
systemctl --user enable --now hartsanticloud-supervisor.service
echo "Installation complete! Open 'Hart's AntiCloud Control Plane' from your Applications menu."
INSTALL_EOF
chmod +x "$DIST_DIR/install.sh"

cd "$DIST_DIR"
sha256sum bin/* *.deb *.AppImage install.sh 2>/dev/null > SHA256SUMS || true

# Create single compressed archive for one-file customer download
cd "$ROOT_DIR"
tar -cf - -C "$ROOT_DIR" dist-release | zstd -19 -T0 -o "$ROOT_DIR/hartsanticloud-v0.1.0-linux-x86_64.tar.zst" 2>/dev/null || \
  tar -cJf "$ROOT_DIR/hartsanticloud-v0.1.0-linux-x86_64.tar.xz" -C "$ROOT_DIR" dist-release

ls -lh "$DIST_DIR" "$ROOT_DIR"/hartsanticloud-v0.1.0-linux-x86_64.tar.*
