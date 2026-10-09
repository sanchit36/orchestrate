# Fake `gh` (body only: the caller adds `#!/bin/sh` and `D=<dir>`). It serves `gh api`
# over the fixture issues under $D (layout in mod.rs) and applies writes there.
[ "$1" = api ] || { echo "fake gh: only gh api is served" >&2; exit 2; }
shift
method=GET input=
while [ $# -gt 0 ]; do
  case "$1" in
    -X|--method) method=$2; shift 2 ;;
    --input) input=$(cat); shift 2 ;;
    -*) echo "fake gh: unexpected flag $1" >&2; exit 2 ;;
    *) path=$1; shift ;;
  esac
done
path=${path#repos/*/*/}
path=${path%%\?*}
n=${path#issues/}
n=${n%%/*}
[ -n "$n" ] && [ -d "$D/$n" ] || { echo 'gh: Not Found (HTTP 404)' >&2; exit 1; }

# Builtin reads only (every fixture file is one line): forks make the fake slow.
issue() {
  d=$D/$1
  IFS= read -r title < "$d/title.json"
  IFS= read -r body < "$d/body.json"
  IFS= read -r state < "$d/state"
  IFS= read -r labels < "$d/labels.json"
  printf '{"id":%s,"number":%s,"title":%s,"body":%s,"state":"%s","labels":%s}' \
    "$((1000 + $1))" "$1" "$title" "$body" "$state" "$labels"
}

list() {
  ids=
  [ -f "$1" ] && read -r ids < "$1"
  sep=
  printf '['
  for i in $ids; do printf '%s' "$sep"; issue "$i"; sep=,; done
  printf ']'
}

case "$method $path" in
  "GET issues/$n") issue "$n" ;;
  "PATCH issues/$n")
    case "$input" in
      '{"body":'*) printf '%s' "$input" | sed 's/^{"body":\(.*\)}$/\1/' > "$D/$n/body.json" ;;
      *'"state":"closed"'*) echo closed > "$D/$n/state" ;;
      *) echo "fake gh: unexpected edit $input" >&2; exit 2 ;;
    esac
    issue "$n" ;;
  "GET issues/$n/sub_issues") list "$D/$n/subs" ;;
  "GET issues/$n/dependencies/blocked_by") list "$D/$n/blocked_by" ;;
  "POST issues/$n/comments") printf '%s\n' "$input" >> "$D/$n/comments"; echo '{"id":1}' ;;
  *) echo "fake gh: unexpected $method $path" >&2; exit 2 ;;
esac
