#!/usr/bin/env bash
# Gate 2 of the ABI freeze (docs/adr/0004-abi-freeze.md).
#
# A pull request that changes anything under kernel/src/abi/ must either bump
# the ABI constant or carry the `abi-change` label. The label is a paper trail,
# not permission: it requires a linked ADR, which is what review checks.
#
# This gate deliberately fails CLOSED. If it cannot determine what changed — no
# merge base, missing environment — it errors rather than passing, because a
# guard that fails open is indistinguishable from no guard on the day it matters.
set -euo pipefail

ABI_DIR="kernel/src/abi"
ABI_CONST_FILE="kernel/src/abi/mod.rs"
LABEL="abi-change"

: "${BASE_SHA:?BASE_SHA is required (the pull request's base commit)}"
: "${HEAD_SHA:?HEAD_SHA is required (the pull request's head commit)}"
PR_LABELS="${PR_LABELS:-}"

if ! git cat-file -e "${BASE_SHA}^{commit}" 2>/dev/null; then
  echo "::error::base commit ${BASE_SHA} is not present; checkout needs fetch-depth: 0"
  exit 1
fi

changed="$(git diff --name-only "${BASE_SHA}" "${HEAD_SHA}" -- "${ABI_DIR}")"

if [ -z "${changed}" ]; then
  echo "no changes under ${ABI_DIR}/ — gate not applicable"
  exit 0
fi

echo "changed under ${ABI_DIR}/:"
echo "${changed}" | sed 's/^/  /'

# Read the constant from both sides rather than grepping the diff: a diff that
# merely moves the line would otherwise read as a bump.
abi_value() {
  git show "$1:${ABI_CONST_FILE}" 2>/dev/null \
    | sed -n 's/^pub const ABI: u16 = \([0-9][0-9]*\);.*/\1/p' \
    | head -n1
}

base_abi="$(abi_value "${BASE_SHA}")"
head_abi="$(abi_value "${HEAD_SHA}")"

if [ -z "${head_abi}" ]; then
  echo "::error::could not read 'pub const ABI: u16' from ${ABI_CONST_FILE} at HEAD"
  echo "::error::the guard's anchor moved; fix the guard in the same PR that moved it"
  exit 1
fi

echo "ABI: base=${base_abi:-<absent>} head=${head_abi}"

if [ -n "${base_abi}" ] && [ "${base_abi}" != "${head_abi}" ]; then
  echo "ABI bumped ${base_abi} -> ${head_abi}; gate satisfied"
  exit 0
fi

case ",${PR_LABELS}," in
  *",${LABEL},"*)
    echo "'${LABEL}' label present; gate satisfied"
    echo "::notice::an ADR must justify this change — review checks the link, not this script"
    exit 0
    ;;
esac

cat <<MSG
::error::${ABI_DIR}/ changed without an ABI bump or the '${LABEL}' label.

kernel/src/abi/ is the frozen surface (docs/adr/0004-abi-freeze.md). To proceed:

  * bump 'pub const ABI: u16' in ${ABI_CONST_FILE}, or
  * add the '${LABEL}' label and link the ADR that authorises the change.

If the change is incidental — a comment, a doc fix — move it out of ${ABI_DIR}/
or take the label. The cheapest path is not to touch this directory.
MSG
exit 1
