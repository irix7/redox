#!/usr/bin/env bash
# Fetch, patch, build and stage the userland programs of the Indy root.
#
#   tools/indy-userland.sh              every package
#   tools/indy-userland.sh ion kibi     only these
#   tools/indy-userland.sh --export     write each checkout's changes back to
#                                       its indy-mips64.patch
#
# Each package is an upstream checkout in recipes/<group>/<name>/source at a
# pinned revision, with recipes/<group>/<name>/indy-mips64.patch on top (lock
# file pins for the patched libc and redox_syscall, and source changes). The
# programs are copied to build/indy/userland/usr/bin, which `make indy-root`
# adds to the root tree.
#
# Every program is its own static executable. A program is read whole over
# the network before it starts and nothing is shared between processes, so a
# multicall binary would cost its full size on every command.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
STAGE="${INDY_USERLAND:-$ROOT/build/indy/userland}"
cd "$ROOT"

# name | recipe directory | git URL | revision | cargo arguments | programs
PACKAGES='
ion|recipes/core/ion|https://gitlab.redox-os.org/redox-os/ion.git|e9b3ca48b7ab6444498d5c3aa902d471ce29f759|--bin ion|ion
coreutils|recipes/core/coreutils|https://gitlab.redox-os.org/redox-os/coreutils.git|575a46d3b624ad9cd222c9e3250073e9269c3462||chown clear df free ps reset shutdown time uptime which
userutils|recipes/core/userutils|https://gitlab.redox-os.org/redox-os/userutils.git|641a5aa81574db79d883151bc3438d3e59f90b88||id login passwd su
netutils|recipes/core/netutils|https://gitlab.redox-os.org/redox-os/netutils.git|4900bdc333e291abc61ac8790908a295ec92cd0b||dns ifconfig nc ping
findutils|recipes/core/findutils|https://gitlab.redox-os.org/redox-os/findutils.git|116c044399318bf1c77a0d97371b61999ee170b4|--bin find|find
extrautils|recipes/core/extrautils|https://gitlab.redox-os.org/redox-os/extrautils.git|f5e8626a89484549c8ac38525a534d831e64ff31|--no-default-features|calc dmesg gunzip gzip less resize screenfetch tar unzip
kibi|recipes/text/kibi|https://github.com/ilai-deutel/kibi|v0.3.3||kibi
grep|recipes/tools/uutils-grep|https://github.com/uutils/grep|6754749fa5db1ea7f2df2df2f200567a2094f83c||grep
sed|recipes/tools/uutils-sed|https://github.com/uutils/sed|ce791636917b4d4d9d79353a69d2e743ab8c5d92||sed
doom|recipes/games/doom-rs|https://github.com/bejiihiu/doom-rs|9f5b61294b14cfb2fed3fddcd61cf55e3cdf7b85|-p doom-indy|doom
uutils|recipes/core/uutils|https://github.com/uutils/coreutils|0.11.0|UUTILS|UUTILS
'
# uutils builds one package per program (uu_<name>).
UUTILS="base64 basename cat chmod cksum comm cp cut date dd dirname du echo env expand false fmt fold head join kill link ln ls md5sum mkdir mktemp more mv nice nl nproc numfmt od paste printenv printf pwd readlink realpath rm rmdir seq sha1sum sha256sum shuf sleep sort split stat sum tac tail tee test touch tr true truncate tsort uname unexpand uniq unlink wc yes"

fetch() { # recipe-dir url rev
  local dir="$1" url="$2" rev="$3"
  [ -d "$dir/source" ] && return 0
  git clone -q "$url" "$dir/source"
  git -C "$dir/source" checkout -q "$rev"
  # Upstream's own Redox patches come first and are committed locally, so
  # that `git diff` in the checkout is this port's patch alone.
  for patch in $(sed -n '/^patches = \[/,/^\]/s/^ *"\(.*\)",\{0,1\}$/\1/p' "$dir/recipe.toml"); do
    git -C "$dir/source" apply "../$patch"
  done
  if [ -n "$(git -C "$dir/source" status --porcelain)" ]; then
    git -C "$dir/source" -c user.name=recipe -c user.email=recipe@localhost commit -qam "recipe patches"
  fi
  if [ -f "$dir/indy-mips64.patch" ]; then
    git -C "$dir/source" apply ../indy-mips64.patch
  fi
}

want() { [ ${#ONLY[@]} -eq 0 ] || printf '%s\n' "${ONLY[@]}" | grep -qx "$1"; }

EXPORT=0
ONLY=()
for arg in "$@"; do
  if [ "$arg" = --export ]; then EXPORT=1; else ONLY+=("$arg"); fi
done

mkdir -p "$STAGE/usr/bin"
while IFS='|' read -r name dir url rev cargo bins; do
  [ -n "$name" ] || continue
  want "$name" || continue
  if [ "$EXPORT" = 1 ]; then
    git -C "$dir/source" add -N . >/dev/null
    git -C "$dir/source" diff HEAD > "$dir/indy-mips64.patch"
    [ -s "$dir/indy-mips64.patch" ] || rm "$dir/indy-mips64.patch"
    continue
  fi
  fetch "$dir" "$url" "$rev"
  out="$dir/source/target/mips64-sgi-redox/release"
  if [ "$name" = uutils ]; then
    bins="$UUTILS"
    for bin in $bins; do
      echo "indy-userland: uutils $bin"
      tools/userland-indy.sh "$dir/source" -p "uu_$bin" 2>&1 | grep -E '^(error|warning: unused)' -A12 || true
      [ -x "$out/$bin" ] || { echo "indy-userland: $bin did not build" >&2; exit 1; }
    done
  else
    echo "indy-userland: $name"
    # shellcheck disable=SC2086
    tools/userland-indy.sh "$dir/source" $cargo 2>&1 | grep -E '^error' -A12 || true
  fi
  for bin in $bins; do
    install -m 0755 "$out/$bin" "$STAGE/usr/bin/$bin"
  done
done <<<"$PACKAGES"
[ "$EXPORT" = 1 ] && exit 0

# Names that are links to another program.
ln -sf id "$STAGE/usr/bin/whoami"
ln -sf test "$STAGE/usr/bin/["
# kibi's syntax files.
if [ -d recipes/text/kibi/source/syntax.d ]; then
  mkdir -p "$STAGE/usr/share/kibi"
  cp -r recipes/text/kibi/source/syntax.d "$STAGE/usr/share/kibi/"
fi
du -sb "$STAGE"
