#!/bin/sh
# Install Lanes (`lane` and `laned`) from GitHub Releases on macOS, Linux and WSL.
#
#   curl -fsSL https://github.com/Autter-dev/lanes/releases/latest/download/lanes-installer.sh | sh
#
# Options, as flags (`... | sh -s -- --dir ~/bin`) or environment variables:
#   --version X   LANES_VERSION       install version X instead of the one this script came with
#   --dir DIR     LANES_INSTALL_DIR   where to put the binaries (default: ~/.local/bin)
#   --no-setup    LANES_NO_SETUP=1    don't offer to set Lanes up afterwards
#
# It downloads one archive and its checksum from github.com, checks the checksum and copies two
# files. It sends nothing anywhere else and doesn't touch your shell profile. At a terminal, it
# then asks whether to set Lanes up now (`lane setup`); in CI or a pipe it never asks. Run it
# again to update. To uninstall, delete `lane` and `laned` from the folder it printed.

set -eu

REPO="Autter-dev/lanes"
# The release workflow stamps the release's version in here.
DEFAULT_VERSION="__LANES_VERSION__"

say() { printf 'lanes: %s\n' "$1"; }
fail() {
	printf 'lanes: %s\n' "$1" >&2
	exit 1
}

usage() {
	cat <<EOF
Install Lanes from GitHub Releases.

  curl -fsSL https://github.com/$REPO/releases/latest/download/lanes-installer.sh | sh -s -- [options]

Options:
  --version X   install version X (default: $DEFAULT_VERSION)
  --dir DIR     install into DIR (default: ~/.local/bin)
  --no-setup    don't offer to set Lanes up afterwards
  -h, --help    show this
EOF
}

detect_target() {
	os="$(uname -s)"
	arch="$(uname -m)"
	case "$os" in
	Darwin)
		os=apple-darwin
		# A shell running under Rosetta says x86_64 on Apple silicon; use the native build.
		if [ "$arch" = x86_64 ] && [ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" = 1 ]; then
			arch=arm64
		fi
		;;
	Linux) os=unknown-linux-musl ;;
	MINGW* | MSYS* | CYGWIN*)
		fail "on Windows, run this in PowerShell instead: irm https://github.com/$REPO/releases/latest/download/lanes-installer.ps1 | iex"
		;;
	*) fail "Lanes has no build for $os yet. See https://lanes.autter.dev/download for the systems it runs on." ;;
	esac
	case "$arch" in
	x86_64 | amd64) arch=x86_64 ;;
	arm64 | aarch64) arch=aarch64 ;;
	*) fail "Lanes has no build for $arch processors yet, only x86_64 and arm64. See https://lanes.autter.dev/download." ;;
	esac
	printf '%s-%s\n' "$arch" "$os"
}

download() {
	if command -v curl >/dev/null 2>&1; then
		curl -fsSL --proto '=https' --tlsv1.2 -o "$2" "$1"
	elif command -v wget >/dev/null 2>&1; then
		wget -q --https-only -O "$2" "$1"
	else
		fail "this installer needs curl or wget. Install one of them, then run it again."
	fi
}

verify() {
	expected="$(cut -d ' ' -f 1 <"$2")"
	if command -v sha256sum >/dev/null 2>&1; then
		actual="$(sha256sum "$1" | cut -d ' ' -f 1)"
	elif command -v shasum >/dev/null 2>&1; then
		actual="$(shasum -a 256 "$1" | cut -d ' ' -f 1)"
	else
		fail "this installer needs sha256sum or shasum to check the download. Install one, then run it again."
	fi
	[ "$expected" = "$actual" ] ||
		fail "the download doesn't match its checksum, so nothing was installed. Run the installer again; if it keeps happening, open an issue at https://github.com/$REPO/issues."
}

