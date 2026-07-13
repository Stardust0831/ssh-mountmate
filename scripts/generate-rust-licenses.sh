#!/bin/sh
set -eu

cargo_about="${1:-cargo-about}"
output="${2:-licenses/RUST-THIRD-PARTY.txt}"
mode="${3:-offline}"
temporary="${output}.tmp.$$"
trap 'rm -f "${temporary}"' EXIT HUP INT TERM

set -- generate --workspace --all-features --locked --fail \
  --output-file "${temporary}" distribution/licenses.hbs
case "${mode}" in
  offline) set -- "$@" --offline ;;
  online) ;;
  *) echo "usage: $0 [cargo-about] [output] [offline|online]" >&2; exit 2 ;;
esac
"${cargo_about}" "$@"
perl -0pi -e 's/\r\n/\n/g; s/[ \t]+\n/\n/g; s/\n+\z/\n/' "${temporary}"
mv "${temporary}" "${output}"
trap - EXIT HUP INT TERM
