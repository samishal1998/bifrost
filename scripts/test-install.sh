#!/usr/bin/env bash
# End-to-end test of install.sh against a fake release served on 127.0.0.1 (Linux).
# Usage: scripts/test-install.sh [BIN_DIR]   BIN_DIR holds bifrost, bifrostd, bifrost-tui for this host
#        (default: ${CARGO_TARGET_DIR:-target}/debug)
# Happy path under dash and bash, from the file and piped; then a corrupted tarball, a missing SHA256SUMS,
# an unwritable install dir and a hostile BIFROST_VERSION must all fail with nothing installed.
set -euo pipefail
cd "$(dirname "$0")/.."
bin=${1:-${CARGO_TARGET_DIR:-target}/debug}
target=$(uname -m)-unknown-linux-musl
asset=bifrost-$target.tar.gz
work=$(mktemp -d)
srv_pid=
trap '[ -n "$srv_pid" ] && kill "$srv_pid"; chmod -R u+w "$work"; rm -rf "$work"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }

sh -n install.sh && dash -n install.sh && bash -n install.sh
if command -v shellcheck >/dev/null; then shellcheck install.sh; fi

# Fake release with the contract layout: good/ (tarball + SHA256SUMS), bad/ (truncated tarball, good sums),
# nosums/ (tarball only).
pkg=$work/pkg/bifrost-$target
mkdir -p "$pkg" "$work/srv/good" "$work/srv/bad" "$work/srv/nosums"
cp "$bin/bifrost" "$bin/bifrostd" "$bin/bifrost-tui" "$pkg/"
strip "$pkg"/* 2>/dev/null || true # smaller archive; debug info is irrelevant here
for f in README.md LICENSE-MIT LICENSE-APACHE; do cp "$f" "$pkg/" 2>/dev/null || echo placeholder >"$pkg/$f"; done
tar -czf "$work/srv/good/$asset" -C "$work/pkg" "bifrost-$target"
(cd "$work/srv/good" && sha256sum "$asset" >SHA256SUMS)
head -c 100000 "$work/srv/good/$asset" >"$work/srv/bad/$asset"
cp "$work/srv/good/SHA256SUMS" "$work/srv/bad/"
ln -s "../good/$asset" "$work/srv/nosums/$asset"

port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
python3 -m http.server --bind 127.0.0.1 --directory "$work/srv" "$port" >/dev/null 2>&1 &
srv_pid=$!
url=http://127.0.0.1:$port
for _ in $(seq 50); do curl -fs "$url/good/SHA256SUMS" >/dev/null && break; sleep 0.1; done

for sh in dash bash; do
	for how in file pipe; do
		dir=$work/$sh-$how/bin
		if [ "$how" = file ]; then
			BIFROST_DOWNLOAD_URL=$url/good BIFROST_INSTALL_DIR=$dir "$sh" install.sh >"$work/out" 2>&1 ||
				{ cat "$work/out"; fail "$sh $how install"; }
		else
			cat install.sh | BIFROST_DOWNLOAD_URL=$url/good BIFROST_INSTALL_DIR=$dir "$sh" >"$work/out" 2>&1 ||
				{ cat "$work/out"; fail "$sh $how install"; }
		fi
		for b in bifrost bifrostd bifrost-tui; do [ -x "$dir/$b" ] || fail "$sh $how: $b not executable"; done
		[ "$(stat -c %a "$dir/bifrost")" = 755 ] || fail "$sh $how: mode not 755"
		"$dir/bifrost" --version | grep -q '^bifrost ' || fail "$sh $how: bifrost --version"
		"$dir/bifrostd" --version | grep -q '^bifrostd ' || fail "$sh $how: bifrostd --version"
		"$dir/bifrost-tui" --help >/dev/null || fail "$sh $how: bifrost-tui --help"
		grep -q 'sha256 verified' "$work/out" || fail "$sh $how: no verification line"
		grep -q 'not on your PATH' "$work/out" || fail "$sh $how: no PATH warning"
		! grep -q $'\033' "$work/out" || fail "$sh $how: colour escapes without a TTY"
		for f in "$dir"/.*.new; do [ ! -e "$f" ] || fail "$sh $how: staged $f left behind"; done
		echo "ok: $sh $how → $("$dir/bifrost" --version)"
	done
done

# expect_fail NAME PATTERN DIR [ENV=VAL...]: install.sh exits non-zero, says PATTERN, installs nothing into DIR.
expect_fail() {
	local name=$1 pattern=$2 dir=$3
	shift 3
	if env "$@" BIFROST_INSTALL_DIR="$dir" dash install.sh >"$work/out" 2>&1; then
		cat "$work/out"
		fail "$name: exited 0"
	fi
	grep -q "$pattern" "$work/out" || { cat "$work/out"; fail "$name: no '$pattern' in output"; }
	[ ! -e "$dir/bifrost" ] || fail "$name: something was installed"
	echo "ok: $name → $(grep error: "$work/out")"
}
expect_fail "corrupted tarball" "checksum mismatch" "$work/bad/bin" BIFROST_DOWNLOAD_URL="$url/bad"
expect_fail "missing SHA256SUMS" "download failed: .*/SHA256SUMS" "$work/nosums/bin" BIFROST_DOWNLOAD_URL="$url/nosums"
expect_fail "hostile BIFROST_VERSION" "invalid BIFROST_VERSION" "$work/ver/bin" 'BIFROST_VERSION=v1;id'
if [ "$(id -u)" != 0 ]; then # root can write anywhere
	mkdir -p "$work/ro" && chmod 555 "$work/ro"
	expect_fail "unwritable install dir" "set BIFROST_INSTALL_DIR" "$work/ro" BIFROST_DOWNLOAD_URL="$url/good"
fi
echo "install.sh: all tests passed"
