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
# A changing cache name makes each deploy replace the old shell instead of
# keeping a stale app.js until the next background revalidation.
sed "s/__BUILD__/$(date +%s)/" webapp/sw.js > docs/sw.js
cp webapp/manifest.webmanifest docs/manifest.webmanifest
cp webapp/icon-192.png docs/icon-192.png
cp webapp/icon-512.png docs/icon-512.png
touch docs/.nojekyll

# An external config file (rather than an inline script) keeps the hosted page
# compatible with a 'self'-only script policy.
cat > docs/config.js <<EOF
window.TEMPTOOLS_API_BASE = "$API_BASE";
EOF

# The hosted bundle is the only one that gets a manifest, a service worker and
# a content policy. The desktop build keeps its plain head and same-origin API.
# frame-src is required for the sandboxed HTML message view.
sed -i "s#</head>#  <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self' $API_BASE; frame-src $API_BASE; manifest-src 'self'; worker-src 'self'; base-uri 'none'; form-action 'none'; object-src 'none'\">\n  <link rel=\"manifest\" href=\"./manifest.webmanifest\">\n  <link rel=\"icon\" href=\"./icon-192.png\">\n  <link rel=\"apple-touch-icon\" href=\"./icon-192.png\">\n  <script src=\"./config.js\"></script>\n</head>#" docs/index.html

echo "built docs/ for $API_BASE"
