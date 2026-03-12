#!/bin/bash
# Build Rivals Swapper for Linux
# Outputs to: dist/RivalsSwapper/
#
# Prerequisites:
#   pip install pyinstaller
#   # For AppImage (optional):
#   sudo apt install fuse libfuse2

set -e

echo "Building Rivals Swapper..."
python3 -m PyInstaller build.spec --noconfirm --clean

echo ""
echo "Done! Output in: dist/RivalsSwapper/"
echo "Copy your tools/ folder into dist/RivalsSwapper/ before distributing."
echo ""

# --- Optional: Create AppImage ---
if command -v appimagetool &> /dev/null; then
    echo "Creating AppImage..."
    
    APPDIR="dist/RivalsSwapper.AppDir"
    rm -rf "$APPDIR"
    mkdir -p "$APPDIR/usr/bin"
    mkdir -p "$APPDIR/usr/share/icons/hicolor/256x256/apps"
    
    # Copy built files
    cp -r dist/RivalsSwapper/* "$APPDIR/usr/bin/"
    
    # Icon
    cp assets/RivalsIcon_NoOutline.png "$APPDIR/usr/share/icons/hicolor/256x256/apps/rivalsswapper.png"
    cp assets/RivalsIcon_NoOutline.png "$APPDIR/rivalsswapper.png"
    
    # Desktop entry
    cat > "$APPDIR/rivalsswapper.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Rivals Swapper
Exec=RivalsSwapper
Icon=rivalsswapper
Categories=Game;Utility;
EOF
    
    # AppRun
    cat > "$APPDIR/AppRun" <<'EOF'
#!/bin/bash
SELF=$(readlink -f "$0")
HERE=${SELF%/*}
exec "$HERE/usr/bin/RivalsSwapper" "$@"
EOF
    chmod +x "$APPDIR/AppRun"
    
    appimagetool "$APPDIR" "dist/RivalsSwapper.AppImage"
    echo "AppImage created: dist/RivalsSwapper.AppImage"
else
    echo "appimagetool not found — skipping AppImage creation."
    echo "To create an AppImage, install appimagetool:"
    echo "  wget https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-x86_64.AppImage"
    echo "  chmod +x appimagetool-x86_64.AppImage && sudo mv appimagetool-x86_64.AppImage /usr/local/bin/appimagetool"
fi
