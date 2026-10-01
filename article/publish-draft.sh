#!/bin/sh
# Creates the DRAFT on dev.to (published: false). Requires an API key from
# https://dev.to/settings/extensions (DEV Community API Keys -> Generate).
: "${DEVTO_API_KEY:?export DEVTO_API_KEY=... first}"
cd "$(dirname "$0")"
curl -s -X POST https://dev.to/api/articles \
  -H "Content-Type: application/json" \
  -H "api-key: $DEVTO_API_KEY" \
  -d @payload.json | python3 -c "import json,sys; d=json.load(sys.stdin); print('draft created:', d.get('url', d))"
