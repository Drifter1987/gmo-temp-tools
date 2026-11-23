#!/usr/bin/env bash
# Build the static GitHub Pages bundle in docs/.
#
#   TEMPTOOLS_API_BASE=https://gmo-temptools-web-api-z1jsh3.freebuff.site \
#     ./scripts/build-pages.sh
#
# The TypeScript sources stay in webapp/ and web/ui.ts. The copied app.js,
# ui.js and app.css are generated, tracked artifacts because GitHub Pages serves
# them without a build step. Set TEMPTOOLS_API_BASE to the deployed API origin.

set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
API_BASE="${TEMPTOOLS_API_BASE:?set TEMPTOOLS_API_BASE to the deployed API origin}"
API_BASE="${API_BASE%/}"
case "$API_BASE" in
  https://*) ;;
  *)
    echo "TEMPTOOLS_API_BASE must be an https origin" >&2
    exit 2
    ;;
esac

cd "$ROOT"
if [ ! -d node_modules ]; then
  npm ci
fi
npm run build:web --silent

rm -rf docs
mkdir -p docs
cp webapp/index.html docs/index.html
cp webapp/app.css docs/app.css
cp webapp/app.js docs/app.js
cp web/ui.js docs/ui.js
touch docs/.nojekyll

# The local desktop server keeps using relative /api endpoints. The Pages copy
# points those requests at the hosted API instead.
sed -i "s#</head>#  <script>window.TEMPTOOLS_API_BASE = \"$API_BASE\";</script>\\n</head>#" docs/index.html

echo "built docs/ for $API_BASE"
