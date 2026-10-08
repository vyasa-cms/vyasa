#!/bin/sh
# Installs Vyasa from the latest GitHub release (or VYASA_VERSION) into an
# install directory and puts the `vyasa` command on your PATH.
#
#   curl -fsSL https://vyasa.site/install.sh | sh
#
#   VYASA_VERSION=0.2.0  pins a version        (default: the latest release)
#   VYASA_HOME=~/vyasa   where it is installed  (default: $HOME/vyasa)
#   VYASA_BIN=~/.local/bin  where `vyasa` is linked (default: /usr/local/bin
#                        when writable, else $HOME/.local/bin)
#
# The install directory is also the server's working directory: it serves
# the admin from admin/dist beside the binary, keeps media/ and index/
# there, and `vyasa update apply` swaps the binary and bundle in place.
# The checksum published with every archive is verified before anything
# is unpacked. Needs PostgreSQL 15+ to run; see docs/DEPLOYMENT.md.
set -eu

REPO="vyasa-cms/vyasa"
VYASA_HOME="${VYASA_HOME:-$HOME/vyasa}"

say()  { printf '%s\n' "$*"; }
fail() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || fail "needs $1"; }
need curl; need tar

os=$(uname -s); arch=$(uname -m)
case "$os-$arch" in
    Linux-x86_64)            target=x86_64-unknown-linux-gnu ;;
    Linux-aarch64|Linux-arm64) target=aarch64-unknown-linux-gnu ;;
    Darwin-arm64)            target=aarch64-apple-darwin ;;
    Darwin-x86_64)           fail "no build for Intel macOS yet; run the container image instead (see the README)" ;;
    *)                       fail "no build for $os $arch; see https://github.com/$REPO/releases" ;;
esac
if [ "$os" = Linux ]; then
    glibc=$(ldd --version 2>/dev/null | sed -n '1s/.* \([0-9][0-9.]*\)$/\1/p')
    case "$glibc" in
        2.[0-9]|2.[0-9].*|2.[12][0-9]|2.[12][0-9].*|2.3[0-4]|2.3[0-4].*)
            fail "glibc $glibc is too old; the Linux build needs 2.35 or newer (Debian 12, Ubuntu 22.04, or later)" ;;
    esac
fi

auth=""
[ -n "${GITHUB_TOKEN:-}" ] && auth="Authorization: Bearer $GITHUB_TOKEN"
fetch() { # url out
    if [ -n "$auth" ]; then curl -fsSL -H "$auth" -H "Accept: application/octet-stream" -o "$2" "$1"
    else curl -fsSL -o "$2" "$1"; fi
}

version="${VYASA_VERSION:-}"
if [ -z "$version" ]; then
    api="https://api.github.com/repos/$REPO/releases/latest"
    version=$( { [ -n "$auth" ] && curl -fsSL -H "$auth" "$api" || curl -fsSL "$api"; } \
        | sed -n 's/.*"tag_name": *"v\{0,1\}\([^"]*\)".*/\1/p' | head -n 1)
    [ -n "$version" ] || fail "could not find the latest release; set VYASA_VERSION"
fi
name="vyasa-$version-$target"
if [ -n "$auth" ]; then
    # Private repository: resolve asset ids through the API.
    assets=$(curl -fsSL -H "$auth" "https://api.github.com/repos/$REPO/releases/tags/v$version")
    # Pick the assets by name (python is only needed for this private-repo path).
    url=$(printf '%s' "$assets" | python3 -c "import json,sys;d=json.load(sys.stdin);print(next(a['url'] for a in d['assets'] if a['name']=='$name.tar.gz'))" 2>/dev/null || true)
    sum_url=$(printf '%s' "$assets" | python3 -c "import json,sys;d=json.load(sys.stdin);print(next(a['url'] for a in d['assets'] if a['name']=='$name.tar.gz.sha256'))" 2>/dev/null || true)
    [ -n "$url" ] || fail "release v$version has no archive for $target"
else
    base="https://github.com/$REPO/releases/download/v$version"
    url="$base/$name.tar.gz"; sum_url="$base/$name.tar.gz.sha256"
fi

tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
say "Downloading Vyasa $version for $target"
fetch "$url" "$tmp/$name.tar.gz" || fail "download failed: $url"
fetch "$sum_url" "$tmp/$name.tar.gz.sha256" || fail "checksum download failed"
expected=$(awk '{print $1}' "$tmp/$name.tar.gz.sha256")
if command -v sha256sum >/dev/null 2>&1; then actual=$(sha256sum "$tmp/$name.tar.gz" | awk '{print $1}')
else actual=$(shasum -a 256 "$tmp/$name.tar.gz" | awk '{print $1}'); fi
[ "$expected" = "$actual" ] || fail "checksum mismatch for $name.tar.gz (expected $expected, got $actual)"
say "Checksum verified"

mkdir -p "$VYASA_HOME"
tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
if [ -x "$VYASA_HOME/vyasa" ]; then
    mv -f "$VYASA_HOME/vyasa" "$VYASA_HOME/vyasa.previous"
fi
cp "$tmp/$name/vyasa" "$VYASA_HOME/vyasa"
rm -rf "$VYASA_HOME/admin/dist.new"; mkdir -p "$VYASA_HOME/admin"
cp -R "$tmp/$name/admin/dist" "$VYASA_HOME/admin/dist.new"
rm -rf "$VYASA_HOME/admin/dist"; mv "$VYASA_HOME/admin/dist.new" "$VYASA_HOME/admin/dist"
mkdir -p "$VYASA_HOME/docs"; cp "$tmp/$name/docs/DEPLOYMENT.md" "$VYASA_HOME/docs/" 2>/dev/null || true
cp "$tmp/$name/CHANGELOG.md" "$VYASA_HOME/" 2>/dev/null || true

if [ -z "${VYASA_BIN:-}" ]; then
    if [ -w /usr/local/bin ]; then VYASA_BIN=/usr/local/bin; else VYASA_BIN="$HOME/.local/bin"; fi
fi
mkdir -p "$VYASA_BIN"
ln -sf "$VYASA_HOME/vyasa" "$VYASA_BIN/vyasa"

say ""
say "Installed Vyasa $version to $VYASA_HOME"
say "  command: $VYASA_BIN/vyasa"
case ":$PATH:" in *":$VYASA_BIN:"*) ;; *) say "  add $VYASA_BIN to your PATH to call it as \`vyasa\`" ;; esac
say ""
say "Next, with a PostgreSQL database ready:"
say "  cd $VYASA_HOME"
say "  export VYASA_DATABASE_URL=postgres://vyasa:PASSWORD@localhost:5432/vyasa"
say "  vyasa migrate && vyasa serve      # then open http://localhost:3000/admin/setup"
say ""
say "Run the server from $VYASA_HOME: it serves the admin from admin/dist there"
say "and keeps media/ and index/ beside it. Upgrades: \`vyasa update check\`."
