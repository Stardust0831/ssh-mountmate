#!/bin/sh
set -eu

cargo_about="${1:-cargo-about}"
output="${2:-licenses/RUST-THIRD-PARTY.txt}"
temporary="${output}.tmp.$$"
trap 'rm -f "${temporary}"' EXIT HUP INT TERM

"${cargo_about}" generate --workspace --all-features --locked --offline --fail \
  --output-file "${temporary}" distribution/licenses.hbs
perl -0pi -e 's/\r\n/\n/g; s/[ \t]+\n/\n/g; s/\n+\z/\n/' "${temporary}"
mv "${temporary}" "${output}"
trap - EXIT HUP INT TERM