main() {
	version="${LANES_VERSION:-}"
	dir="${LANES_INSTALL_DIR:-${HOME:?set HOME, or pass --dir}/.local/bin}"
	while [ $# -gt 0 ]; do
		case "$1" in
		--version)
			[ $# -ge 2 ] || fail "--version needs a value, like --version 0.1.0."
			version="$2"
			shift 2
			;;
		--dir)
			[ $# -ge 2 ] || fail "--dir needs a folder, like --dir ~/bin."
			dir="$2"
			shift 2
			;;
		--no-setup)
			LANES_NO_SETUP=1
			shift
			;;
		-h | --help)
			usage
			exit 0
			;;
		*) fail "unknown option $1. Run it with --help to see the options." ;;
		esac
	done
	version="${version#v}"
	if [ -z "$version" ]; then
		case "$DEFAULT_VERSION" in
		__*) fail "this copy of the installer has no version in it. Use the one from a release (https://github.com/$REPO/releases/latest/download/lanes-installer.sh), or pass --version 0.1.0." ;;
		*) version="$DEFAULT_VERSION" ;;
		esac
	fi

	target="$(detect_target)"
	name="lanes-$version-$target"
	url="https://github.com/$REPO/releases/download/v$version/$name.tar.gz"

	tmp="$(mktemp -d 2>/dev/null || mktemp -d -t lanes)"
	trap 'rm -rf "$tmp"' EXIT
	trap 'exit 130' INT TERM

	say "downloading Lanes $version for $target"
	download "$url" "$tmp/$name.tar.gz" ||
		fail "couldn't download $url. Check your connection, and that version $version is listed at https://github.com/$REPO/releases."
	download "$url.sha256" "$tmp/$name.tar.gz.sha256" ||
		fail "couldn't download the checksum for $name.tar.gz. Run the installer again in a minute."
	verify "$tmp/$name.tar.gz" "$tmp/$name.tar.gz.sha256"
	tar -xzf "$tmp/$name.tar.gz" -C "$tmp" ||
		fail "couldn't unpack $name.tar.gz. Run the installer again; if it keeps happening, open an issue at https://github.com/$REPO/issues."

	mkdir -p "$dir" || fail "couldn't create $dir. Pick another folder with --dir."
	for bin in lane laned; do
		[ -f "$tmp/$name/$bin" ] || fail "the archive has no $bin in it. Please open an issue at https://github.com/$REPO/issues."
		# Write a temporary file and rename it, so a running copy is never left half-written.
		{ cp "$tmp/$name/$bin" "$dir/.$bin.new" && chmod 755 "$dir/.$bin.new" && mv -f "$dir/.$bin.new" "$dir/$bin"; } ||
			fail "couldn't write $dir/$bin. Check that folder's permissions, or pick another with --dir."
	done

	say "installed lane and laned $version in $dir"
	case ":$PATH:" in
	*":$dir:"*) ;;
	*)
		say "$dir isn't on your PATH yet. Add this line to your shell profile (~/.zshrc or ~/.bashrc), then open a new terminal:"
		printf '\n  export PATH="%s:$PATH"\n\n' "$dir"
		;;
	esac
	offer_setup "$dir"
}

# At a terminal, offer to set Lanes up straight away: check this machine and put its agents in
# lanes (`lane setup`). Never in CI, without a terminal, or with LANES_NO_SETUP / --no-setup.
offer_setup() {
	if [ -n "${LANES_NO_SETUP:-}" ] || [ -n "${CI:-}" ] || [ ! -t 1 ] || ! (: </dev/tty) 2>/dev/null; then
		say "next: run \`lane setup\` to check this machine and put your agents in lanes"
		return 0
	fi
	printf 'lanes: set Lanes up now? It checks this machine and puts your agents in lanes. [Y/n] ' >/dev/tty
	answer=""
	read -r answer </dev/tty || answer="n"
	case "$answer" in
	"" | y | Y | yes | Yes | YES)
		"$1/lane" setup </dev/tty >/dev/tty 2>&1 || say "setup didn't finish; run \`lane setup\` any time"
		;;
	*) say "later: run \`lane setup\` to check this machine and put your agents in lanes" ;;
	esac
}

main "$@"
