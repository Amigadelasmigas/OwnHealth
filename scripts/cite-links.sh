#!/usr/bin/env bash
# Check every https link that the cited data files name (the YAML, JSON and TSV files under data/:
# `cite` fields, `citation` columns, source notes) and every link of the connector pages under
# docs/connectors/ (the vendors' terms each page quotes). Fetch each link once and print a Markdown
# report. Exit 0 when every link answers, 1 when one is dead, and 2 when the check could not run (no
# links found, no curl).
#
#   bash scripts/cite-links.sh                 # the repo's data/ and docs/connectors/
#   bash scripts/cite-links.sh <dir>...        # other trees, for example a copy with a broken link
#   bash scripts/cite-links.sh --list [dir]... # print the links a run would fetch, fetch nothing
#
# A link is dead when its host does not resolve or refuses the connection or the TLS handshake, or
# when it answers 404 or 410. A link that answers 401, 403, 429 or a 5xx, or does not answer within
# 30 seconds after two retries, could not be checked: many sites refuse scripts. The report lists
# those apart, and they do not count as dead. .github/workflows/cite-links.yml runs this every week.
#
# Some hosts answer 404 to a cloud address (a GitHub runner) for a page that a browser opens. A 404
# from a host in `fake404` therefore counts as could not be checked. The host is the one the link
# ends on after its redirects. In November 2026 (issue #89) www.fda.gov did that for two live pages,
# and snomedbrowser.org, where every snomed.info concept link ends, did it for all of them.
set -uo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
only_list=0
if [ "${1:-}" = --list ]; then only_list=1; shift; fi
if [ "$#" -gt 0 ]; then dirs=("$@"); else dirs=("$root/data" "$root/docs/connectors"); fi
for d in "${dirs[@]}"; do
  [ -d "$d" ] || { echo "cite-links: no directory $d" >&2; exit 2; }
done
fake404='www.fda.gov snomedbrowser.org'
ua='Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36'
# The files a link can sit in: data files, and the Markdown of the connector pages.
types=(--include='*.yaml' --include='*.yml' --include='*.json' --include='*.tsv' --include='*.md')

# A file's name in the report: its path from the repository root when it is inside it.
name() { case "$1" in "$root"/*) printf '%s\n' "${1#"$root"/}" ;; *) printf '%s\n' "$1" ;; esac; }
files() { grep -rlF "${types[@]}" -- "$1" "${dirs[@]}" | while IFS= read -r f; do name "$f"; done; }

# A link ends at white space, a quote, a backtick or one of , } < > |, and loses a trailing . or )
# (the end of a sentence, or a Markdown link's closing parenthesis).
urls=$(grep -rhoE "${types[@]}" "https://[^\"'\`[:space:],}<>|]+" "${dirs[@]}" | sed -E 's/[.)]+$//' | sort -u)
total=$(printf '%s\n' "$urls" | grep -c '^https://')
where=$(for d in "${dirs[@]}"; do name "$d"; done | sed 's|$|/|' | paste -sd, - | sed 's/,/, /g')
[ "$total" -gt 0 ] || { echo "cite-links: no https links found under $where" >&2; exit 2; }
if [ "$only_list" -eq 1 ]; then printf '%s\n' "$urls"; exit 0; fi
command -v curl >/dev/null || { echo "cite-links: curl is not installed" >&2; exit 2; }

dead='' unchecked=''
while IFS= read -r url; do
  out=$(curl -sS -L -o /dev/null -w '%{http_code} %{url_effective}' --max-time 30 --retry 2 \
    --retry-delay 5 -A "$ua" -- "$url" 2>/dev/null)
  rc=$?
  code=${out%% *} final=${out#* }
  if [ "$rc" -ne 0 ]; then
    case "$rc" in
      6 | 7 | 35 | 51 | 60) list=dead ;;       # no such host, connection refused, TLS
      *) list=unchecked ;;                      # a timeout or another transfer error
    esac
    code="no answer (curl exit $rc)"
  else
    case "$code" in
      2?? | 3??) continue ;;
      410) list=dead ;;
      404)
        host=${final#*://}; host=${host%%[/?#]*}
        case " $fake404 " in *" $host "*) list=unchecked ;; *) list=dead ;; esac ;;
      *) list=unchecked ;;
    esac
  fi
  used=$(files "$url" | sort | paste -sd, - | sed 's/,/, /g')
  line="| $url | $code | $used |"
  if [ "$list" = dead ]; then dead+="$line"$'\n'; else unchecked+="$line"$'\n'; fi
done <<<"$urls"

n_dead=$(printf '%s' "$dead" | grep -c '^|')
n_unchecked=$(printf '%s' "$unchecked" | grep -c '^|')
echo "Checked $total links under $where: $n_dead dead, $n_unchecked could not be checked by a script."
if [ "$n_dead" -gt 0 ]; then
  printf '\n## Dead\n\n| Link | Answer | Used in |\n|---|---|---|\n%s' "$dead"
fi
if [ "$n_unchecked" -gt 0 ]; then
  printf '\n## Could not be checked by a script (open them in a browser)\n\n| Link | Answer | Used in |\n|---|---|---|\n%s' "$unchecked"
fi
[ "$n_dead" -eq 0 ]
