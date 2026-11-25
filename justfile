# Name of the application's binary.
name := 'cosmic-ext-mail'
# The unique ID of the application.
appid := 'dev.edfloreshz.Mail'

# Path to root file system, which defaults to `/`.
rootdir := ''
# The prefix for the `/usr` directory.
prefix := '/usr'
# The location of the cargo target directory.
cargo-target-dir := env('CARGO_TARGET_DIR', 'target')

# Application's appstream metadata
appdata := appid + '.metainfo.xml'
# Application's desktop entry
desktop := appid + '.desktop'
# Application's icon.
icon-svg := appid + '.svg'

# Install destinations
base-dir := absolute_path(clean(rootdir / prefix))
appdata-dst := base-dir / 'share' / 'appdata' / appdata
bin-dst := base-dir / 'bin' / name
desktop-dst := base-dir / 'share' / 'applications' / desktop
icons-dst := base-dir / 'share' / 'icons' / 'hicolor'
icon-svg-dst := icons-dst / 'scalable' / 'apps' / icon-svg

# Default recipe which runs `just build-release`
default: build-release

# Runs `cargo clean`
clean:
    cargo clean

# Removes vendored dependencies
clean-vendor:
    rm -rf .cargo vendor vendor.tar

# `cargo clean` and removes vendored dependencies
clean-dist: clean clean-vendor

# Compiles with debug profile
build-debug *args:
    cargo build --locked {{args}}

# Compiles with release profile
build-release *args: (build-debug '--release' args)

# Compiles release profile with vendored dependencies
build-vendored *args: vendor-extract (build-release '--frozen --offline' args)

# Runs a clippy check
check *args:
    cargo clippy --all-features --locked {{args}} -- -W clippy::pedantic

# Runs a clippy check with JSON message format
check-json: (check '--message-format=json')

# Run the application for testing purposes
run *args:
    env RUST_BACKTRACE=full cargo run --release --locked {{args}}

# Installs files
install:
    install -Dm0755 {{ cargo-target-dir / 'release' / name }} {{bin-dst}}
    install -Dm0644 {{ 'target' / 'xdgen' / 'app.desktop' }} {{desktop-dst}}
    install -Dm0644 {{ 'target' / 'xdgen' / 'app.metainfo.xml' }} {{appdata-dst}}
    install -Dm0644 {{ 'resources' / 'icons' / 'hicolor' / 'scalable' / 'apps' / 'icon.svg' }} {{icon-svg-dst}}

# Uninstalls installed files
uninstall:
    rm {{bin-dst}} {{desktop-dst}} {{icon-svg-dst}}

# Vendor dependencies locally
vendor:
    mkdir -p .cargo
    cargo vendor | head -n -1 > .cargo/config.toml
    echo 'directory = "vendor"' >> .cargo/config.toml
    tar pcf vendor.tar vendor
    rm -rf vendor

# Extracts vendored dependencies
vendor-extract:
    rm -rf vendor
    tar pxf vendor.tar

# Bump the Cargo package version, commit it, and create an annotated tag.
tag version:
    #!/usr/bin/env bash
    set -euo pipefail
    version={{quote(version)}}

    if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$ ]]; then
        echo "Expected a Cargo version such as 1.2.3." >&2
        exit 1
    fi
    if git show-ref --verify --quiet "refs/tags/$version"; then
        echo "Tag $version already exists." >&2
        exit 1
    fi
    if [[ -n "$(git status --porcelain -- Cargo.toml Cargo.lock)" ]]; then
        echo "Commit or stash pending Cargo.toml and Cargo.lock changes before tagging." >&2
        exit 1
    fi

    sed -i "0,/^version = /s/^version = .*/version = \"$version\"/" Cargo.toml
    if ! cargo update --workspace; then
        git restore --worktree -- Cargo.toml Cargo.lock
        exit 1
    fi
    git commit --only -m "release: $version" -- Cargo.toml Cargo.lock
    git tag -a "$version" -m "Release $version"

# Regenerates the Flatpak dependency manifest from Cargo.lock.
flatpak-cargo-sources:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p .flatpak-builder
    scratch=$(mktemp -d .flatpak-builder/cargo-sources.XXXXXX)
    trap 'rm -rf "$scratch"' EXIT
    curl -fsSL https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py -o "$scratch/generator.py"
    python3 -m venv "$scratch/.venv"
    "$scratch/.venv/bin/python" -m pip install 'aiohttp>=3.9.5,<4' 'tomlkit>=0.13.3,<1'
    "$scratch/.venv/bin/python" "$scratch/generator.py" Cargo.lock -o "$scratch/cargo-sources.json"
    mv "$scratch/cargo-sources.json" flatpak/cargo-sources.json

