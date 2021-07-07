#!/bin/sh

# Convert manifest.toml into elixir load script 'project.exs'

# output generated to this file in local dir - override with $OUTPUT
OUTPUT=${OUTPUT:-"./project.exs"}
# input manifest file - override with INPUT
INPUT=${INPUT:-"manifest.toml"}
TMP_JSON=$(mktemp)
LOAD_SCRIPT="extra/loadactors.ex"

[ ! -f "$LOAD_SCRIPT" ] \
  && echo "missing $LOAD_SCRIPT" \
  && exit 1
[ ! -f "$INPUT" ] \
  && echo "no manifest input file $INPUT, won't build generating $OUTPUT" \
  && exit 0

../target/debug/weld toml-json $INPUT | jq > $TMP_JSON

PROVIDERS=$(cat $TMP_JSON | jq -r  '.providers[]'  | tr '\n' ' ')
ACTORS=$(cat $TMP_JSON | jq -r  '.actors[]'  | tr '\n' ' ')

# build the projects first to make sure dependencies are available
for a in $ACTORS; do
  exs_out="actor/$a/build/exs.out"
  make --quiet -C actor/$a build/exs.out || exit $?
  [ ! -f "$exs_out" ] && echo "ERROR: Build $exs_out failed" && exit 2
done
for p in $PROVIDERS; do
  make --quiet -C provider/$p build/exs.out || exit $?
  exs_out="provider/$p/build/exs.out"
  [ ! -f "$exs_out" ] && echo "ERROR: Build $exs_out failed" && exit 2
done

# turn json links map into elixir map - seems to work
format_links() {
    jq -r '.links[] | tostring'  \
    | sed -E 's/"(actor|contract|params)":/\1: /g'  \
    | sed -E 's/":/" => /g' \
    | sed -E 's/\{/%{/g' \
    | sed -E 's/$/,/'
}

{
  echo 'defmodule Project do'
  echo '  def defines() do'
  echo '   %{ actors: ['
  for a in $ACTORS; do  cat actor/$a/build/exs.out; done
  echo '   ], providers: ['
  for p in $PROVIDERS; do cat provider/$p/build/exs.out; done
  echo "   ], links: [ "
  cat $TMP_JSON | format_links
  echo "   ] } "
  echo "  end"
  # join loadactors with module
  if [ -f "$LOAD_SCRIPT" ]; then
    cat $LOAD_SCRIPT | grep -v defmodule
  else
    echo "end"
  fi
} >$OUTPUT
echo Generated $OUTPUT

