#!/bin/sh
# Pushes article/devto-draft.md to the LIVE dev.to article (PUT, keeps slug).
# The body_markdown carries its own front matter — dev.to parses it.
: "${DEVTO_API_KEY:?export DEVTO_API_KEY=... first}"
ARTICLE_ID=4782852   # "~1 MB to orbit" — dev.to/slabb
cd "$(dirname "$0")"
python3 - "$ARTICLE_ID" <<'EOF' > /tmp/navette-put.json
import json, sys
body = open('devto-draft.md').read()
print(json.dumps({"article": {"body_markdown": body}}))
EOF
curl -s -X PUT "https://dev.to/api/articles/$ARTICLE_ID" \
  -H "Content-Type: application/json" \
  -H "api-key: $DEVTO_API_KEY" \
  -d @/tmp/navette-put.json | python3 -c \
  "import json,sys; d=json.load(sys.stdin); print('updated:', d.get('url', d))"
rm -f /tmp/navette-put.json