# Builds an installable Flatpak bundle under dist.
flatpak-bundle *args: flatpak-cargo-sources
    mkdir -p dist
    flatpak-builder --user --force-clean --repo=.flatpak-builder/repo {{args}} .flatpak-builder/build-dir flatpak/{{appid}}.json
    flatpak build-bundle .flatpak-builder/repo dist/{{appid}}.flatpak {{appid}}

# Build the AppImage, zsync metadata, and checksums under dist.
appimage:
    #!/usr/bin/env bash
    set -euo pipefail
    export ARCH=x86_64
    APPIMAGE=Mail-x86_64.AppImage
    repository="${GITHUB_REPOSITORY:-cosmic-utils/cosmic-ext-mail}"

    if [[ "$(uname -sm)" != "Linux x86_64" ]]; then
        echo "AppImage packaging requires Linux x86_64." >&2
        exit 1
    fi
    for dependency in curl desktop-file-validate zsyncmake; do
        if ! command -v "$dependency" >/dev/null; then
            echo "Missing packaging dependency: $dependency" >&2
            exit 1
        fi
    done

    output="$PWD/dist"
    mkdir -p "$output"
    tools=$(mktemp -d "$output/.build.XXXXXX")
    trap 'rm -rf -- "$tools"' EXIT
    curl --fail --location --retry 3 \
      https://github.com/linuxdeploy/linuxdeploy/releases/download/1-alpha-20250213-2/linuxdeploy-x86_64.AppImage \
      --output "$tools/linuxdeploy"
    curl --fail --location --retry 3 \
      https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage \
      --output "$tools/appimagetool"
    echo "ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0  $tools/appimagetool" | sha256sum --check
    chmod +x "$tools/linuxdeploy" "$tools/appimagetool"

    cargo build --locked --release --target-dir "{{cargo-target-dir}}"
    install -Dm644 target/xdgen/app.metainfo.xml "$tools/AppDir/usr/share/metainfo/{{appid}}.metainfo.xml"
    install -Dm644 resources/icons/hicolor/scalable/apps/icon.svg "$tools/{{appid}}.svg"
    install -Dm644 target/xdgen/app.desktop "$tools/{{desktop}}"
    desktop-file-validate "$tools/{{desktop}}"
    APPIMAGE_EXTRACT_AND_RUN=1 "$tools/linuxdeploy" --appdir "$tools/AppDir" \
      --executable "{{cargo-target-dir}}/release/{{name}}" \
      --desktop-file "$tools/{{desktop}}" \
      --icon-file "$tools/{{appid}}.svg"

    cd "$tools"
    update_info="gh-releases-zsync|${repository%/*}|${repository#*/}|latest|${APPIMAGE}.zsync"
    ./appimagetool --appimage-extract > /dev/null
    mv squashfs-root appimagetool-root
    OWD="$tools" ./appimagetool-root/AppRun --updateinformation "$update_info" \
      --file-url "$APPIMAGE" AppDir "$APPIMAGE"
    test -s "$APPIMAGE.zsync"
    LC_ALL=C grep -aFxm1 "URL: $APPIMAGE" "$APPIMAGE.zsync"
    test "$(./"$APPIMAGE" --appimage-updateinformation)" = "$update_info"
    "./$APPIMAGE" --appimage-extract > /dev/null
    test -x squashfs-root/AppRun
    test -x squashfs-root/usr/bin/{{name}}
    test -s squashfs-root/{{appid}}.svg
    test -s squashfs-root/usr/share/metainfo/{{appdata}}
    grep -Fx 'Icon={{appid}}' squashfs-root/{{desktop}}
    grep -Fx 'StartupWMClass={{appid}}' squashfs-root/{{desktop}}
    sha256sum "$APPIMAGE" "$APPIMAGE.zsync" > SHA256SUMS
    sha256sum --check SHA256SUMS
    mv -- "$APPIMAGE" "$APPIMAGE.zsync" SHA256SUMS "$output/"
    printf 'AppImage: %s/%s\n' "$output" "$APPIMAGE"
