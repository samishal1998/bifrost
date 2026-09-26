#!/bin/sh
# Bifröst installer: curl -fsSL https://samishal1998.github.io/bifrost/install.sh | sh
#
#   BIFROST_VERSION       release tag to install, e.g. v0.1.0 (default: latest)
#   BIFROST_INSTALL_DIR   where bifrost, bifrostd and bifrost-tui go (default: $HOME/.local/bin)
#   BIFROST_DOWNLOAD_URL  base URL override for mirrors/tests; the script fetches <base>/<asset>
#
# Never uses sudo and installs nothing but the three binaries. Everything runs inside main(), which is
# called on the last line, so a truncated download runs nothing.
set -eu

main() {
	REPO=samishal1998/bifrost
	DOCS_QUICKSTART=https://samishal1998.github.io/bifrost/quickstart/

	if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
		B=$(printf '\033[1m') T=$(printf '\033[36m') Y=$(printf '\033[33m') R=$(printf '\033[31m') N=$(printf '\033[0m')
	else
		B='' T='' Y='' R='' N=''
	fi
	say() { printf '%s\n' "$*"; }
	warn() { printf '%swarning:%s %s\n' "$Y" "$N" "$*" >&2; }
	die() {
		printf '%serror:%s %s\n' "$R" "$N" "$*" >&2
		exit 1
	}
	have() { command -v "$1" >/dev/null 2>&1; }
	fetch() { # url dest
		if have curl; then
			curl -fsSL -o "$2" "$1" || die "download failed: $1"
		elif have wget; then
			wget -qO "$2" "$1" || die "download failed: $1"
		else
			die "need curl or wget to download"
		fi
	}

	# --- target (release contract: bifrost-<target>.tar.gz) ---
	case $(uname -s) in
	Linux) os=unknown-linux-musl ;;
	Darwin) os=apple-darwin ;;
	*) die "unsupported OS: $(uname -s) (Bifröst supports Linux and macOS)" ;;
	esac
	case $(uname -m) in
	x86_64 | amd64) arch=x86_64 ;;
	aarch64 | arm64) arch=aarch64 ;;
	*) die "unsupported CPU: $(uname -m) (Bifröst supports x86_64 and aarch64)" ;;
	esac
	target=$arch-$os
	asset=bifrost-$target.tar.gz

	# --- where from ---
	version=${BIFROST_VERSION:-latest}
	case $version in
	'' | *[!A-Za-z0-9._-]*) die "invalid BIFROST_VERSION '$version' (want a tag like v0.1.0)" ;;
	[0-9]*) version=v$version ;;
	esac
	if [ -n "${BIFROST_DOWNLOAD_URL:-}" ]; then
		base=${BIFROST_DOWNLOAD_URL%/}
	elif [ "$version" = latest ]; then
		base=https://github.com/$REPO/releases/latest/download
	else
		base=https://github.com/$REPO/releases/download/$version
	fi

	# --- where to (checked before any download) ---
	dir=${BIFROST_INSTALL_DIR:-$HOME/.local/bin}
	mkdir -p "$dir" 2>/dev/null || die "cannot create $dir; set BIFROST_INSTALL_DIR to a directory you can write (no sudo needed)"
	[ -w "$dir" ] || die "$dir is not writable; set BIFROST_INSTALL_DIR to a directory you can write (no sudo needed)"

	# --- download and verify ---
	tmp=$(mktemp -d "${TMPDIR:-/tmp}/bifrost.XXXXXX") || die "mktemp failed"
	trap 'rm -rf "$tmp" "$dir/.bifrost.new" "$dir/.bifrostd.new" "$dir/.bifrost-tui.new"' EXIT
	trap 'exit 1' HUP INT TERM

	say "${B}Bifröst${N} — installing $asset"
	say "  from $base"
	fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS"
	fetch "$base/$asset" "$tmp/$asset"

	if have sha256sum; then
		sum=$(sha256sum "$tmp/$asset")
	elif have shasum; then
		sum=$(shasum -a 256 "$tmp/$asset")
	else
		die "need sha256sum or shasum to verify the download; refusing to install unverified binaries"
	fi
	actual=${sum%% *}
	expected=$(awk -v a="$asset" '$2 == a || $2 == "*" a { print $1; exit }' "$tmp/SHA256SUMS")
	[ -n "$expected" ] || die "SHA256SUMS has no entry for $asset"
	[ "$actual" = "$expected" ] || die "checksum mismatch for $asset (expected $expected, got $actual); nothing was installed"
	say "  ${T}✓${N} sha256 verified"

	# --- install: stage all three, then rename (atomic, works while bifrostd is running) ---
	tar -xzf "$tmp/$asset" -C "$tmp" || die "could not extract $asset"
	src=$tmp/bifrost-$target
	for b in bifrost bifrostd bifrost-tui; do
		[ -f "$src/$b" ] || die "$asset has no bifrost-$target/$b"
		cp "$src/$b" "$dir/.$b.new" || die "cannot write $dir/.$b.new"
		chmod 755 "$dir/.$b.new"
	done
	for b in bifrost bifrostd bifrost-tui; do
		mv -f "$dir/.$b.new" "$dir/$b" || die "cannot install $dir/$b"
	done
	ver=$("$dir/bifrost" --version) || die "installed $dir/bifrost does not run on this machine"
	say "  ${T}✓${N} installed $ver: bifrost, bifrostd, bifrost-tui → $dir"

	case ":$PATH:" in
	*":$dir:"*) ;;
	*) warn "$dir is not on your PATH. Add this line to your shell profile (~/.profile, ~/.bashrc or ~/.zshrc):
    export PATH=\"$dir:\$PATH\"" ;;
	esac

	# --- runtime dependencies: report only, never install ---
	say ""
	say "${B}Runtime dependencies${N} (bifrost doctor re-checks these)"
	ok() { say "  ${T}✓${N} $1"; }
	miss() { say "  ${Y}✗${N} $1 — $2"; }
	if [ "$(uname -s)" = Linux ]; then
		if have apt-get; then pm='sudo apt install' p_ssh=openssh-client p_sshfs=sshfs
		elif have dnf; then pm='sudo dnf install' p_ssh=openssh-clients p_sshfs=fuse-sshfs
		elif have pacman; then pm='sudo pacman -S' p_ssh=openssh p_sshfs=sshfs
		else pm='install with your package manager:' p_ssh=openssh p_sshfs=sshfs
		fi
		if have ssh; then ok ssh; else miss ssh "$pm $p_ssh"; fi
		if have sshfs; then ok sshfs; else miss sshfs "$pm $p_sshfs"; fi
		if have fusermount3; then ok fusermount3; elif have fusermount; then ok fusermount; else miss fusermount3 "$pm fuse3"; fi
		if [ -e /dev/fuse ]; then ok /dev/fuse; else miss /dev/fuse "sudo modprobe fuse (in a container: --device /dev/fuse)"; fi
		if have rclone; then ok rclone; else miss "rclone (optional driver)" "$pm rclone"; fi
	else
		if have ssh; then ok ssh; else miss ssh "ships with macOS; check your PATH"; fi
		if [ -e /Library/Filesystems/macfuse.fs ]; then ok macFUSE
		elif [ -e "/Library/Application Support/fuse-t" ] || [ -e /usr/local/lib/libfuse-t.dylib ]; then ok FUSE-T
		else miss "macFUSE or FUSE-T (for sshfs/rclone mounts)" "brew install --cask macfuse  |  brew install macos-fuse-t/cask/fuse-t"
		fi
		if have sshfs; then ok sshfs; else miss sshfs "brew install gromgit/fuse/sshfs-mac  |  brew install macos-fuse-t/cask/fuse-t-sshfs"; fi
		if have rclone; then ok rclone; else miss "rclone (rclone-nfs needs no FUSE)" "brew install rclone"; fi
	fi

	say ""
	say "${B}Next steps${N}"
	say "  1. Write ~/.config/bifrost/config.toml — quickstart: $DOCS_QUICKSTART"
	say "  2. Start the daemon:   bifrostd"
	say "  3. Check on it:        bifrost status"
	say ""
	say "Remote worlds. Local files."
}

main "$@"
