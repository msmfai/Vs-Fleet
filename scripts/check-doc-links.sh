#!/usr/bin/env bash
set -euo pipefail

root="$(pwd)"
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  root="$(git rev-parse --show-toplevel)"
fi

files=("$@")
if [ "${#files[@]}" -eq 0 ]; then
  if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "usage: $0 [markdown-file ...]"
    exit 2
  fi
  cd "$root"
  mapfile -t files < <(git ls-files '*.md')
fi

fail=0
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

for file in "${files[@]}"; do
  if [ ! -f "$file" ]; then
    echo "FAIL: missing markdown file: $file"
    fail=1
    continue
  fi

  : >"$tmp"
  perl -Mstrict -Mwarnings -e '
    my $file = shift @ARGV;
    open my $fh, "<", $file or die "open $file: $!";
    my $in_fence = 0;
    my $line_no = 0;
    while (my $line = <$fh>) {
      $line_no++;
      if ($line =~ /^\s*(```|~~~)/) {
        $in_fence = !$in_fence;
        next;
      }
      next if $in_fence;
      while ($line =~ /!?\[[^\]]+\]\(([^)]+)\)/g) {
        print "$line_no\t$1\n";
      }
      if ($line =~ /^\s*\[[^\]]+\]:\s*(\S+)/) {
        print "$line_no\t$1\n";
      }
    }
  ' "$file" >"$tmp"

  while IFS=$'\t' read -r line target; do
    [ -n "${target:-}" ] || continue

    target="${target#"${target%%[![:space:]]*}"}"
    target="${target%"${target##*[![:space:]]}"}"
    if [[ "$target" == \<*\> ]]; then
      target="${target#<}"
      target="${target%>}"
    else
      target="${target%%[[:space:]]*}"
    fi

    case "$target" in
      ""|\#*) continue ;;
    esac
    if [[ "$target" =~ ^[A-Za-z][A-Za-z0-9+.-]*: ]] || [[ "$target" == //* ]]; then
      continue
    fi

    path="${target%%#*}"
    path="${path%%\?*}"
    [ -n "$path" ] || continue

    if [[ "$path" == /* ]]; then
      full="$root/${path#/}"
    else
      full="$(dirname "$file")/$path"
    fi

    if [ ! -e "$full" ]; then
      echo "FAIL: $file:$line links to missing local target: $target"
      fail=1
    fi
  done <"$tmp"
done

if [ "$fail" -ne 0 ]; then
  exit 1
fi

echo "Documentation link check passed."
